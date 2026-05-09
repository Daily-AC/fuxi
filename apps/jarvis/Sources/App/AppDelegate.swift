import AppKit
import Speech
import OSLog

/// NSApplicationDelegate——挂权限请求、启动状态机。SwiftUI MenuBarExtra 自己管图标，
/// 但权限请求 / 全局热键 / always-on audio 都是 NSApplication 层的事。
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.jarvis", category: "delegate")

    func applicationDidFinishLaunching(_ notification: Notification) {
        // 启动时主动请求语音识别权限——首次会弹系统对话框；用户拒了就只能纯文字模式。
        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            self?.logger.info("speech auth status = \(status.rawValue, privacy: .public)")
        }
        // 麦克风权限——AVCaptureDevice.requestAccess on first audio engine start。Recognizer.start
        // 内部会兜一次。
        Task { @MainActor in
            AppState.shared.bootstrap()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        // 菜单栏 app 不因关窗退出。
        return false
    }
}

