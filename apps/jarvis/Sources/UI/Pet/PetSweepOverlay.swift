import SwiftUI

/// 立绘 ack 横扫 overlay——同 CapsuleStateView 的 SweepOverlay 视觉语言，
/// 但适配立绘尺寸（更宽，扫得更慢）。
///
/// 由 PetPoseView 监听 AppState.ackPulse `.id(state.ackPulse)` 触发 onAppear 重建，
/// 与 earcon 200ms × 1.5 = 300ms 视觉同步（立绘大要扫得久点才看得到）。
struct PetSweepOverlay: View {
    let scheme: ColorScheme
    @State private var progress: Double = 0
    @State private var visible = false

    static let sweepDuration: Double = 0.3  // 立绘比药丸扫得慢

    var body: some View {
        GeometryReader { geo in
            let lineHeight: CGFloat = 2.0
            let xPos = geo.size.width * progress
            Rectangle()
                .fill(ZenStyle.inkTeal(scheme))
                .frame(width: geo.size.width * 0.4, height: lineHeight)
                .position(x: xPos, y: geo.size.height / 2)
                .opacity(visible ? 0.7 : 0.0)
        }
        .onAppear {
            withAnimation(.easeIn(duration: Self.sweepDuration / 2)) {
                visible = true
            }
            withAnimation(.linear(duration: Self.sweepDuration)) {
                progress = 1.0
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.sweepDuration / 2) {
                withAnimation(.easeOut(duration: Self.sweepDuration / 2)) {
                    visible = false
                }
            }
        }
        .allowsHitTesting(false)
    }
}
