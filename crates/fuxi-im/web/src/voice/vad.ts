/// 极简 energy-based VAD。每收一个 Int16 PCM chunk 算 RMS，低于阈值连续 N
/// 个 chunk → 视为静音段 → 触发 onSilence 让上层 finish ASR。
///
/// 这是 Phase 2 MVP 版——不接 silero-vad ML 模型省工程量。对桌宠的「按住说话」
/// 场景够用：用户停说 1.5s 自动断。室内环境噪声 RMS 通常 < 300 (int16 range
/// 0-32767)，正常说话 1500-8000，threshold 600 是稳的中间值。

export interface VadOpts {
  /// RMS 阈值（int16 abs scale）。默认 600；环境噪声大时调高
  threshold?: number
  /// 连续多少个 chunk 静音视为「停止说话」。chunk ~85ms（MicRecorder 默认）
  /// → 18 chunks ≈ 1.5s
  silenceChunks?: number
  /// 至少先有多少个 chunk 非静音才允许触发 onSilence（避免一开就 fire）
  minVoiceChunks?: number
  /// 静音条件满足触发
  onSilence: () => void
}

export class EnergyVad {
  private silentRun = 0
  private voiceCount = 0
  private fired = false
  private opts: Required<Omit<VadOpts, 'onSilence'>> & { onSilence: () => void }

  constructor(opts: VadOpts) {
    this.opts = {
      threshold: opts.threshold ?? 600,
      silenceChunks: opts.silenceChunks ?? 18,
      minVoiceChunks: opts.minVoiceChunks ?? 3,
      onSilence: opts.onSilence
    }
  }

  feed(pcm: ArrayBuffer): void {
    if (this.fired) return
    const i16 = new Int16Array(pcm)
    let sumSq = 0
    for (let i = 0; i < i16.length; i++) {
      const s = i16[i] ?? 0
      sumSq += s * s
    }
    const rms = Math.sqrt(sumSq / Math.max(1, i16.length))
    if (rms >= this.opts.threshold) {
      this.silentRun = 0
      this.voiceCount++
    } else {
      this.silentRun++
      if (
        this.voiceCount >= this.opts.minVoiceChunks &&
        this.silentRun >= this.opts.silenceChunks
      ) {
        this.fired = true
        this.opts.onSilence()
      }
    }
  }

  reset(): void {
    this.silentRun = 0
    this.voiceCount = 0
    this.fired = false
  }
}
