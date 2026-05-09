import AppKit
import SwiftUI
import Speech
import OSLog

/// 玄女 macOS 入口——正经 GUI app，Dock 里有图标，关窗 = 退出。
///
/// 早期试过 menubar app (LSUIElement + .accessory + NSStatusItem)，用户找不到、关不掉，
/// 体验糟糕。换成正经 .regular GUI app：开窗口能看见，⌘Q 能退，所见即所得。
@main
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let logger = Logger(subsystem: "cn.qmledmq.fuxi.xuannv", category: "delegate")
    private var mainController: MainWindowController?

    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        app.run()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            self?.logger.info("speech auth status = \(status.rawValue, privacy: .public)")
        }

        AppState.shared.bootstrap()

        // 主窗口——开 App 即弹，关掉 = 退出
        mainController = MainWindowController(state: AppState.shared)
        mainController?.show()

        logger.notice("玄女 ready (regular GUI mode)")
    }

    /// Dock 图标点击时，如果窗口关了就重开——但我们关窗就 terminate，所以这条只在
    /// 用户 ⌘W （miniaturize）后点 Dock 时触发。
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows visible: Bool) -> Bool {
        if !visible {
            mainController?.show()
        }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return true
    }
}
