import Foundation
import OSLog

/// 唤醒词「玄女」识别——Picovoice Porcupine 包装。
///
/// **当前状态**：占位（no-op）。要激活按以下步骤：
///
/// ## 一次性准备
///
/// 1. 注册 Picovoice Console（https://console.picovoice.ai/） — 个人 plan 免费
/// 2. 取 access key（Settings → Access Key），填进贾维斯 → 设置 → 唤醒
/// 3. 训练自定义中文唤醒词「玄女」：
///    - Console → Porcupine → "Create Wake Word"
///    - Language: Chinese (Mandarin)
///    - Phrase: 玄女
///    - Platform: macOS
///    - Train → 下载 `xuannv_zh_mac_v3_0_0.ppn`
/// 4. 把 .ppn 放进 `apps/jarvis/Resources/`（已 gitignore，不入仓——Picovoice 模型
///    许可证可能不允许公开分发）
///
/// ## 代码接入（每次升级 Porcupine 主版本时回这）
///
/// 1. project.yml `packages` 段解开注释：
///    ```yaml
///    Porcupine:
///      url: https://github.com/Picovoice/porcupine.git
///      from: "3.0.0"
///    ```
///    main target `dependencies` 加 `- package: Porcupine, product: Porcupine`
///    `xcodegen generate` 重生工程
/// 2. 这个文件的 `import Foundation` 下加 `import Porcupine`
/// 3. `start()` 改为：
///    ```swift
///    let modelURL = Bundle.main.url(forResource: "xuannv_zh_mac_v3_0_0",
///                                    withExtension: "ppn")!
///    let mgr = try PorcupineManager(accessKey: accessKey,
///                                    keywordPath: modelURL.path,
///                                    onDetection: { _ in self.onWake() })
///    try mgr.start()
///    self.manager = mgr
///    ```
/// 4. AppState.bootstrap 在 settings.triggerMode == .wakeWord/.both 时实例化 WakeWord
///
/// ## always-on 时的音频抢占
///
/// Porcupine + SFSpeechRecognizer 不能同时占麦克风。Recognizer.start() 前必须
/// `wakeWord?.pause()`，Recognizer.stop() 后 `wakeWord?.resume()`。AppState 的
/// `startListening` / `stopListening` 加这两行就齐了。
final class WakeWord {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "wakeword")
    private let onWake: () -> Void

    init(accessKey: String, onWake: @escaping () -> Void) {
        self.onWake = onWake
        logger.info("WakeWord 占位实例化（task #6 未接 Porcupine）")
    }

    func start() { /* TODO task #6 */ }
    func stop() { /* TODO task #6 */ }
    func pause() { /* TODO task #6——Recognizer 抢麦时短暂停 */ }
    func resume() { /* TODO task #6 */ }
}
