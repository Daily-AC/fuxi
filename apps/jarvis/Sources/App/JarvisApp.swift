import SwiftUI

/// 贾维斯（Jarvis）—— 玄女的语音壳子。菜单栏 app，无 Dock 图标（LSUIElement=YES）。
///
/// 链路：热键/唤醒词 → SFSpeechRecognizer 听写 → POST /api/intervene → WS /api/conv 监听
/// `XuannvVoiceLine` 事件 → AVSpeechSynthesizer 念。
@main
struct JarvisApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @StateObject private var state = AppState.shared

    var body: some Scene {
        MenuBarExtra {
            MenuBarPanel(state: state)
        } label: {
            // 菜单栏图标随状态变色——idle=灰、listening=蓝、speaking=紫。
            Image(systemName: state.menuBarIconName)
        }
        .menuBarExtraStyle(.window)

        // 显式 SwiftUI.Settings——本项目里有 struct Settings 同名，trailing-closure 推断
        // 在更多字段出现后会优先匹配我们自己的 struct，导致编译错。
        SwiftUI.Settings {
            PreferencesView(state: state)
        }
    }
}
