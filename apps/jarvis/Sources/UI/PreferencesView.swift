import SwiftUI

/// 设置面板——SwiftUI Settings scene。`Settings` (struct) 是值类型，编辑后调用 save()
/// 落 UserDefaults + Keychain，并通知 fuxiClient 重连（base URL 变了 / token 变了都要换）。
struct PreferencesView: View {
    @ObservedObject var state: AppState
    @State private var draft: Settings

    init(state: AppState) {
        self.state = state
        _draft = State(initialValue: state.settings)
    }

    var body: some View {
        TabView {
            connectionTab
                .tabItem { Label("连接", systemImage: "network") }
            voiceTab
                .tabItem { Label("语音", systemImage: "waveform") }
            triggerTab
                .tabItem { Label("唤醒", systemImage: "mic.badge.plus") }
        }
        .frame(width: 480, height: 320)
        .padding(20)
        .onChange(of: draft) { _, new in
            // 实时存——简单粗暴；高频键入会有点 churn 但不影响。
            new.save()
            state.settings = new
            state.fuxiClient?.updateSettings(new)
            state.hotkey?.install(combo: new.hotkey)
        }
    }

    private var connectionTab: some View {
        Form {
            TextField("fuxi-im 地址", text: $draft.baseURL)
                .textFieldStyle(.roundedBorder)
            SecureField("Pair Token", text: $draft.pairToken)
                .textFieldStyle(.roundedBorder)
            HStack {
                Text("当前状态：")
                Text(state.connectionStatus)
                    .foregroundStyle(.secondary)
            }
            Text("配对流程：在 PWA 设置里生成 token，粘贴这里，重启 App。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var voiceTab: some View {
        Form {
            TextField("TTS Voice (留空=系统默认中文)", text: $draft.ttsVoice)
                .textFieldStyle(.roundedBorder)
            Text("更换音色：系统设置 → 辅助功能 → 朗读 → 系统语音 下载，然后填 identifier。")
                .font(.caption)
                .foregroundStyle(.secondary)
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
                SecureField("Picovoice Access Key", text: $draft.picovoiceKey)
                    .textFieldStyle(.roundedBorder)
                Text("唤醒词「玄女」需要 Picovoice access key（个人 plan 免费）+ 自定义训练的 .ppn 模型。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Text("默认热键：⌥⇧Space（暂不支持自定义，下版本加）。需要给「贾维斯」开启「辅助功能」权限：系统设置 → 隐私与安全性 → 辅助功能。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}
