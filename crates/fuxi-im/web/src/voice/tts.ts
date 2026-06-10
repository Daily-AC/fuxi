/// home /api/tts —— GPT-SoVITS 角色音色 TTS（ref 音色由 sovits-proxy 配置决定）。
/// 移植自 jarvis-pet（apps/jarvis-pet/src/voice/tts.ts），跟桌宠同一颗代理，音色一致。
///
/// 协议（见 deploy/sovits/tts_proxy.py）：
///   POST /api/tts
///   Authorization: Bearer <token>
///   Body: {"text": "玄女要念的内容", "emotion": "happy" | ...?}
///   → wav binary (Content-Type: audio/wav)
///
/// emotion 可选，未传 / 未知 / 后端缺 ref 均 fallback normal。
/// 前端不校验 emotion 字符串——后端是 single source of truth。
///
/// AudioContext 解码 wav → AudioBufferSourceNode 播。

let _ctx: AudioContext | null = null
let _curSrc: AudioBufferSourceNode | null = null

function audioCtx(): AudioContext {
  if (!_ctx) _ctx = new AudioContext()
  return _ctx
}

/// 播一段 TTS——返回 Promise 在 wav 播完后 resolve。
/// 同时只能播一段；新调用会立即停掉旧的。onStep 给每步状态便于桌宠 toast 诊断。
export async function playTts(opts: {
  baseURL: string
  token: string
  text: string
  /// Phase 3：可选 emotion 透传到后端 sovits-proxy；undefined / 空串 → 后端走 normal
  emotion?: string
  /// 调试钩子：每个里程碑回调一次（"fetching" / "got_wav 64324B" /
  /// "ctx state=running" / "decoded 4.2s" / "playing" / "ended"）
  onStep?: (msg: string) => void
  onPlay?: (durationSec: number) => void
  onEnd?: () => void
}): Promise<void> {
  const step = (m: string) => opts.onStep?.(m)
  // 停旧
  if (_curSrc) {
    try { _curSrc.stop() } catch { /* ignore */ }
    _curSrc = null
  }

  step('fetching')
  const r = await fetch(`${opts.baseURL}/api/tts`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${opts.token}`
    },
    body: JSON.stringify(
      opts.emotion
        ? { text: opts.text, emotion: opts.emotion }
        : { text: opts.text }
    )
  })
  if (!r.ok) {
    const body = await r.text().catch(() => '')
    throw new Error(`tts ${r.status}: ${body.slice(0, 100)}`)
  }
  const wavBuf = await r.arrayBuffer()
  step(`got_wav ${wavBuf.byteLength}B`)
  const ctx = audioCtx()
  step(`ctx state=${ctx.state}`)
  if (ctx.state === 'suspended') {
    try {
      await ctx.resume()
      step(`ctx resumed -> ${ctx.state}`)
    } catch (e) {
      step(`ctx resume fail: ${String(e).slice(0, 40)}`)
    }
  }
  const audioBuf = await ctx.decodeAudioData(wavBuf.slice(0))
  step(`decoded ${audioBuf.duration.toFixed(1)}s ${audioBuf.numberOfChannels}ch ${audioBuf.sampleRate}Hz`)
  const src = ctx.createBufferSource()
  src.buffer = audioBuf
  src.connect(ctx.destination)
  _curSrc = src

  const duration = audioBuf.duration
  opts.onPlay?.(duration)

  return new Promise(resolve => {
    src.onended = () => {
      if (_curSrc === src) _curSrc = null
      step('ended')
      opts.onEnd?.()
      resolve()
    }
    src.start()
    step('playing')
  })
}

export function stopTts(): void {
  if (_curSrc) {
    try { _curSrc.stop() } catch { /* ignore */ }
    _curSrc = null
  }
}
