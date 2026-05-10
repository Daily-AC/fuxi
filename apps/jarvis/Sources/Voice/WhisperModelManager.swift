import Foundation
import OSLog
// WhisperKit 的 `WhisperKit` 类没标 Sendable——Swift 6 严格并发下，把它放进 Task<WhisperKit, _>
// / @MainActor 实例属性都会爆 warning。我们的使用模式是「单例 main actor 持有，pipe 只在
// MainActor 上下文用」——逻辑安全，但编译器看不出来。@preconcurrency 抑制这些 warning。
@preconcurrency import WhisperKit

// 管 WhisperKit 模型生命周期。Recognizer 跟它要 pipe，不直接 init WhisperKit——
// 让模型加载/下载状态有处可问，避免每次 listening 都重 load。
//
// 选 `openai_whisper-large-v3-v20240930_turbo_632MB`（v3 dated turbo 4-bit quantized）：
//   - large-v3-turbo 是 OpenAI 2024-10 出的精简版（4 decoder layers vs large-v3 的 32），
//     英文 wer 几乎不掉、中文实测拉满，但推理速度 8x。M 系芯片 ANE 跑实时绰绰有余。
//   - dated v3 (v20240930) 是 OpenAI 2024-09-30 重训发布的 v3 权重，相比原始 v3 中文准
//     确率显著提升，是 WhisperKit README 推荐的 production 模型族。
//   - 4-bit quantized → 模型 632MB（首启 background download；之后离线可用），ANE 友好。
//
// **千万别**写成 `openai_whisper-large-v3-turbo`——HF repo `argmaxinc/whisperkit-coreml`
// 里没这个目录，WhisperKit `*<name>/*` glob 永远 miss → 死链路。已知踩过。
// 真实目录见 https://huggingface.co/argmaxinc/whisperkit-coreml/tree/main，可选：
//   - openai_whisper-large-v3-v20240930_turbo_632MB ← 我们选这个（dated turbo + 量化）
//   - openai_whisper-large-v3-v20240930_turbo       (未量化全精度 turbo，~1.6GB)
//   - openai_whisper-large-v3-v20240930_626MB       (非 turbo 但量化，更准但慢 8x)
//   - openai_whisper-large-v3_turbo                 (老 v3 turbo，注意 `_turbo` 是 _underscore_)
//
// 模型存放：`~/Library/Application Support/Xuannv/whisper-models/`。
// 选 ApplicationSupport 而非 Caches——后者系统会自动清，模型重下载体感差；
// 名字用 Xuannv（产品名「玄女」）保持跟 task spec 一致，不跟 bundleId（jarvis）混。

@MainActor
final class WhisperModelManager {
    static let shared = WhisperModelManager()

    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "whisper")

    /// WhisperKit hub 仓库 + 模型目录名。modelRepo 里的文件夹名要跟 model 字段对得上。
    /// 改名前**先去 https://huggingface.co/argmaxinc/whisperkit-coreml/tree/main 核对存在**。
    static let modelRepo = "argmaxinc/whisperkit-coreml"
    static let modelName = "openai_whisper-large-v3-v20240930_turbo_632MB"

    enum State: Equatable {
        case notLoaded
        case downloading(progress: Double)   // 0~1，仅首启
        case loading                          // 解 mlmodelc + prewarm
        case ready
        case failed(String)
    }

    private(set) var state: State = .notLoaded
    private var pipeline: WhisperKit?
    /// 加载中的 waiter——load 跑完后逐个 resume。
    /// 不缓存 `Task<WhisperKit, _>` 是为了规避 Swift 6 strict concurrency
    /// 把非 Sendable 值跨 actor 转移的 warning：MainActor 自身串行执行，
    /// continuation list 等价于「单飞 + fan-out」语义但全程不离开 main actor。
    private var loadingWaiters: [CheckedContinuation<WhisperKit, Error>] = []
    private var loading = false

    /// 状态变更回调——UI 可订阅显示「模型加载中…」 / 「下载 42%」。
    var onStateChange: ((State) -> Void)?

    private init() {}

    /// 模型存放目录。第一次启动会创建。
    var storageURL: URL {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory,
                                                   in: .userDomainMask).first!
        return appSupport
            .appendingPathComponent("Xuannv", isDirectory: true)
            .appendingPathComponent("whisper-models", isDirectory: true)
    }

    /// 起 load——idempotent。重入挂到 waiter 列表，第一个调用方负责真起加载。
    /// 调用方在 awaiting 期间应通过 state 反馈给用户。
    func ensureLoaded() async throws -> WhisperKit {
        if let pipeline, state == .ready {
            return pipeline
        }
        if loading {
            return try await withCheckedThrowingContinuation { cont in
                loadingWaiters.append(cont)
            }
        }
        loading = true
        do {
            let pipe = try await performLoad()
            self.pipeline = pipe
            self.transition(to: .ready)
            self.loading = false
            // 把同期等待的 waiter 都放掉。
            let waiters = loadingWaiters
            loadingWaiters.removeAll()
            for w in waiters { w.resume(returning: pipe) }
            return pipe
        } catch {
            self.transition(to: .failed(error.localizedDescription))
            self.loading = false
            let waiters = loadingWaiters
            loadingWaiters.removeAll()
            for w in waiters { w.resume(throwing: error) }
            throw error
        }
    }

    /// 同步快查——已 ready 时拿 pipeline；未 ready 返 nil 让调用方放弃本轮。
    var readyPipeline: WhisperKit? {
        state == .ready ? pipeline : nil
    }

    private func performLoad() async throws -> WhisperKit {
        // 准备目录。WhisperKit 自带下载逻辑，downloadBase 落到这里就行。
        try FileManager.default.createDirectory(at: storageURL,
                                                 withIntermediateDirectories: true)

        let alreadyLocal = isModelOnDisk()
        if !alreadyLocal {
            transition(to: .downloading(progress: 0))
        } else {
            transition(to: .loading)
        }

        logger.notice("WhisperKit 加载开始: model=\(Self.modelName, privacy: .public) repo=\(Self.modelRepo, privacy: .public) base=\(self.storageURL.path, privacy: .public) localCached=\(alreadyLocal)")

        // 注：WhisperKit 0.x 的 convenience init 没暴露 download progress callback；
        // downloading 状态只是粗粒度 placeholder。后续如果库给了 progress hook 再细化。
        let config = WhisperKitConfig(
            model: Self.modelName,
            downloadBase: storageURL,
            modelRepo: Self.modelRepo,
            verbose: false,
            logLevel: .error,
            prewarm: true,
            load: true,
            download: true
        )

        if !alreadyLocal {
            transition(to: .loading)
        }
        let pipe = try await WhisperKit(config)
        logger.notice("WhisperKit 加载完成")
        return pipe
    }

    /// 模型是否已在本地——用 modelFolder 的子目录是否非空大致判断（足够 UI 状态用，
    /// 真实合法性由 WhisperKit 加载时校验）。
    private func isModelOnDisk() -> Bool {
        let modelDir = storageURL
            .appendingPathComponent("argmaxinc")
            .appendingPathComponent("whisperkit-coreml")
            .appendingPathComponent(Self.modelName)
        guard let contents = try? FileManager.default.contentsOfDirectory(atPath: modelDir.path) else {
            return false
        }
        return !contents.isEmpty
    }

    private func transition(to newState: State) {
        state = newState
        onStateChange?(newState)
    }
}
