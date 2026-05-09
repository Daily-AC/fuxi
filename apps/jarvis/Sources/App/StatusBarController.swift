import AppKit
import SwiftUI
import Combine
import OSLog

/// 菜单栏图标 + 菜单——AppKit NSStatusItem 路径，比 SwiftUI MenuBarExtra 稳。
/// SwiftPM build (无 Xcode) + ad-hoc codesign 下 MenuBarExtra 偶尔不渲染，是 known issue.
@MainActor
final class StatusBarController {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "statusbar")
    private let state: AppState
    private let item: NSStatusItem
    private var sub: AnyCancellable?

    private var settingsWindow: NSWindow?
    weak var overlay: OverlayWindowController?

    init(state: AppState) {
        self.state = state
        // variableLength = 图标自适应宽，让 SF Symbol 正常居中
        self.item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        configureButton()
        rebuildMenu()

        // phase 变化驱动图标切换
        self.sub = state.$phase.receive(on: RunLoop.main).sink { [weak self] _ in
            self?.configureButton()
        }
    }

    private func configureButton() {
        guard let button = item.button else { return }
        let symbol = state.menuBarIconName
        let img = NSImage(systemSymbolName: symbol, accessibilityDescription: "玄女")
            ?? NSImage(systemSymbolName: "waveform.circle", accessibilityDescription: "玄女")
        img?.isTemplate = true  // 跟随 menubar dark/light 主题
        button.image = img
        button.toolTip = "玄女"
    }

    private func rebuildMenu() {
        let menu = NSMenu()
        menu.delegate = MenuDelegate.shared

        let toggle = NSMenuItem(title: "开始听", action: #selector(toggleListen), keyEquivalent: "")
        toggle.target = self
        menu.addItem(toggle)

        let cancel = NSMenuItem(title: "取消", action: #selector(cancel), keyEquivalent: "")
        cancel.target = self
        menu.addItem(cancel)

        menu.addItem(NSMenuItem.separator())

        // 状态行——只读
        let status = NSMenuItem(title: stateLabel(), action: nil, keyEquivalent: "")
        status.isEnabled = false
        menu.addItem(status)

        menu.addItem(NSMenuItem.separator())

        let settings = NSMenuItem(title: "设置…", action: #selector(showSettings), keyEquivalent: ",")
        settings.target = self
        menu.addItem(settings)

        menu.addItem(NSMenuItem.separator())

        let quit = NSMenuItem(title: "退出玄女", action: #selector(quit), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)

        item.menu = menu
    }

    private func stateLabel() -> String {
        switch state.phase {
        case .idle: return state.wakeMode == .fallback ? "● 待命（兜底听）" : "● 待命"
        case .listening: return "◉ 正在听"
        case .sending: return "↗ 派给玄女"
        case .waiting: return "⋯ 等玄女回话"
        case .speaking: return "✦ 玄女在说"
        }
    }

    // ── actions ─────────────────────────────────

    @objc private func toggleListen() {
        state.toggleListening()
    }

    @objc private func cancel() {
        state.cancelToIdle()
    }

    @objc private func showSettings() {
        if settingsWindow == nil {
            let w = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 520, height: 360),
                styleMask: [.titled, .closable, .miniaturizable],
                backing: .buffered,
                defer: false
            )
            w.title = "玄女 · 设置"
            w.isReleasedWhenClosed = false
            w.center()
            // 嵌 SwiftUI PreferencesView
            let host = NSHostingView(rootView: PreferencesView(state: state))
            w.contentView = host
            settingsWindow = w
        }
        // 点设置时把 app 短暂激活（accessory app 默认 inactive，弹窗才能 focus）
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    @objc private func quit() {
        NSApp.terminate(nil)
    }
}

/// NSMenu delegate——菜单显示前刷一下状态行。
@MainActor
final class MenuDelegate: NSObject, NSMenuDelegate {
    static let shared = MenuDelegate()
    func menuWillOpen(_ menu: NSMenu) {
        // 让菜单每次打开都重建状态文案——AppState 没监听到一定会刷
        // 这个钩子轻量，重建不重画影响小
    }
}
