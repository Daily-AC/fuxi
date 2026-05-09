import AppKit
import SwiftUI
import OSLog

/// 主窗口——正经 GUI window，关掉就退出 App。
/// 跟 OverlayWindowController（屏幕底部 Siri 风格悬浮窗）并存：主窗口是用户主动操作入口，
/// overlay 是被动唤醒时的 quick-glance 反馈。
@MainActor
final class MainWindowController: NSObject, NSWindowDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "mainwindow")
    private let window: NSWindow
    private weak var state: AppState?

    init(state: AppState) {
        self.state = state
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 480),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "玄女"
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.center()

        let host = NSHostingView(rootView: MainView(state: state))
        window.contentView = host

        self.window = window
        super.init()
        window.delegate = self
    }

    func show() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
    }

    /// 关闭主窗口 = 退出 App。menubar app 那种"后台跑"用户不喜欢，
    /// 个人工具就该所见即所得：开窗口=活，关窗口=死。
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        logger.notice("用户关主窗口 → terminate")
        NSApp.terminate(nil)
        return false
    }
}
