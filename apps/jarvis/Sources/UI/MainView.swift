import SwiftUI

/// 主窗口内容——所有跟用户交互的入口都在这。
/// 顶部状态徽章 + 中间大波形 + 转写/回复 + 底部操作按钮。
struct MainView: View {
    @ObservedObject var state: AppState
    @State private var showSettings = false

    var body: some View {
        VStack(spacing: 0) {
            header
                .padding(.top, 28)
                .padding(.horizontal, 32)

            Spacer(minLength: 18)

            WaveformBars(level: state.audioLevel, phase: state.phase)
                .frame(height: 80)
                .padding(.horizontal, 40)

            Spacer(minLength: 18)

            transcriptView
                .padding(.horizontal, 32)

            Spacer()

            controls
                .padding(.horizontal, 32)
                .padding(.bottom, 28)
        }
        .frame(minWidth: 480, idealWidth: 560, minHeight: 420, idealHeight: 480)
        .background(.ultraThinMaterial)
        .sheet(isPresented: $showSettings) {
            PreferencesView(state: state)
                .frame(width: 520, height: 360)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                Image(systemName: state.menuBarIconName)
                    .font(.system(size: 28, weight: .light))
                    .foregroundStyle(badgeColor)
                    .frame(width: 36, height: 36)
                    .background(badgeColor.opacity(0.12), in: Circle())
                VStack(alignment: .leading, spacing: 2) {
                    Text("玄女")
                        .font(.title2.bold())
                    Text(stateLabel)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                connectionBadge
            }
        }
    }

    private var connectionBadge: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(wakeColor)
                .frame(width: 8, height: 8)
            Text(wakeLabel)
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
    }

    private var transcriptView: some View {
        VStack(alignment: .leading, spacing: 10) {
            if !state.lastTranscript.isEmpty {
                bubble(label: "我说", text: state.lastTranscript, color: .blue)
            }
            if !state.lastVoiceLine.isEmpty {
                bubble(label: "玄女", text: state.lastVoiceLine, color: .purple)
            }
            if state.lastTranscript.isEmpty && state.lastVoiceLine.isEmpty {
                Text(emptyHint)
                    .font(.callout)
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .center)
            }
        }
        .frame(maxHeight: 160)
    }

    private func bubble(label: String, text: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label).font(.caption2).foregroundStyle(color.opacity(0.85))
            Text(text)
                .font(.callout)
                .foregroundStyle(.primary)
                .lineLimit(4)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(color.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
    }

    private var controls: some View {
        HStack(spacing: 12) {
            Button(action: { state.toggleListening() }) {
                Label(state.phase == .listening ? "停" : "开始听",
                      systemImage: state.phase == .listening ? "stop.fill" : "mic.fill")
                    .frame(maxWidth: .infinity, minHeight: 36)
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.return, modifiers: [])

            Button(action: { state.cancelToIdle() }) {
                Label("取消", systemImage: "xmark.circle")
                    .frame(maxWidth: .infinity, minHeight: 36)
            }
            .buttonStyle(.bordered)
            .keyboardShortcut(.escape, modifiers: [])

            Button(action: { showSettings = true }) {
                Label("设置", systemImage: "gearshape")
                    .frame(minHeight: 36)
            }
            .buttonStyle(.bordered)
            .keyboardShortcut(",", modifiers: .command)
        }
    }

    private var stateLabel: String {
        switch state.phase {
        case .idle: return state.wakeMode == .fallback ? "本机兜底监听中" : "待命中"
        case .listening: return "正在听你说"
        case .sending: return "派给玄女…"
        case .waiting: return "等玄女回话…"
        case .speaking: return "玄女在说"
        }
    }

    private var emptyHint: String {
        switch state.phase {
        case .idle: return "说「玄女」唤醒，或 ⌃⌥M 开始"
        case .listening: return "在听，开口讲就行"
        default: return ""
        }
    }

    private var badgeColor: Color {
        switch state.phase {
        case .idle: return .gray
        case .listening: return .blue
        case .sending, .waiting: return .yellow
        case .speaking: return .purple
        }
    }

    private var wakeColor: Color {
        switch state.wakeMode {
        case .remote: return .green
        case .fallback: return .orange
        case .disabled: return .gray
        }
    }

    private var wakeLabel: String {
        switch state.wakeMode {
        case .remote: return "远端唤醒"
        case .fallback: return "本机兜底"
        case .disabled: return "唤醒关"
        }
    }
}

/// Siri 风格 28 条胶囊波形条带，TimelineView 30fps 实时绘制。
/// listening 时 audioLevel 驱动振幅；speaking 时稳定呼吸节奏；其他低位余波。
struct WaveformBars: View {
    let level: Double
    let phase: AppState.VoicePhase

    private let barCount = 28
    private let barWidth: CGFloat = 4
    private let spacing: CGFloat = 4

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            Canvas { ctx, size in
                let now = timeline.date.timeIntervalSinceReferenceDate
                let mid = size.height / 2
                let totalW = CGFloat(barCount) * barWidth + CGFloat(barCount - 1) * spacing
                let startX = (size.width - totalW) / 2

                for i in 0..<barCount {
                    let center = (Double(i) - Double(barCount - 1) / 2) / Double(barCount / 2)
                    let envelope = cos(center * .pi / 2)
                    let phaseOffset = Double(i) * 0.18

                    let amp: Double
                    switch self.phase {
                    case .listening:
                        let wob = 0.6 + 0.4 * sin(now * 8 + phaseOffset)
                        amp = max(0.12, level * wob)
                    case .speaking:
                        amp = 0.35 + 0.35 * sin(now * 4 + phaseOffset)
                    case .sending, .waiting:
                        amp = 0.12 + 0.08 * sin(now * 2 + phaseOffset)
                    case .idle:
                        amp = 0.08
                    }

                    let h = max(barWidth, CGFloat(amp * envelope) * size.height)
                    let x = startX + CGFloat(i) * (barWidth + spacing)
                    let rect = CGRect(x: x, y: mid - h / 2, width: barWidth, height: h)
                    ctx.fill(Path(roundedRect: rect, cornerRadius: barWidth / 2),
                             with: .color(barColor(for: self.phase)))
                }
            }
        }
    }

    private func barColor(for phase: AppState.VoicePhase) -> Color {
        switch phase {
        case .listening: return .blue
        case .sending, .waiting: return .yellow
        case .speaking: return .purple
        case .idle: return .gray.opacity(0.5)
        }
    }
}
