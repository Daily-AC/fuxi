import SwiftUI

/// 衣袖飘动效果——三层 sin 叠加画一道淡墨气韵线。
///
/// 视觉语言：
/// - 主基调跟 ZenStyle.inkTeal 同色
/// - 三层 sin 振幅 / 频率 / 相位错开形成"自然"飘动
/// - amplitudeBoost 0~1，listening 时让 PetPoseView 透传 audioLevel 让它跟麦克风电平耦合
/// - 透明度 0.5，避免抢主体 pose
struct SleeveCanvasOverlay: View {
    let amplitudeBoost: Double  // 0~1，外部调制（idle 给 0，listening 给 audioLevel）
    let scheme: ColorScheme

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
            Canvas { ctx, size in
                let now = timeline.date.timeIntervalSinceReferenceDate
                let baseColor = ZenStyle.inkTeal(scheme).opacity(0.5)

                // 三层错开
                for layer in 0..<3 {
                    let layerPhase = Double(layer) * 1.7
                    let layerFreq = 0.5 + Double(layer) * 0.3
                    let layerAmp = (4.0 + Double(layer) * 2.0) * (1.0 + amplitudeBoost)
                    var path = Path()
                    let yMid = size.height * (0.65 + Double(layer) * 0.05)
                    let step: CGFloat = 4
                    var x: CGFloat = 0
                    path.move(to: CGPoint(x: 0, y: yMid))
                    while x < size.width {
                        let phase = Double(x) / 40.0 + now * layerFreq + layerPhase
                        let y = yMid + sin(phase) * layerAmp
                        path.addLine(to: CGPoint(x: x, y: y))
                        x += step
                    }
                    ctx.stroke(path, with: .color(baseColor), lineWidth: 0.8)
                }
            }
        }
        .allowsHitTesting(false)
    }
}
