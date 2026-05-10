import Foundation
@preconcurrency import AVFoundation
import OSLog
@preconcurrency import WhisperKit

/// 语音转写——接 AECEngine 的 16kHz mono Float32 fan-out，喂 WhisperKit large-v3-turbo。
///
/// 替原来的 SFSpeechRecognizer zh-CN（系统中文识别准确率烂——实测「帮我把伏羲跑起来」
/// 经常听成「帮我把胡子刮起来」）。WhisperKit 走 Core ML + ANE 加速，模型一次性加载常驻。
///
/// 由于 WhisperKit 不暴露真 streaming API（`transcribeStream` 在 oss-swift 1.0 里没有），
/// 这里采用「滚动 buffer + 周期性 transcribe」的近似 partial 方案：
///   1. AECEngine listener `"main-stt-whisper"` 收 16kHz Float32 buffer，append 到内部 ring
///   2. 每 ~1.2s 截一次「迄今全部 audio」喂 WhisperKit.transcribe(audioArray:)，输出当成 partial
///   3. stop() 时再做一次最终 transcribe，标 isFinal=true
///
/// **不**做端点检测——T3 的 VAD agent 负责。Recognizer 暴露 `finalize()` 给 VAD 触发收尾。
/// **不**复用「上一轮 transcribe」结果——whisper 每次都从头吃完整音频，partial 就是越来越长的
/// 重新转写。1.2s 间隔在 large-v3-turbo 上 M-series 实测延迟 ~400ms，能跟上正常语速。
@MainActor
final class Recognizer {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "speech")

    private let onResult: (String, Bool) -> Void
    private let onLevel: (Double) -> Void

    /// audio thread 跟 main 共享的 PCM ring——audio thread append、main 读快照转写。
    private let buffer = PCMBuffer()
    /// AECEngine listener id——进程内最多一个 Recognizer 实例 hardcode 即可。
    nonisolated static let listenerID = "main-stt-whisper"

    /// 周期性 partial transcribe 触发器。stop() 时取消。
    private var partialTimer: DispatchSourceTimer?
    private static let partialInterval: TimeInterval = 1.2

    /// 防止 partial transcribe 任务串行重叠（whisper 一次推理 ~300-500ms，两次紧挨会堆任务）。
    private var partialInFlight = false
    private var lastPartialText = ""
    /// 标记当前是否处于 listening——stop()/finalize 后 partial timer 触发要忽略。
    private var listening = false

    init(onResult: @escaping (String, Bool) -> Void,
         onLevel: @escaping (Double) -> Void = { _ in }) {
        self.onResult = onResult
        self.onLevel = onLevel
        // 预触发模型加载——首启时 ~800MB 下载，越早起越好。等用户唤醒时大概率已就绪。
        Task { @MainActor in
            do {
                _ = try await WhisperModelManager.shared.ensureLoaded()
            } catch {
                logger.error("Whisper 模型预加载失败: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    func start() {
        stop()
        do {
            try AECEngine.shared.start()
        } catch {
            logger.error("AECEngine start 失败: \(error.localizedDescription, privacy: .public)")
            return
        }

        // WhisperKit 期望 16kHz mono Float32——AECEngine 内部 AVAudioConverter 帮忙转。
        guard let targetFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                               sampleRate: 16_000,
                                               channels: 1,
                                               interleaved: false) else {
            logger.error("16kHz target format 构造失败")
            return
        }

        buffer.reset()
        listening = true
        lastPartialText = ""

        // 模型未就绪时给 UI 一个提示，listener 照常挂；transcribe 在第一次 timer 触发时
        // 看 readyPipeline，没就绪就跳过。
        if WhisperModelManager.shared.readyPipeline == nil {
            onResult("(模型加载中…)", false)
            // 起一条独立 task 等模型加载完——加载完后清掉占位文本，partial 自然续上。
            Task { @MainActor [weak self] in
                _ = try? await WhisperModelManager.shared.ensureLoaded()
                guard let self, self.listening else { return }
                if self.lastPartialText.isEmpty {
                    self.onResult("", false)
                }
            }
        }

        let buffer = self.buffer
        let onLevel = self.onLevel
        AECEngine.shared.addListener(id: Self.listenerID, format: targetFormat) { audioBuffer in
            // audio thread——append 到 ring + 算 RMS hop 到 main。
            buffer.append(audioBuffer)
            let level = computeRMSLevel(audioBuffer)
            Task { @MainActor in onLevel(level) }
        }

        armPartialTimer()
    }

    /// 标记最终一次转写——T3 VAD onSpeechEnd 调；外部 stop() 也走这里。
    /// 跑完后 partial timer 取消、listener 移除、isFinal=true 推一次给 onResult。
    func finalize() {
        guard listening else { return }
        listening = false
        partialTimer?.cancel()
        partialTimer = nil
        AECEngine.shared.removeListener(id: Self.listenerID)
        onLevel(0)

        let snapshot = buffer.snapshot()
        guard !snapshot.isEmpty,
              let pipe = WhisperModelManager.shared.readyPipeline else {
            // 模型没好或没收到声——直接发空 final，让 AppState 的兜底逻辑接管
            onResult(lastPartialText, true)
            return
        }
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let text = try await Self.runTranscribe(pipe: pipe, audio: snapshot)
                self.onResult(text, true)
            } catch {
                self.logger.error("final transcribe 失败: \(error.localizedDescription, privacy: .public)")
                self.onResult(self.lastPartialText, true)
            }
        }
    }

    func stop() {
        if listening {
            // 用户主动 stop（例：再按热键收回）——走 finalize 路径让 partial 落 isFinal。
            // 注意：finalize 是 async，stop 同步返回；listener 在 finalize 内已立即 removed。
            finalize()
            return
        }
        partialTimer?.cancel()
        partialTimer = nil
        AECEngine.shared.removeListener(id: Self.listenerID)
        onLevel(0)
    }

    // MARK: - 内部

    private func armPartialTimer() {
        partialTimer?.cancel()
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + Self.partialInterval, repeating: Self.partialInterval)
        t.setEventHandler { [weak self] in
            Task { @MainActor [weak self] in
                self?.tickPartial()
            }
        }
        t.resume()
        partialTimer = t
    }

    private func tickPartial() {
        guard listening else { return }
        guard !partialInFlight else { return }
        guard let pipe = WhisperModelManager.shared.readyPipeline else { return }
        let snapshot = buffer.snapshot()
        // 太短没必要喂——whisper 对 <0.3s 输入容易瞎填字
        guard snapshot.count >= 16_000 / 3 else { return }
        partialInFlight = true
        Task { @MainActor [weak self] in
            defer { self?.partialInFlight = false }
            guard let self else { return }
            do {
                let text = try await Self.runTranscribe(pipe: pipe, audio: snapshot)
                guard self.listening else { return }
                if text != self.lastPartialText {
                    self.lastPartialText = text
                    self.onResult(text, false)
                }
            } catch {
                self.logger.error("partial transcribe 失败: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// transcribe 的薄包装——`detectLanguage=false` + 锁 zh，避免短 partial 被识别成英文。
    /// `usePrefillPrompt`/`temperatureFallbackCount` 走默认；后续要 tune 再说。
    private static func runTranscribe(pipe: WhisperKit, audio: [Float]) async throws -> String {
        let options = DecodingOptions(
            verbose: false,
            task: .transcribe,
            language: "zh",
            temperature: 0.0,
            temperatureFallbackCount: 3,
            sampleLength: 224,
            usePrefillPrompt: true,
            skipSpecialTokens: true,
            withoutTimestamps: true
        )
        let results = try await pipe.transcribe(audioArray: audio, decodeOptions: options)
        // results 多段拼起来——partial 走滚动 buffer 实际只有一段，但兜底处理一下。
        let joined = results.map { $0.text }.joined(separator: "")
        return cleanWhisperText(joined)
    }
}

// MARK: - PCM ring + utils

/// audio thread append、main thread snapshot。NSLock 守 storage——append 短临界区，
/// snapshot 拷贝整段（whisper 反正要全量）。
final class PCMBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var samples: [Float] = []

    func reset() {
        lock.lock(); defer { lock.unlock() }
        samples.removeAll(keepingCapacity: true)
    }

    func append(_ buffer: AVAudioPCMBuffer) {
        guard let channelData = buffer.floatChannelData?[0] else { return }
        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else { return }
        let chunk = UnsafeBufferPointer(start: channelData, count: frameCount)
        lock.lock()
        samples.append(contentsOf: chunk)
        lock.unlock()
    }

    func snapshot() -> [Float] {
        lock.lock(); defer { lock.unlock() }
        return samples
    }

    var count: Int {
        lock.lock(); defer { lock.unlock() }
        return samples.count
    }
}

/// whisper 输出经常带 `<|notimestamps|>` 残留特殊 token + 头尾空格——剥掉。
/// `withoutTimestamps=true` 已让 segment 里不带 `<|0.00|>`，但首尾偶尔还漏。
private func cleanWhisperText(_ raw: String) -> String {
    var s = raw
    // 去常见控制 token 残留
    let tokens = ["<|startoftranscript|>", "<|endoftext|>", "<|notimestamps|>",
                  "<|zh|>", "<|transcribe|>"]
    for t in tokens { s = s.replacingOccurrences(of: t, with: "") }
    return s.trimmingCharacters(in: .whitespacesAndNewlines)
}

/// 从 PCM buffer 算 RMS → log 归一化到 0~1。静音 ~0.0；正常语音 0.2~0.6；大声 0.8+。
/// nonisolated，audio thread 上跑。
private func computeRMSLevel(_ buffer: AVAudioPCMBuffer) -> Double {
    guard let channelData = buffer.floatChannelData?[0] else { return 0 }
    let frameCount = Int(buffer.frameLength)
    guard frameCount > 0 else { return 0 }
    var sum: Float = 0
    for i in 0..<frameCount {
        let s = channelData[i]
        sum += s * s
    }
    let rms = sqrt(sum / Float(frameCount))
    let db = 20 * log10(max(rms, 1e-7))
    return max(0, min(1, (Double(db) + 50) / 40))
}
