/// 语音模式编排状态机——把 wake/mic/asr/vad/tts 串成贾维斯闭环：
///   喊「玄女」→ ack「我在」→ 听写（VAD 静音断句）→ intervene → 回复自动 TTS
/// 外加独立的按住说话（PTT）：录音 → ASR → 文本交还 UI（填 composer，不自动发）。
///
/// 所有浏览器依赖经 VoiceDeps 注入——本文件零 DOM/Audio API，纯逻辑可单测
/// （tests/unit/voice-controller.test.ts）。真实 wiring 见 realVoiceDeps.ts。
///
/// 状态语义：
///   off       语音模式关。PTT 仍可用（临时起 mic，用完即关，不留浏览器红点）。
///   listening 常驻唤醒中，mic PCM 流向 wake server。
///   dictating 听写中（wake 触发 VAD 自动断 / PTT 手动断），wake 暂停喂。
///   speaking  正在播玄女回复 TTS。

export type VoiceState = 'off' | 'listening' | 'dictating' | 'speaking'

export interface MicLike {
  start(): Promise<void>
  stop(): void
  subscribe(cb: (chunk: ArrayBuffer) => void): () => void
}

export interface WakeLike {
  start(): void
  stop(): void
  sendPcm(chunk: ArrayBuffer): void
}

export interface AsrLike {
  connect(): Promise<void>
  sendPcm(chunk: ArrayBuffer): void
  finish(): Promise<{ text: string }>
  abort(): void
}

export interface VadLike {
  feed(chunk: ArrayBuffer): void
  reset(): void
}

export interface TtsLike {
  play(text: string, emotion?: string): Promise<void>
  stop(): void
}

export interface VoiceDeps {
  /// GET /api/voice/tokens——cookie 登录态换 asr/tts HMAC token + wake 预共享 token
  fetchTokens(): Promise<{ imToken: string; wakeToken: string | null }>
  createMic(): MicLike
  createWake(opts: {
    token: string
    onWake: () => void
    onStatus?: (s: 'connecting' | 'ready' | 'disconnected') => void
  }): WakeLike
  createAsr(opts: { token: string }): AsrLike
  createVad(onSilence: () => void): VadLike
  createTts(token: string): TtsLike
  /// PWA 自己的 /api/intervene 走 cookie，不需要 token
  intervene(text: string): Promise<void>
}

/// 唤醒后的应答语——同 jarvis 桌宠 UX，让用户知道「她听见了」。
const WAKE_ACK = '我在'

export class VoiceController {
  private _state: VoiceState = 'off'
  private mic: MicLike | null = null
  private wake: WakeLike | null = null
  private tts: TtsLike | null = null
  private imToken: string | null = null

  private unsubWake: (() => void) | null = null
  private unsubAsr: (() => void) | null = null
  private curAsr: AsrLike | null = null
  private pttActive = false

  private stateCbs: Array<(s: VoiceState) => void> = []
  private errorCbs: Array<(msg: string) => void> = []

  constructor(private deps: VoiceDeps) {}

  get state(): VoiceState {
    return this._state
  }

  get enabled(): boolean {
    return this._state !== 'off'
  }

  onState(cb: (s: VoiceState) => void): () => void {
    this.stateCbs.push(cb)
    return () => {
      this.stateCbs = this.stateCbs.filter(c => c !== cb)
    }
  }

  onError(cb: (msg: string) => void): () => void {
    this.errorCbs.push(cb)
    return () => {
      this.errorCbs = this.errorCbs.filter(c => c !== cb)
    }
  }

  /// 开语音模式：换 token → 起 mic → 常驻 wake。wake token 缺失（home 没部署
  /// wake server）直接抛——UI 应在拿到 tokens 时就隐藏开关，这里是兜底。
  async enable(): Promise<void> {
    if (this._state !== 'off') return
    const tokens = await this.deps.fetchTokens()
    if (!tokens.wakeToken) {
      throw new Error('wake token 不可用——home 未部署唤醒服务')
    }
    this.imToken = tokens.imToken
    this.tts = this.deps.createTts(tokens.imToken)

    this.mic = this.deps.createMic()
    await this.mic.start()

    this.wake = this.deps.createWake({
      token: tokens.wakeToken,
      onWake: () => void this.startDictation()
    })
    this.wake.start()
    this.unsubWake = this.mic.subscribe(c => this.wake?.sendPcm(c))
    this.setState('listening')
  }

  async disable(): Promise<void> {
    this.unsubWake?.()
    this.unsubWake = null
    this.unsubAsr?.()
    this.unsubAsr = null
    this.curAsr?.abort()
    this.curAsr = null
    this.wake?.stop()
    this.wake = null
    this.tts?.stop()
    this.mic?.stop()
    this.mic = null
    this.pttActive = false
    this.setState('off')
  }

  /// 玄女新回复到达（UI 层从 conv WS 喂进来）。语音模式开着才念。
  onXuannvReply(text: string, emotion?: string): void {
    if (this._state === 'off' || !this.tts) return
    const t = text.trim()
    if (!t) return
    const wasListening = this._state === 'listening'
    if (wasListening) this.setState('speaking')
    void this.tts
      .play(t, emotion)
      .catch(e => this.emitError(`TTS 播放失败：${trunc(e)}`))
      .finally(() => {
        if (this._state === 'speaking') this.setState('listening')
      })
  }

  /// 按住说话开始。语音模式开着→借用现有 mic（暂停 wake 喂）；关着→临时起 mic。
  async pttStart(): Promise<void> {
    if (this.pttActive || this._state === 'dictating') return
    this.pttActive = true
    try {
      if (!this.imToken) {
        const tokens = await this.deps.fetchTokens()
        this.imToken = tokens.imToken
      }
      if (!this.mic) {
        this.mic = this.deps.createMic()
        await this.mic.start()
      }
      this.unsubWake?.()
      this.unsubWake = null
      const asr = this.deps.createAsr({ token: this.imToken })
      await asr.connect()
      this.curAsr = asr
      this.unsubAsr = this.mic.subscribe(c => asr.sendPcm(c))
      if (this._state !== 'off') this.setState('dictating')
    } catch (e) {
      this.pttActive = false
      this.cleanupDictation()
      this.afterDictation()
      throw e
    }
  }

  /// 按住说话松手——返回识别文本（空串 = 没识别出话）。不自动 intervene，
  /// 由 UI 决定填 composer 还是直接发。
  async pttStop(): Promise<string> {
    if (!this.pttActive) return ''
    this.pttActive = false
    const asr = this.curAsr
    this.cleanupDictation()
    let text = ''
    if (asr) {
      try {
        const r = await asr.finish()
        text = r.text.trim()
      } catch (e) {
        this.emitError(`听写失败：${trunc(e)}`)
      }
    }
    this.afterDictation()
    return text
  }

  // ── 内部：wake 触发的自动听写 ────────────────────────────────────────

  private async startDictation(): Promise<void> {
    if (this._state !== 'listening' || !this.mic || !this.imToken) return
    this.setState('dictating')
    this.unsubWake?.()
    this.unsubWake = null
    try {
      // ack「我在」——失败不阻断听写（sovits 挂了不该哑掉整条链）
      await this.tts?.play(WAKE_ACK).catch(() => {})
      const asr = this.deps.createAsr({ token: this.imToken })
      await asr.connect()
      const vad = this.deps.createVad(() => void this.finishDictation())
      this.curAsr = asr
      this.unsubAsr = this.mic.subscribe(c => {
        asr.sendPcm(c)
        vad.feed(c)
      })
    } catch (e) {
      this.emitError(`听写启动失败：${trunc(e)}`)
      this.cleanupDictation()
      this.afterDictation()
    }
  }

  private async finishDictation(): Promise<void> {
    const asr = this.curAsr
    if (!asr) return
    this.cleanupDictation()
    try {
      const { text } = await asr.finish()
      const t = text.trim()
      if (t) await this.deps.intervene(t)
    } catch (e) {
      this.emitError(`发送失败：${trunc(e)}`)
    }
    this.afterDictation()
  }

  /// 解除 ASR 订阅（mic 继续跑）。
  private cleanupDictation(): void {
    this.unsubAsr?.()
    this.unsubAsr = null
    this.curAsr = null
  }

  /// 听写收尾：语音模式下恢复 wake 喂回 listening；off（纯 PTT）下关临时 mic。
  private afterDictation(): void {
    if (this._state === 'off') {
      this.mic?.stop()
      this.mic = null
      return
    }
    if (this.mic && this.wake) {
      const wake = this.wake
      this.unsubWake = this.mic.subscribe(c => wake.sendPcm(c))
    }
    this.setState('listening')
  }

  private setState(s: VoiceState): void {
    this._state = s
    for (const cb of this.stateCbs) cb(s)
  }

  private emitError(msg: string): void {
    for (const cb of this.errorCbs) cb(msg)
  }
}

function trunc(e: unknown): string {
  return String(e instanceof Error ? e.message : e).slice(0, 120)
}
