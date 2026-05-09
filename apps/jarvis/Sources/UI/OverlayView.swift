import SwiftUI

/// Siri 风格悬浮窗内容——屏幕底部中央，圆胶囊容器 + 波形条带 + 实时转写 / 玄女回复。
///
/// 状态机映射：
///   listening → 波形随 audioLevel 抖 + 显示用户实时转写
///   sending/waiting → 波形定格 + spinner + "派给玄女..."
///   speaking → 波形 + 玄女回复 + 播 TTS
///   idle → OverlayWindowController 直接 hide 窗口（这个 view 不见）
struct OverlayView: View {
    @ObservedObject var state: AppState

    var body: some View {
        VStack(spacing: 14) {
            // 主对话内容——根据 phase 切换
            switch state.phase {
            case .listening:
                Text(state.lastTranscript.isEmpty ? "在听…" : state.lastTranscript)
                    .font(.title3)
                    .foregroundStyle(.primary)
                    .lineLimit(3)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 520)
            case .sending, .waiting:
                HStack(spacing: 10) {
                    ProgressView().scaleEffect(0.7).tint(.secondary)
                    Text(state.phase == .sending ? "派给玄女…" : "等玄女回话…")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            case .speaking:
                Text(state.lastVoiceLine.isEmpty ? "（玄女正在说…）" : state.lastVoiceLine)
                    .font(.title3)
                    .foregroundStyle(.primary)
                    .lineLimit(3)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 520)
            case .idle:
                EmptyView()
            }

            // 波形条带——audioLevel 驱动 animated bars
            WaveformBars(level: state.audioLevel, phase: state.phase)
                .frame(height: 36)
                .frame(maxWidth: 280)
        }
        .padding(.horizontal, 28)
        .padding(.vertical, 22)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 24))
        .overlay(
            RoundedRectangle(cornerRadius: 24)
                .stroke(.white.opacity(0.08), lineWidth: 0.5)
        )
        .shadow(color: .black.opacity(0.25), radius: 16, y: 6)
        .frame(minWidth: 280, idealWidth: 480, maxWidth: 600)
        .fixedSize(horizontal: false, vertical: true)
    }
}

/// 波形——一组中央对称的胶囊条，每条的高度由 audioLevel + 自身相位决定。
/// listening 时随 mic 实时抖动；speaking 时沿一个稳定的"呼吸节奏"上下；其余 phase 保持静态低位。
struct WaveformBars: View {
    let level: Double  // 0~1
    let phase: AppState.VoicePhase

    @State private var t: Double = 0
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
                    // 中央条带高，两侧低——余弦包络
                    let center = (Double(i) - Double(barCount - 1) / 2) / Double(barCount / 2)
                    let envelope = cos(center * .pi / 2)  // 0~1

                    // 相位：每条偏移让波纹"流动"
                    let phaseOffset = Double(i) * 0.18

                    // 振幅依 phase 不同
                    let amp: Double
                    switch self.phase {
                    case .listening:
                        // 用户麦克风实时电平驱动
                        let wob = 0.6 + 0.4 * sin(now * 8 + phaseOffset)
                        amp = max(0.12, level * wob)
                    case .speaking:
                        // 玄女在说——稳定呼吸节奏
                        amp = 0.35 + 0.35 * sin(now * 4 + phaseOffset)
                    case .sending, .waiting:
                        // 等待时小幅余波
                        amp = 0.12 + 0.08 * sin(now * 2 + phaseOffset)
                    case .idle:
                        amp = 0.08
                    }

                    let h = max(barWidth, CGFloat(amp * envelope) * size.height)
                    let x = startX + CGFloat(i) * (barWidth + spacing)
                    let rect = CGRect(x: x, y: mid - h / 2, width: barWidth, height: h)
                    let color = barColor(for: self.phase)
                    ctx.fill(Path(roundedRect: rect, cornerRadius: barWidth / 2), with: .color(color))
                }
            }
        }
    }

    private func barColor(for phase: AppState.VoicePhase) -> Color {
        switch phase {
        case .listening: return .blue
        case .sending, .waiting: return .yellow
        case .speaking: return .purple
        case .idle: return .gray
        }
    }
}
