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
            // SF Symbol `waveform.circle` 圆框波形——避开系统橙色 mic indicator 视觉
            // 重叠（菜单栏小图标也是 app 颜面，纯文字 label 在某些 macOS 14+ 下不渲染）。
            // 状态映射靠 fill / non-fill 切换给视觉提示，颜色由系统 dark/light 自适应。
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
