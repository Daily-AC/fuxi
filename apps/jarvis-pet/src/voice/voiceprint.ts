/// 声纹注册：捕获 N 秒 16kHz mono PCM → 拼 WAV → base64 → 上传 home /api/sv/enroll。
///
/// 复用 MicRecorder 的 PCM 流（已经做了 48k→16k 下采样 + Int16 编码），只需要
/// 攒满 N 秒后停订阅 + 拼一个标准 WAV 头。后端 sv_server.py 用 soundfile.read
/// 解，所以必须**真正的 WAV 容器**（不是 raw PCM）——44 字节 PCM_16 header
/// 自己拼，避免引依赖。
///
/// 协议（见 deploy/sv/sv_server.py）：
///   POST /api/sv/enroll
///   Authorization: Bearer <pair token>
///   Body: {"wav_b64": "<base64 of wav file>"}
///   → {"enrolled": true, "dim": 192, "owner_path": "..."}
///
/// verify 也走同 helper：用 `verifyUrl` = /api/sv/verify 入口。

import type { MicRecorder } from './micRecorder'

const SAMPLE_RATE = 16000

/// 收 mic 流 N 秒，返回 Int16Array（拼好的 mono 16kHz PCM）。
/// onTick 每个 chunk 回调一次「已录 X.Xs / RMS Y」，给 UI 画进度条 + 音量条。
export async function captureForSeconds(opts: {
  mic: MicRecorder
  seconds: number
  onTick?: (info: { elapsedSec: number; rms: number }) => void
}): Promise<Int16Array> {
  const targetSamples = Math.floor(opts.seconds * SAMPLE_RATE)
  const collected: Int16Array[] = []
  let total = 0

  return new Promise((resolve, reject) => {
    let done = false
    let unsub: (() => void) | null = null
    const finish = (val: Int16Array | Error) => {
      if (done) return
      done = true
      unsub?.()
      if (val instanceof Error) reject(val)
      else resolve(val)
    }

    // 超时兜底：mic 不喂数据时 N+5 秒 reject
    const timeout = setTimeout(() => {
      finish(new Error(`捕获超时（${opts.seconds + 5}s 内未收满 PCM）`))
    }, (opts.seconds + 5) * 1000)

    unsub = opts.mic.subscribe(chunk => {
      const i16 = new Int16Array(chunk)
      collected.push(i16.slice()) // copy；subscribers 共享同一 buffer 不能改
      total += i16.length
      let sumSq = 0
      for (let i = 0; i < i16.length; i++) sumSq += i16[i] * i16[i]
      const rms = Math.sqrt(sumSq / i16.length) / 32768
      const elapsedSec = total / SAMPLE_RATE
      opts.onTick?.({ elapsedSec, rms })
      if (total >= targetSamples) {
        clearTimeout(timeout)
        // 截到刚好 targetSamples
        const out = new Int16Array(targetSamples)
        let offset = 0
        for (const part of collected) {
          const take = Math.min(part.length, targetSamples - offset)
          out.set(part.subarray(0, take), offset)
          offset += take
          if (offset >= targetSamples) break
        }
        finish(out)
      }
    })
  })
}

/// PCM int16 mono → WAV bytes（44 字节 header + raw samples）。
/// 跟 sv_server.py 用 soundfile.read 期望的契约一致。
export function pcmToWav(pcm: Int16Array, sampleRate = SAMPLE_RATE): Uint8Array {
  const dataSize = pcm.length * 2
  const buf = new ArrayBuffer(44 + dataSize)
  const v = new DataView(buf)
  // RIFF chunk
  writeStr(v, 0, 'RIFF')
  v.setUint32(4, 36 + dataSize, true)
  writeStr(v, 8, 'WAVE')
  // fmt chunk
  writeStr(v, 12, 'fmt ')
  v.setUint32(16, 16, true) // fmt chunk size
  v.setUint16(20, 1, true) // PCM format
  v.setUint16(22, 1, true) // mono
  v.setUint32(24, sampleRate, true)
  v.setUint32(28, sampleRate * 2, true) // byte rate
  v.setUint16(32, 2, true) // block align
  v.setUint16(34, 16, true) // bits per sample
  // data chunk
  writeStr(v, 36, 'data')
  v.setUint32(40, dataSize, true)
  // samples
  const out = new Uint8Array(buf)
  const samplesView = new Int16Array(buf, 44)
  samplesView.set(pcm)
  return out
}

function writeStr(v: DataView, offset: number, s: string): void {
  for (let i = 0; i < s.length; i++) v.setUint8(offset + i, s.charCodeAt(i))
}

/// Uint8Array → base64（chunk-wise，避免单次 btoa 大字符串 OOM）。
export function bytesToBase64(bytes: Uint8Array): string {
  const chunkSize = 0x8000
  let binary = ''
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const chunk = bytes.subarray(i, i + chunkSize)
    binary += String.fromCharCode.apply(null, Array.from(chunk))
  }
  return btoa(binary)
}

/// POST /api/sv/enroll，body { wav_b64 }。token 用桌宠的 pairToken（fuxi-im HMAC）。
/// 返 sv_server 响应：{enrolled, dim, owner_path}。
export async function uploadEnroll(opts: {
  baseURL: string
  token: string
  wavBytes: Uint8Array
}): Promise<{ enrolled: boolean; dim: number; ownerPath?: string }> {
  const wavB64 = bytesToBase64(opts.wavBytes)
  const r = await fetch(`${opts.baseURL}/api/sv/enroll`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${opts.token}`,
    },
    body: JSON.stringify({ wav_b64: wavB64 }),
  })
  if (!r.ok) {
    const body = await r.text().catch(() => '')
    throw new Error(`enroll ${r.status}: ${body.slice(0, 200)}`)
  }
  const j = await r.json()
  return {
    enrolled: !!j.enrolled,
    dim: j.dim || 0,
    ownerPath: j.owner_path,
  }
}

/// 同 enroll 但走 /verify——用户录完后跑一次 sanity check（看 score 是不是
/// 真的 ≥ threshold；防 mic 录到环境噪音导致 enrollment 无效）。
export async function uploadVerify(opts: {
  baseURL: string
  token: string
  wavBytes: Uint8Array
}): Promise<{ match: boolean; score: number; threshold: number; enrolled: boolean }> {
  const wavB64 = bytesToBase64(opts.wavBytes)
  const r = await fetch(`${opts.baseURL}/api/sv/verify`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${opts.token}`,
    },
    body: JSON.stringify({ wav_b64: wavB64 }),
  })
  if (!r.ok) {
    const body = await r.text().catch(() => '')
    throw new Error(`verify ${r.status}: ${body.slice(0, 200)}`)
  }
  const j = await r.json()
  return {
    match: !!j.match,
    score: typeof j.score === 'number' ? j.score : 0,
    threshold: typeof j.threshold === 'number' ? j.threshold : 0.3,
    enrolled: !!j.enrolled,
  }
}

/// 推荐用户朗读的语料——20s 自然话量 + 包含足够元音覆盖；CAM++ 抽 embedding
/// 对内容不敏感，关键是**自然语调** + **包含常用音素**。这段是经典声纹采集语料
/// （CMU/AESRC 等 corpus 类似设计）。
export const SAMPLE_TEXT = [
  '你好，我是以琳。今天天气还不错，桌宠也运行得很顺。',
  '玄女是我专属的助理，平时会帮我安排各种小事——',
  '从写代码到记笔记，再到泡杯冰美式。',
  '现在我要让她记住我的声音，以后陌生人喊她也不会再应。',
].join('\n')
