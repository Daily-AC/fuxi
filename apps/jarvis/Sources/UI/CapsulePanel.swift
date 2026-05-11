import AppKit
import SwiftUI
import OSLog

/// 禅意药丸悬浮窗——dock 正上方 ~160×32 圆角胶囊。
///
/// 关键约束：
/// - `.borderless + .nonactivatingPanel` —— 不抢焦点，不进 cmd-tab
/// - `.floating` level —— 浮在 dock 上方但不压住菜单栏
/// - `collectionBehavior = [.canJoinAllSpaces, .stationary]` —— 切 Space 跟随
/// - 监听 `didChangeScreenParametersNotification` —— 屏幕分辨率 / dock 位置变 → 重新定位
@MainActor
final class CapsulePanel: NSPanel {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "capsule")
    private let state: AppState
    private var hostingView: NSHostingView<CapsuleStateView>?

    init(state: AppState) {
        self.state = state
        // 初始化先按设计尺寸开窗，位置 0,0；show() 时再 reposition
        let rect = NSRect(
            x: 0,
            y: 0,
            width: ZenStyle.capsuleWidth,
            height: ZenStyle.capsuleHeight
        )
        super.init(
            contentRect: rect,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        // 不抢焦点：用户点胶囊不会激活 App，仍可继续在前台 app 操作
        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = true
        hidesOnDeactivate = false
        // 浮在最上层（dock + 普通窗口之上，但不盖住菜单栏 / 全屏 app）
        level = .floating
        // Space 切换时跟随，Mission Control 里也保持位置
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        // 透明背景——视觉效果靠 SwiftUI Capsule + NSVisualEffectView 共同呈现
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        // 不在 cmd-tab 出现
        isExcludedFromWindowsMenu = true

        // hosting view：SwiftUI 渲染胶囊
        let host = NSHostingView(rootView: CapsuleStateView(state: state))
        host.frame = rect

        // 视觉效果层——`.menu` material 接近 macOS 原生 control bar 质感
        let visual = NSVisualEffectView(frame: rect)
        visual.material = .menu
        visual.blendingMode = .behindWindow
        visual.state = .active
        visual.wantsLayer = true
        visual.layer?.cornerRadius = ZenStyle.capsuleCornerRadius
        visual.layer?.masksToBounds = true
        visual.autoresizingMask = [.width, .height]

        let container = NSView(frame: rect)
        container.addSubview(visual)
        host.frame = visual.bounds
        host.autoresizingMask = [.width, .height]
        visual.addSubview(host)

        contentView = container
        self.hostingView = host

        // 屏幕参数变化（外接显示器插拔 / 分辨率切换 / dock 位置改）→ 重定位
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(handleScreenChange),
            name: NSApplication.didChangeScreenParametersNotification,
            object: nil
        )
        // Space 切换——胶囊已 canJoinAllSpaces，但不同 Space 上 visibleFrame 可能不同
        NSWorkspace.shared.notificationCenter.addObserver(
            self,
            selector: #selector(handleScreenChange),
            name: NSWorkspace.activeSpaceDidChangeNotification,
            object: nil
        )
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
    }

    /// NSPanel 子类必须 override 这两个否则 borderless 不能成为 key/main。
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// 右键药丸 → 弹「设置 / 退出」菜单。NSStatusItem 在 menubar 太挤被 macOS 隐藏时
    /// 用户找不到设置 / 关不掉 app（pkill 才能退）；右键药丸是兜底入口。
    /// `nonactivatingPanel` 下 mouseDown 事件仍正常派发到 NSPanel——直接 override。
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

        let quitItem = NSMenuItem(
            title: "退出玄女",
            action: #selector(AppDelegate.quitApp),
            keyEquivalent: "q"
        )
        quitItem.keyEquivalentModifierMask = [.command]
        quitItem.target = NSApp.delegate
        menu.addItem(quitItem)

        // 在鼠标位置弹——nil view 让 NSMenu 走全局坐标。
        NSMenu.popUpContextMenu(menu, with: event, for: contentView ?? NSView())
    }

    /// 显示胶囊——计算位置后 orderFront（不 makeKey 避免抢焦点）。
    func show() {
        repositionAboveDock()
        orderFront(nil)
        logger.notice("capsule shown at \(self.frame.debugDescription, privacy: .public)")
    }

    @objc private func handleScreenChange() {
        repositionAboveDock()
    }

    /// 算出 dock 上方居中位置：
    /// - 主屏 `visibleFrame` —— 已扣掉 dock 的可用区域
    /// - 胶囊水平居中
    /// - 垂直放在 visibleFrame 底部 + dockGap
    private func repositionAboveDock() {
        guard let screen = NSScreen.main else {
            logger.warning("无主屏，跳过 reposition")
            return
        }
        let visible = screen.visibleFrame
        let x = visible.midX - ZenStyle.capsuleWidth / 2
        let y = visible.minY + ZenStyle.dockGap
        let origin = NSPoint(x: x, y: y)
        setFrameOrigin(origin)
    }
}
