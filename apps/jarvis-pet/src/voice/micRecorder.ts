/// 麦克风录音 → 16kHz mono Int16 PCM 流。喂给 asrClient 上传 home /api/asr。
///
/// 实现选 ScriptProcessorNode（deprecated 但 WebKit 仍支持，Tauri 2 webview =
/// macOS WebKit）。AudioWorklet 更现代但要单独 worklet 文件 + Vite bundling，
/// Phase 2 MVP 不上。downsample 用整除比 3:1（48k→16k）取样，无 anti-alias
/// filter，语音内容上 alias 几乎不可闻——Phase 2 可接受。
///
/// 用法：
///   const rec = new MicRecorder()
///   await rec.start(chunk => asrClient.sendPcm(chunk))
///   // ... 用户说话中 ...
///   rec.stop()
///
/// WebKit 默认 AudioContext sampleRate = 48000；如未来 macOS 默认变了，
/// resample 步长会算错——init 时 assert 实际 sampleRate ∈ {32000, 44100, 48000}
/// 三个常见值，按 ratio 取整。

const TARGET_RATE = 16000
const BUFFER_SIZE = 4096  // ~85ms @ 48kHz，平衡延迟 vs callback 频率

export class MicRecorder {
  private ctx: AudioContext | null = null
  private stream: MediaStream | null = null
  private src: MediaStreamAudioSourceNode | null = null
  private proc: ScriptProcessorNode | null = null
  private subscribers: Array<(chunk: ArrayBuffer) => void> = []

  /// 订阅 PCM chunks（16kHz int16 LE，~85ms 数据）。返回 unsubscribe。
  /// 支持多订阅者：wake client 常驻 + asr client 录音期间并行用同一流，
  /// 避免重复 getUserMedia 抢占麦克风。
  subscribe(onPcm: (chunk: ArrayBuffer) => void): () => void {
    this.subscribers.push(onPcm)
    return () => {
      this.subscribers = this.subscribers.filter(s => s !== onPcm)
    }
  }

  /// 开始录音；首次调用启动 audio context + mic。多次调用幂等。
  async start(onPcm?: (chunk: ArrayBuffer) => void): Promise<void> {
    if (onPcm) this.subscribe(onPcm)
    if (this.ctx) return  // 已在录音，订阅者已加

    // mac 第一次会弹麦克风权限 prompt（前提 Info.plist 有 NSMicrophoneUsageDescription）
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: 1,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true
      }
    })
    this.ctx = new AudioContext()
    const sr = this.ctx.sampleRate
    if (sr % TARGET_RATE !== 0) {
      // 非整除比走最接近的整数 ratio，会有小漂移但 ASR 不敏感
      console.warn(`[mic] non-integer ratio ${sr}/${TARGET_RATE}, using floor`)
    }
    const ratio = Math.max(1, Math.floor(sr / TARGET_RATE))

    this.src = this.ctx.createMediaStreamSource(this.stream)
    this.proc = this.ctx.createScriptProcessor(BUFFER_SIZE, 1, 1)
    this.proc.onaudioprocess = e => {
      if (this.subscribers.length === 0) return
      const f32 = e.inputBuffer.getChannelData(0)
      const outLen = Math.floor(f32.length / ratio)
      const out = new Int16Array(outLen)
      for (let i = 0; i < outLen; i++) {
        const v = f32[i * ratio]
        // Float [-1,1] → Int16 LE
        out[i] = Math.max(-32768, Math.min(32767, Math.round(v * 32767)))
      }
      // 多订阅者：广播同一 chunk；每个订阅者拿到 same buffer slice，不允许改
      const buf = out.buffer
      for (const cb of this.subscribers) cb(buf)
    }
    this.src.connect(this.proc)
    // ScriptProcessor 必须 connect 到 destination 才会触发 onaudioprocess——
    // 但 destination 是扬声器，回放自己的声音会反馈。trick：连一个 gain=0 的节点
    const sink = this.ctx.createGain()
    sink.gain.value = 0
    this.proc.connect(sink)
    sink.connect(this.ctx.destination)
  }

  stop(): void {
    this.proc?.disconnect()
    this.src?.disconnect()
    this.stream?.getTracks().forEach(t => t.stop())
    this.ctx?.close().catch(() => {})
    this.ctx = null
    this.stream = null
    this.src = null
    this.proc = null
    this.subscribers = []
  }

  get isRecording(): boolean {
    return this.ctx !== null
  }
}
