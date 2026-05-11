/// home /api/tts —— GPT-SoVITS 心海音色 TTS。
///
/// 协议（见 deploy/sovits/tts_proxy.py）：
///   POST /api/tts
///   Authorization: Bearer <token>
///   Body: {"text": "玄女要念的内容"}
///   → wav binary (Content-Type: audio/wav)
///
/// 桌宠用 AudioContext 解码 wav → AudioBufferSourceNode 播。声学跟药丸 v0.2
/// 一样的链路（同一颗 sovits-proxy），所以音色一致。

let _ctx: AudioContext | null = null
let _curSrc: AudioBufferSourceNode | null = null

function audioCtx(): AudioContext {
  if (!_ctx) _ctx = new AudioContext()
  return _ctx
}

/// 播一段 TTS——返回 Promise 在 wav 播完后 resolve。
/// 同时只能播一段；新调用会立即停掉旧的。
export async function playTts(opts: {
  baseURL: string
  token: string
  text: string
  /// 播放进度回调（含开始 / 停止）—— Phase 2 用于切 Say 动画
  onPlay?: (durationSec: number) => void
  onEnd?: () => void
}): Promise<void> {
  // 停旧
  if (_curSrc) {
    try { _curSrc.stop() } catch { /* ignore */ }
    _curSrc = null
  }

  const r = await fetch(`${opts.baseURL}/api/tts`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${opts.token}`
    },
    body: JSON.stringify({ text: opts.text })
  })
  if (!r.ok) {
    const body = await r.text().catch(() => '')
    throw new Error(`tts ${r.status}: ${body.slice(0, 100)}`)
  }
  const wavBuf = await r.arrayBuffer()
  const ctx = audioCtx()
  if (ctx.state === 'suspended') {
    // Safari/WebKit 要求用户手势后才能 resume。桌宠用户已点过菜单或说过话才走
    // 到这里，所以 resume 应该成功。
    try { await ctx.resume() } catch { /* ignore */ }
  }
  const audioBuf = await ctx.decodeAudioData(wavBuf)
  const src = ctx.createBufferSource()
  src.buffer = audioBuf
  src.connect(ctx.destination)
  _curSrc = src

  const duration = audioBuf.duration
  opts.onPlay?.(duration)

  return new Promise(resolve => {
    src.onended = () => {
      if (_curSrc === src) _curSrc = null
      opts.onEnd?.()
      resolve()
    }
    src.start()
  })
}

export function stopTts(): void {
  if (_curSrc) {
    try { _curSrc.stop() } catch { /* ignore */ }
    _curSrc = null
  }
}
