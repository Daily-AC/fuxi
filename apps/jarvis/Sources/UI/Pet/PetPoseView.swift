import SwiftUI
import AppKit

struct PetPoseView: View {
    @ObservedObject var state: AppState
    @StateObject var blink = BlinkCoordinator()
    @Environment(\.colorScheme) private var scheme

    private let catalog = PoseAssetCatalog()

    var body: some View {
        ZStack {
            poseImage
                .id(state.phase)
                .transition(.opacity.animation(.easeInOut(duration: 0.25)))

            SleeveCanvasOverlay(
                amplitudeBoost: state.phase == .listening ? state.audioLevel : 0,
                scheme: scheme
            )

            if state.phase == .idle || state.phase == .listening {
                BlinkLineOverlay(scheme: scheme)
                    .id(blink.blinkTrigger)
            }

            if state.ackPulse > 0 {
                PetSweepOverlay(scheme: scheme)
                    .id(state.ackPulse)
            }
        }
        .frame(width: 280, height: 420)
        .onAppear { blink.start() }
        .onDisappear { blink.stop() }
        .animation(.easeInOut(duration: 0.25), value: state.phase)
    }

    @ViewBuilder
    private var poseImage: some View {
        let poseName = PoseAssetCatalog.poseName(for: state.phase) ?? "idle"
        if let nsImg = catalog.image(for: poseName) {
            Image(nsImage: nsImg)
                .resizable()
                .interpolation(.high)
                .aspectRatio(contentMode: .fit)
        } else {
            ZStack {
                Rectangle()
                    .fill(ZenStyle.paper(scheme).opacity(0.3))
                VStack(spacing: 8) {
                    Image(systemName: "questionmark.square.dashed")
                        .font(.system(size: 48, weight: .light))
                        .foregroundStyle(ZenStyle.inkTeal(scheme).opacity(0.5))
                    Text("立绘资产未就绪")
                        .font(.caption)
                        .foregroundStyle(ZenStyle.inkTeal(scheme).opacity(0.5))
                    Text("（\(poseName)@2x.png 缺失）")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }
}

private struct BlinkLineOverlay: View {
    let scheme: ColorScheme
    @State private var visible: Bool = false

    var body: some View {
        GeometryReader { geo in
            let y = geo.size.height * 0.32
            Rectangle()
                .fill(ZenStyle.inkTeal(scheme))
                .frame(width: geo.size.width * 0.18, height: 0.8)
                .position(x: geo.size.width * 0.5, y: y)
                .opacity(visible ? 0.6 : 0.0)
        }
        .onAppear {
            withAnimation(.easeIn(duration: 0.06)) {
                visible = true
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) {
                withAnimation(.easeOut(duration: 0.07)) {
                    visible = false
                }
            }
        }
        .allowsHitTesting(false)
    }
}
