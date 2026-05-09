import AppKit
import SwiftUI
import Combine

/// 屏幕底部中央的悬浮窗——非常驻，只在 phase != idle 时出现。
///
/// NSWindow 配置：
///   - styleMask=.borderless：无标题栏，纯内容
///   - level=.floating：浮在所有窗口之上（不抢焦点）
///   - isOpaque=false + backgroundColor=clear：让 SwiftUI 的 .ultraThinMaterial 渗透
///   - collectionBehavior=.canJoinAllSpaces + .stationary：跨 Space 可见，不参与切换
///   - ignoresMouseEvents：默认 true（窗口不挡用户操作）；speaking/listening 时给 hover 关闭
final class OverlayWindowController {
    private let window: NSPanel
    private var sub: AnyCancellable?
    private weak var state: AppState?

    @MainActor
    init(state: AppState) {
        self.state = state

        let w: CGFloat = 520
        let h: CGFloat = 140
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: w, height: h),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .floating
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isMovableByWindowBackground = false
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        panel.hidesOnDeactivate = false
        // 默认透传鼠标——不挡用户操作；speaking/listening 时也保持透传，按 ESC 用全局热键取消
        panel.ignoresMouseEvents = true

        // SwiftUI 内容
        let host = NSHostingView(rootView: OverlayView(state: state))
        host.translatesAutoresizingMaskIntoConstraints = false
        let container = NSView(frame: panel.contentLayoutRect)
        container.addSubview(host)
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            host.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            host.topAnchor.constraint(equalTo: container.topAnchor),
            host.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
        panel.contentView = container

        self.window = panel

        // 订阅 phase 变化驱动 show/hide + 重定位
        self.sub = state.$phase
            .receive(on: RunLoop.main)
            .sink { [weak self] phase in
                self?.update(for: phase)
            }
    }

    @MainActor
    private func update(for phase: AppState.VoicePhase) {
        switch phase {
        case .idle:
            hide()
        default:
            show()
        }
    }

    @MainActor
    private func show() {
        guard let screen = NSScreen.main else { return }
        let frame = screen.frame
        let visibleFrame = screen.visibleFrame
        // 屏幕底部中央 + 距底 80px。frame 用 idealWidth；高度自适应内容（fixedSize 在 SwiftUI 端）
        let panelW: CGFloat = 520
        let panelH: CGFloat = 160
        let x = frame.minX + (frame.width - panelW) / 2
        let y = visibleFrame.minY + 80
        window.setFrame(NSRect(x: x, y: y, width: panelW, height: panelH), display: false)
        window.orderFront(nil)
        // 淡入动画（SwiftUI 内容自身没 transition，简单 alpha tween）
        window.alphaValue = 0
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.18
            window.animator().alphaValue = 1
        }
    }

    @MainActor
    private func hide() {
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.16
            window.animator().alphaValue = 0
        } completionHandler: { [weak self] in
            self?.window.orderOut(nil)
        }
    }
}
