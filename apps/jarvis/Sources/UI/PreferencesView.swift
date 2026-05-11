import AVFoundation
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
        .onChange(of: draft) { old, new in
            new.save()
            state.settings = new
            state.fuxiClient?.updateSettings(new)
            state.hotkey?.install(combo: new.hotkey)
            state.reloadWake()
            // uiMode 变了让 AppDelegate swap panel
            if old.uiMode != new.uiMode {
                if let delegate = NSApp.delegate as? AppDelegate {
                    delegate.applyUIMode(new.uiMode)
                }
            }
        }
    }

    private var connectionTab: some View {
        Form {
            Picker("形态", selection: $draft.uiMode) {
                ForEach(Settings.UIMode.allCases) { m in
                    Text(m.label).tag(m)
                }
            }
            .pickerStyle(.radioGroup)

            Divider()

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
            Text("PWA 设置里生成 token，粘贴这里，App 自动重连。立绘需先在 apps/jarvis/Resources/Pet/poses/ 装 5 张 PNG。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    /// 试听用的临时 synthesizer + remote tts——不复用 AppState.* 避开抢占主线 TTS。
    @State private var auditionSynth = Synthesizer()
    @State private var auditionRemote = RemoteTTSProvider()
    @State private var auditionStatus: String = ""

    private var voiceTab: some View {
        Form {
            Picker("Provider", selection: $draft.ttsProvider) {
                ForEach(Settings.TTSProvider.allCases) { p in
                    Text(p.label).tag(p)
                }
            }
            .pickerStyle(.segmented)

            if draft.ttsProvider == .system {
                systemVoiceSection
            } else {
                remoteVoiceSection
            }

            HStack {
                Button("试听") { audition() }
                Button("停止") {
                    auditionSynth.stop()
                    auditionRemote.stop()
                    auditionStatus = ""
                }
                Spacer()
                if !auditionStatus.isEmpty {
                    Text(auditionStatus)
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    @ViewBuilder
    private var systemVoiceSection: some View {
        let voices = Synthesizer.availableZhCNVoices()
        Picker("音色", selection: $draft.ttsVoice) {
            Text("自动（最高质量）").tag("")
            ForEach(voices, id: \.identifier) { v in
                Text("\(v.name) · \(qualityLabel(v.quality))").tag(v.identifier)
            }
        }
        .pickerStyle(.menu)

        HStack {
            Text("语速")
            Slider(value: $draft.ttsRate, in: 0.4...0.7, step: 0.01)
            Text(String(format: "%.2f", draft.ttsRate))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 40, alignment: .trailing)
        }

        if voices.isEmpty {
            Text("⚠️ 没扫到任何 zh-CN 系统音色——去 系统设置 → 辅助功能 → 朗读 → 系统语音 下载普通话语音包。")
                .font(.caption).foregroundStyle(.orange)
        } else {
            Text("premium > enhanced > default。premium 音色最像真人，需在 系统设置 → 辅助功能 → 朗读 → 系统语音 单独下载（约 100MB）。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private var remoteVoiceSection: some View {
        TextField("远端 TTS URL", text: $draft.ttsRemoteURL)
            .textFieldStyle(.roundedBorder)
        Text("默认走家用部署 home 的 GPT-SoVITS（派蒙音色）。鉴权 token 复用 fuxi-im pair token——「连接」标签里填好的会被一起带上。语速由 server 模型自身决定，本地 slider 只对系统语音生效。")
            .font(.caption).foregroundStyle(.secondary)
        Text("失败自动降级回系统语音——耳朵不会因 home 抖一下就静音。")
            .font(.caption).foregroundStyle(.secondary)
    }

    private func audition() {
        let line = "玄女在位，可调度可点将。"
        auditionSynth.stop()
        auditionRemote.stop()
        switch draft.ttsProvider {
        case .system:
            let id = draft.ttsVoice.trimmingCharacters(in: .whitespaces)
            auditionStatus = ""
            auditionSynth.speak(line, voiceIdentifier: id.isEmpty ? nil : id, rate: Float(draft.ttsRate)) {}
        case .remote:
            auditionStatus = "请求中…"
            auditionRemote.speak(line, baseURL: draft.ttsRemoteURL, bearerToken: draft.pairToken) { ok in
                Task { @MainActor in
                    auditionStatus = ok ? "" : "远端失败，看 ttsRemoteURL / token / server 状态"
                }
            }
        }
    }

    private func qualityLabel(_ q: AVSpeechSynthesisVoiceQuality) -> String {
        switch q {
        case .premium: return "premium"
        case .enhanced: return "enhanced"
        default: return "default"
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
