import AppKit
import SwiftUI
import Speech
import OSLog

/// 玄女 macOS 入口——纯 AppKit 路径，SwiftUI 只用作内容渲染（NSHostingView 嵌入）。
///
/// 为啥不用 SwiftUI App scene：SwiftPM build (无 Xcode) + ad-hoc codesign 下
/// MenuBarExtra / Settings scene 渲染不稳是 known issue。AppKit NSStatusItem +
/// NSWindow 老 API 最兼容。
@main
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "delegate")
    private var statusBar: StatusBarController?

    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        // accessory = 菜单栏 app，不显 Dock 图标，不参与 ⌘Tab。Info.plist 的
        // LSUIElement=YES 配合让 launch 阶段就这样。
        app.setActivationPolicy(.accessory)
        app.run()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // 启动时主动请求语音识别权限——首次会弹系统对话框；用户拒了就只能纯文字模式。
        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            self?.logger.info("speech auth status = \(status.rawValue, privacy: .public)")
        }

        // bootstrap AppState —— 它内部会启动 overlay、wake client、recognizer 等。
        AppState.shared.bootstrap()

        // 菜单栏图标——AppKit NSStatusItem，比 SwiftUI MenuBarExtra 稳。
        statusBar = StatusBarController(state: AppState.shared)

        logger.notice("玄女 ready")
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false
    }
}
