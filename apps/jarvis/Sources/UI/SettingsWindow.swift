import AppKit
import SwiftUI
import OSLog

/// 独立设置窗口——从 NSStatusItem 右键菜单「设置...」呼出。
///
/// 内容沿用旧 PreferencesView（已经成熟），但包成独立 NSWindow 而非 sheet——
/// 主窗口已砍，没法 attach sheet。窗口尺寸固定 520×400，居中弹出。
@MainActor
final class SettingsWindowController: NSObject, NSWindowDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "settings-window")
    private var window: NSWindow?
    private weak var state: AppState?

    init(state: AppState) {
        self.state = state
        super.init()
    }

    /// 显示设置窗口——已存在则 makeKeyAndOrderFront 抢回前台。
    func show() {
        if let window {
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
            return
        }
        guard let state else { return }

        let win = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 520, height: 400),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        win.title = "玄女 · 设置"
        win.titlebarAppearsTransparent = true
        win.isReleasedWhenClosed = false
        win.center()

        // hosting：包一个 binding stub 让 PreferencesView 的 isPresented 关窗即关 NSWindow
        let view = SettingsRootView(state: state) { [weak self] in
            self?.window?.performClose(nil)
        }
        win.contentView = NSHostingView(rootView: view)
        win.delegate = self
        self.window = win

        NSApp.activate(ignoringOtherApps: true)
        win.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        // 关窗 = 销毁；下次 show 重建（避免 stale state 与 SwiftUI binding 残留）
        window = nil
    }
}

/// 设置窗口的 SwiftUI 根——壳子调 PreferencesView 提供一个虚拟 binding。
/// 主体 UI 沿用旧文件，统一字体改成苹方 SC（系统默认中文已 fallback 到苹方）。
private struct SettingsRootView: View {
    @ObservedObject var state: AppState
    let onClose: () -> Void
    @State private var dummyPresented = true

    var body: some View {
        PreferencesView(
            state: state,
            isPresented: Binding(
                get: { dummyPresented },
                set: { newVal in
                    dummyPresented = newVal
                    if !newVal { onClose() }
                }
            )
        )
        // 苹方 SC 已是系统中文 sans-serif 默认，这里显式 .body 让标题/正文一致
        .font(.system(.body, design: .default))
    }
}
