import SwiftUI

/// 玄女 —— 用户的语音壳子，与伏羲平台后端的玄女 agent 同名同魂。
/// 菜单栏 app，无 Dock 图标（LSUIElement=YES）。
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
            // 用「玄」字做 menubar label——避开系统 macOS 麦克风 indicator（橙色 mic）
            // 视觉重叠的歧义。SF Symbol 渲染中文需 Text；颜色随状态变化由 SwiftUI
            // 默认渲染（系统 dark/light 自适应）。
            Text(state.menuBarLabel)
        }
        .menuBarExtraStyle(.window)

        // 显式 SwiftUI.Settings——本项目里有 struct Settings 同名，trailing-closure 推断
        // 在更多字段出现后会优先匹配我们自己的 struct，导致编译错。
        SwiftUI.Settings {
            PreferencesView(state: state)
        }
    }
}
