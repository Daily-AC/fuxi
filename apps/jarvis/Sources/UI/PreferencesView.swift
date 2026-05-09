import SwiftUI

/// 设置面板——sheet 形式，自带「完成」按钮关闭（macOS sheet 默认无 close button，
/// 必须显式提供）。draft 实时持久化到 UserDefaults + 调 reloadWake / FuxiClient.
struct PreferencesView: View {
    @ObservedObject var state: AppState
    @Binding var isPresented: Bool
    @State private var draft: Settings

    init(state: AppState, isPresented: Binding<Bool>) {
        self.state = state
        self._isPresented = isPresented
        _draft = State(initialValue: state.settings)
    }

    var body: some View {
        VStack(spacing: 0) {
            // 顶 bar：标题 + 完成按钮
            HStack {
                Text("玄女 · 设置")
                    .font(.title3.bold())
                Spacer()
                Button("完成") { isPresented = false }
                    .keyboardShortcut(.return, modifiers: [.command])
                    .keyboardShortcut(.escape, modifiers: [])
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)
            .background(.ultraThinMaterial)
            Divider()

            // 主体 tabs
            TabView {
                connectionTab
                    .tabItem { Label("连接", systemImage: "network") }
                voiceTab
                    .tabItem { Label("语音", systemImage: "waveform") }
                triggerTab
                    .tabItem { Label("唤醒", systemImage: "mic.badge.plus") }
            }
            .padding(20)
        }
        .frame(width: 520, height: 400)
        .onChange(of: draft) { _, new in
            new.save()
            state.settings = new
            state.fuxiClient?.updateSettings(new)
            state.hotkey?.install(combo: new.hotkey)
            state.reloadWake()
        }
    }

    private var connectionTab: some View {
        Form {
            TextField("fuxi-im 地址", text: $draft.baseURL)
                .textFieldStyle(.roundedBorder)
            // pair token 用 TextField 不用 SecureField——SecureField 会触发系统密码
            // 自动建议弹窗，干扰用户粘贴。个人工具 token 明文展示无碍。
            TextField("Pair Token", text: $draft.pairToken)
                .textFieldStyle(.roundedBorder)
            HStack {
                Text("当前状态：")
                Text(state.connectionStatus).foregroundStyle(.secondary)
            }
            Text("PWA 设置里生成 token，粘贴这里，App 自动重连。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var voiceTab: some View {
        Form {
            TextField("TTS Voice (留空=系统默认中文)", text: $draft.ttsVoice)
                .textFieldStyle(.roundedBorder)
            Text("更换音色：系统设置 → 辅助功能 → 朗读 → 系统语音 下载，然后填 identifier。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var triggerTab: some View {
        Form {
            Picker("触发方式", selection: $draft.triggerMode) {
                ForEach(Settings.TriggerMode.allCases) { mode in
                    Text(mode.label).tag(mode)
                }
            }
            .pickerStyle(.radioGroup)

            if draft.triggerMode != .hotkey {
                TextField("Wake Server URL", text: $draft.wakeServerURL)
                    .textFieldStyle(.roundedBorder)
                TextField("Wake Token", text: $draft.wakeToken)
                    .textFieldStyle(.roundedBorder)
                Text("唤醒词「玄女」走 home 端 fuxi-wake-server。Token 跟 fuxi-im pair 同一颗。")
                    .font(.caption).foregroundStyle(.secondary)
            }

            Text("默认热键 ⌃⌥M。需要在 系统设置 → 隐私与安全性 → 辅助功能 里允许「玄女」。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}
