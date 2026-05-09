import SwiftUI

/// 菜单栏图标点开后的小面板——状态显示 + 快速操作。
struct MenuBarPanel: View {
    @ObservedObject var state: AppState

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Circle()
                    .fill(stateColor)
                    .frame(width: 8, height: 8)
                Text(stateLabel)
                    .font(.headline)
                Spacer()
                Text(state.connectionStatus)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if !state.lastTranscript.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text("我说").font(.caption2).foregroundStyle(.secondary)
                    Text(state.lastTranscript).font(.callout)
                }
            }

            if !state.lastVoiceLine.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    Text("玄女说").font(.caption2).foregroundStyle(.secondary)
                    Text(state.lastVoiceLine).font(.callout)
                }
            }

            Divider()

            HStack(spacing: 8) {
                Button(state.phase == .listening ? "停止" : "开始听") {
                    state.toggleListening()
                }
                .keyboardShortcut(.defaultAction)
                Button("取消") { state.cancelToIdle() }
                Spacer()
                Button("设置…") {
                    NSApplication.shared.activate(ignoringOtherApps: true)
                    if #available(macOS 14, *) {
                        NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
                    } else {
                        NSApp.sendAction(Selector(("showPreferencesWindow:")), to: nil, from: nil)
                    }
                }
                Button("退出") { NSApp.terminate(nil) }
            }
        }
        .padding(16)
        .frame(width: 320)
    }

    private var stateColor: Color {
        switch state.phase {
        case .idle: return .gray
        case .listening: return .blue
        case .sending, .waiting: return .yellow
        case .speaking: return .purple
        }
    }

    private var stateLabel: String {
        switch state.phase {
        case .idle: return "待命"
        case .listening: return "正在听"
        case .sending: return "派给玄女…"
        case .waiting: return "等玄女回话"
        case .speaking: return "玄女在说"
        }
    }
}
