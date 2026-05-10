import SwiftUI

/// 禅意药丸的内容视图——SwiftUI 渲染 5 状态。
///
/// 留白第一：所有元素只占中央 40%（ZenStyle.contentInsetRatio 控制）。
/// 元素本身极细线条 + 缓动 ease-in-out；色彩克制——主调淡墨青，朱砂只点睛。
///
/// 状态映射（AppState.phase + wakeMode）：
/// - idle      → 居中圆点呼吸
/// - listening → 18 根 1.5px 波形条，audioLevel 驱动
/// - sending   → 三点缓行
/// - waiting   → 三点缓行（同 sending）
/// - speaking  → 反相波形（外端向中央收）
/// - ack       → 由调用方在 listening 进入前以 200ms transient overlay 触发，
///               这里以 phase 切换瞬间 ZStack overlay 隐式呈现（详见 SweepOverlay）
struct CapsuleStateView: View {
    @ObservedObject var state: AppState
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        ZStack {
            // 宣纸底——8% 不透明叠在 NSVisualEffectView 之上
            ZenStyle.paper(scheme)
                .opacity(0.08)
                .clipShape(Capsule())

            // hairline 描边
            Capsule()
                .strokeBorder(ZenStyle.stroke(scheme), lineWidth: ZenStyle.strokeWidth)

            // 主体内容——switch on phase
            content
                .padding(.horizontal, ZenStyle.capsuleWidth * ZenStyle.contentInsetRatio)
        }
        .frame(width: ZenStyle.capsuleWidth, height: ZenStyle.capsuleHeight)
    }

    @ViewBuilder
    private var content: some View {
        switch state.phase {
        case .idle:
            IdleBreathDot(scheme: scheme)
        case .listening:
            WaveformView(level: state.audioLevel, mode: .listening, scheme: scheme)
        case .sending, .waiting:
            WaitingDots(scheme: scheme)
        case .speaking:
            WaveformView(level: 0.5, mode: .speaking, scheme: scheme)
        }
    }
}

// MARK: - idle: 呼吸圆点

/// idle 时居中 6px 淡墨青圆点，2.4s 柔光呼吸（α 0.4→1.0→0.4）。
private struct IdleBreathDot: View {
    let scheme: ColorScheme
    @State private var bright = false

    var body: some View {
        Circle()
            .fill(ZenStyle.inkTeal(scheme))
            .frame(width: ZenStyle.idleDotDiameter, height: ZenStyle.idleDotDiameter)
            .opacity(bright ? 1.0 : 0.4)
            .onAppear {
                withAnimation(
                    .easeInOut(duration: ZenStyle.breathePeriod / 2)
                    .repeatForever(autoreverses: true)
                ) {
                    bright = true
                }
            }
    }
}

// MARK: - listening / speaking: 波形

/// 18 根 1.5px 极细线水平铺中央，按 audioLevel 起伏。
/// listening: 中段高 + 高电平时尖端染朱砂。
/// speaking : 反相——外两端高，中央收（与 listening 视觉区分）。
private struct WaveformView: View {
    enum Mode { case listening, speaking }
    let level: Double
    let mode: Mode
    let scheme: ColorScheme

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            Canvas { ctx, size in
                let now = timeline.date.timeIntervalSinceReferenceDate
                let mid = size.height / 2
                let count = ZenStyle.waveBarCount
                let totalW = CGFloat(count) * ZenStyle.waveBarWidth
                    + CGFloat(count - 1) * ZenStyle.waveBarSpacing
                let startX = (size.width - totalW) / 2

                for i in 0..<count {
                    // 中心化坐标 -1...+1
                    let normalized = (Double(i) - Double(count - 1) / 2) / Double(count / 2)
                    // listening: 中央高（envelope = cos 中央=1 端点=0）
                    // speaking : 反相（envelope = sin 绝对值——中央=0 端点=1）
                    let envelope: Double
                    switch mode {
                    case .listening:
                        envelope = cos(normalized * .pi / 2)
                    case .speaking:
                        envelope = abs(sin(normalized * .pi / 2))
                    }
                    let phaseOffset = Double(i) * 0.18
                    let wob = 0.55 + 0.45 * sin(now * ZenStyle.waveBaseFrequency + phaseOffset)
                    let amp: Double
                    switch mode {
                    case .listening:
                        amp = max(0.15, level * wob)
                    case .speaking:
                        amp = 0.4 + 0.3 * sin(now * 4 + phaseOffset)
                    }

                    let h = max(ZenStyle.waveBarWidth, CGFloat(amp * envelope) * size.height)
                    let x = startX + CGFloat(i) * (ZenStyle.waveBarWidth + ZenStyle.waveBarSpacing)
                    let rect = CGRect(
                        x: x,
                        y: mid - h / 2,
                        width: ZenStyle.waveBarWidth,
                        height: h
                    )

                    // 高电平时尖端染朱砂——只 listening 模式生效，speaking 全用淡墨青
                    let color: Color
                    if mode == .listening, amp > 0.7 {
                        color = ZenStyle.cinnabar(scheme)
                    } else {
                        color = ZenStyle.inkTeal(scheme)
                    }
                    ctx.fill(
                        Path(roundedRect: rect, cornerRadius: ZenStyle.waveBarWidth / 2),
                        with: .color(color)
                    )
                }
            }
        }
    }
}

// MARK: - sending / waiting: 三点缓行

/// `· · ·` 三点缓行。1.6s 周期，每点错相位呼吸。淡墨青。
private struct WaitingDots: View {
    let scheme: ColorScheme
    @State private var step: Int = 0
    private let timer = Timer.publish(every: ZenStyle.waitingPeriod / 3, on: .main, in: .common).autoconnect()

    var body: some View {
        HStack(spacing: ZenStyle.waitingDotSpacing) {
            ForEach(0..<3, id: \.self) { i in
                Circle()
                    .fill(ZenStyle.inkTeal(scheme))
                    .frame(width: ZenStyle.waitingDotDiameter, height: ZenStyle.waitingDotDiameter)
                    .opacity(opacity(for: i))
                    .animation(.easeInOut(duration: ZenStyle.waitingPeriod / 3), value: step)
            }
        }
        .onReceive(timer) { _ in
            step = (step + 1) % 3
        }
    }

    private func opacity(for index: Int) -> Double {
        // 当前 step 的点最亮，其余两个半暗——形成「· · ·」缓行效果
        index == step ? 1.0 : 0.35
    }
}

// MARK: - ack: 横扫线（独立 overlay）

/// 一道墨笔横扫——200ms 内淡入淡出。
/// 由 CapsulePanel 在收到 ack 信号时叠加到胶囊上层显示，与 phase 视图独立。
struct SweepOverlay: View {
    let scheme: ColorScheme
    @State private var progress: Double = 0
    @State private var visible = false

    var body: some View {
        GeometryReader { geo in
            let lineHeight: CGFloat = 1.5
            let xPos = geo.size.width * progress
            Rectangle()
                .fill(ZenStyle.inkTeal(scheme))
                .frame(width: geo.size.width * 0.35, height: lineHeight)
                .position(x: xPos, y: geo.size.height / 2)
                .opacity(visible ? 1.0 : 0.0)
        }
        .onAppear {
            withAnimation(.easeIn(duration: ZenStyle.ackSweepDuration / 2)) {
                visible = true
            }
            withAnimation(.linear(duration: ZenStyle.ackSweepDuration)) {
                progress = 1.0
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + ZenStyle.ackSweepDuration / 2) {
                withAnimation(.easeOut(duration: ZenStyle.ackSweepDuration / 2)) {
                    visible = false
                }
            }
        }
    }
}
