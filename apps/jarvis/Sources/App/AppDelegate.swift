import AppKit
import SwiftUI
import Speech
import OSLog

/// 玄女 macOS 入口——禅意药丸 GUI（NSPanel 悬浮 dock 上方）+ NSStatusItem 设置入口。
///
/// 演变：menubar app（早期，用户找不到）→ 正经 GUI 主窗口（中期，太重）→
/// 禅意药丸 + statusItem（当前，留白第一）。dock 仍可见但 app 自己无主窗口；
/// 设置走 statusItem 右键菜单弹独立窗口；cmd+Q 退出。
@main
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "delegate")
    private var capsulePanel: CapsulePanel?
    private var settingsController: SettingsWindowController?
    private var statusItem: NSStatusItem?

    static func main() {
        // HuggingFace 在国内直连不通——WhisperKit 内部用 swift-transformers 拉模型，
        // 它尊重 HF_ENDPOINT 环境变量。在 NSApplication.run 之前 setenv 让 lib 一启动
        // 就走镜像，否则 lookup metadata.json 直接超时（见日志 NSURLErrorDomain -1001）。
        // hf-mirror.com 是国内常用 HF 镜像，1:1 同步，公益运营。
        if getenv("HF_ENDPOINT") == nil {
            setenv("HF_ENDPOINT", "https://hf-mirror.com", 1)
        }
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        // .accessory：app 不进 dock 也不进 cmd-tab，但仍可有窗口（设置面板）。
        // 跟 LSUIElement YES 互相对齐——dock 上方留给胶囊本身，app 图标不占位。
        app.setActivationPolicy(.accessory)
        app.run()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            self?.logger.info("speech auth status = \(status.rawValue, privacy: .public)")
        }

        AppState.shared.bootstrap()

        // 禅意药丸——dock 正上方悬浮胶囊
        let panel = CapsulePanel(state: AppState.shared)
        panel.show()
        capsulePanel = panel

        // 设置入口——statusItem 右键菜单
        installStatusItem()

        // 设置窗口控制器（lazy show）
        settingsController = SettingsWindowController(state: AppState.shared)

        logger.notice("玄女 ready (zen capsule mode)")
    }

    /// 装菜单栏 NSStatusItem——左键 / 右键都弹同一个菜单（只有「设置」+「退出」两项，
    /// 不需要区分单击行为）。图标用 SF Symbol `waveform.circle`，单色 template 自适应明暗。
    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = item.button {
            let image = NSImage(
                systemSymbolName: "waveform.circle",
                accessibilityDescription: "玄女"
            )
            image?.isTemplate = true
            button.image = image
        }

        let menu = NSMenu()
        menu.delegate = self

        let settingsItem = NSMenuItem(
            title: "设置…",
            action: #selector(openSettings),
            keyEquivalent: ","
        )
        settingsItem.keyEquivalentModifierMask = [.command]
        settingsItem.target = self
        menu.addItem(settingsItem)

        menu.addItem(NSMenuItem.separator())

        let quitItem = NSMenuItem(
            title: "退出玄女",
            action: #selector(quitApp),
            keyEquivalent: "q"
        )
        quitItem.keyEquivalentModifierMask = [.command]
        quitItem.target = self
        menu.addItem(quitItem)

        item.menu = menu
        statusItem = item
    }

    @objc private func openSettings() {
        settingsController?.show()
    }

    @objc private func quitApp() {
        NSApp.terminate(nil)
    }

    /// `.accessory` 策略下 app 没主窗口，dock 点击重开走不到 reopen——
    /// 留这个钩子作 future-proof（用户改回 .regular 时点 dock 会复用胶囊）。
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows visible: Bool) -> Bool {
        capsulePanel?.show()
        return true
    }

    /// `.accessory` 下没主窗口可关——保留语义但不再触发退出。
    /// 退出走 statusItem 菜单 / cmd+Q（NSApp 默认接管）。
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false
    }
}
