import AppKit
import OSLog

/// 全局热键监听——基于 `NSEvent.addGlobalMonitorForEvents`。
///
/// 限制：addGlobalMonitor 收到事件不能 consume（系统仍会派发到当前 app）。但对热键
/// 触发模式 + 配合，按下后启动听写就够。如果要"独占快捷键"，得上 Carbon `RegisterEventHotKey`
/// 或 CGEventTap，复杂度跳一档。先按全局 monitor 起步。
///
/// **必需权限**：辅助功能（Accessibility）—— 系统设置 → 隐私与安全性 → 辅助功能 →
/// 允许「贾维斯」。首次使用要让用户手动开。
final class HotkeyMonitor {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "hotkey")
    private var globalMonitor: Any?
    private var localMonitor: Any?
    private let onTrigger: () -> Void

    init(onTrigger: @escaping () -> Void) {
        self.onTrigger = onTrigger
    }

    deinit {
        uninstall()
    }

    func install(combo: HotkeyCombo) {
        uninstall()
        let flags = combo.nsFlags
        let keyCode = combo.keyCode
        // 全局——前台 app 不是自己也能收。
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return }
            if event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(flags),
               event.keyCode == keyCode {
                self.onTrigger()
            }
        }
        // 自己 app 在前台时 globalMonitor 不触发——加一份 local。
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            if event.modifierFlags.intersection(.deviceIndependentFlagsMask).contains(flags),
               event.keyCode == keyCode {
                self.onTrigger()
                return nil
            }
            return event
        }
    }

    func uninstall() {
        if let m = globalMonitor { NSEvent.removeMonitor(m); globalMonitor = nil }
        if let m = localMonitor { NSEvent.removeMonitor(m); localMonitor = nil }
    }
}
