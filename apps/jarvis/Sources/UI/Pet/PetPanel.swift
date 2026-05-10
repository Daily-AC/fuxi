import AppKit
import SwiftUI
import OSLog

/// 立绘桌宠悬浮窗——~280×420 NSPanel，accessory 模式。
///
/// 关键约束：
/// - `.borderless + .nonactivatingPanel` —— 不抢焦点
/// - `.floating` level —— 浮在 dock 上方
/// - `collectionBehavior = [.canJoinAllSpaces, .stationary]`
/// - 可拖动：CGPoint 写 UserDefaults，下次启动恢复
/// - 屏幕分辨率变 / Space 切换 → 检测当前位置是否还在屏内，否则重置
@MainActor
final class PetPanel: NSPanel {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "pet-panel")
    private let state: AppState
    private var hostingView: NSHostingView<PetPoseView>?

    static let panelWidth: CGFloat = 280
    static let panelHeight: CGFloat = 420
    static let dockGap: CGFloat = 12
    static let positionKey = "cn.qmledmq.fuxi.xuannv.petPanel.origin"

    init(state: AppState) {
        self.state = state
        let rect = NSRect(x: 0, y: 0, width: Self.panelWidth, height: Self.panelHeight)
        super.init(
            contentRect: rect,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = true
        hidesOnDeactivate = false
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        isOpaque = false
        backgroundColor = .clear
        // 立绘自带 alpha，再加阴影会出怪框
        hasShadow = false
        isExcludedFromWindowsMenu = true
        isMovable = true
        // 拖任意位置都可移
        isMovableByWindowBackground = true

        let host = NSHostingView(rootView: PetPoseView(state: state))
        host.frame = rect
        host.autoresizingMask = [.width, .height]
        contentView = host
        hostingView = host

        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleScreenChange),
            name: NSApplication.didChangeScreenParametersNotification,
            object: nil
        )
        NSWorkspace.shared.notificationCenter.addObserver(
            self,
            selector: #selector(handleScreenChange),
            name: NSWorkspace.activeSpaceDidChangeNotification,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleDidMove),
            name: NSWindow.didMoveNotification,
            object: self
        )
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// 右键弹「设置 / 切回药丸 / 退出」—— PetPanel 复用 CapsulePanel 的菜单语义
    override func rightMouseDown(with event: NSEvent) {
        let menu = NSMenu()

        let settingsItem = NSMenuItem(
            title: "设置…",
            action: #selector(AppDelegate.openSettings),
            keyEquivalent: ","
        )
        settingsItem.keyEquivalentModifierMask = [.command]
        settingsItem.target = NSApp.delegate
        menu.addItem(settingsItem)

        menu.addItem(NSMenuItem.separator())

        let switchItem = NSMenuItem(
            title: "切回药丸",
            action: #selector(AppDelegate.switchToCapsule),
            keyEquivalent: ""
        )
        switchItem.target = NSApp.delegate
        menu.addItem(switchItem)

        menu.addItem(NSMenuItem.separator())

        let quitItem = NSMenuItem(
            title: "退出玄女",
            action: #selector(AppDelegate.quitApp),
            keyEquivalent: "q"
        )
        quitItem.keyEquivalentModifierMask = [.command]
        quitItem.target = NSApp.delegate
        menu.addItem(quitItem)

        NSMenu.popUpContextMenu(menu, with: event, for: contentView ?? NSView())
    }

    /// show—— restore saved CGPoint，否则 dock 上方居中
    func show() {
        if let saved = loadSavedOrigin(), originIsOnScreen(saved) {
            setFrameOrigin(saved)
        } else {
            repositionAboveDock()
        }
        orderFront(nil)
        logger.notice("pet panel shown at \(self.frame.debugDescription, privacy: .public)")
    }

    @objc private func handleScreenChange() {
        // 屏幕变了——若当前位置不在任何屏 visibleFrame 内，重置到 dock 上方
        if !originIsOnScreen(frame.origin) {
            repositionAboveDock()
        }
    }

    @objc private func handleDidMove(_ note: Notification) {
        // 用户拖完——存 UserDefaults
        let p = frame.origin
        let dict: [String: Double] = ["x": Double(p.x), "y": Double(p.y)]
        UserDefaults.standard.set(dict, forKey: Self.positionKey)
    }

    private func loadSavedOrigin() -> NSPoint? {
        guard let dict = UserDefaults.standard.dictionary(forKey: Self.positionKey),
              let x = dict["x"] as? Double,
              let y = dict["y"] as? Double
        else { return nil }
        return NSPoint(x: x, y: y)
    }

    /// 检测 origin 是否落在任意 NSScreen 的 visibleFrame 内（左下角点）
    private func originIsOnScreen(_ origin: NSPoint) -> Bool {
        for screen in NSScreen.screens {
            if screen.visibleFrame.contains(origin) {
                return true
            }
        }
        return false
    }

    private func repositionAboveDock() {
        guard let screen = NSScreen.main else {
            logger.warning("无主屏，跳过 reposition")
            return
        }
        let visible = screen.visibleFrame
        let x = visible.midX - Self.panelWidth / 2
        let y = visible.minY + Self.dockGap
        setFrameOrigin(NSPoint(x: x, y: y))
    }
}
