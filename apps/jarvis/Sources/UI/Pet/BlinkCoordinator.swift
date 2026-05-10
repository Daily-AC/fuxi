import Foundation
import Combine

/// 偶发微眨的调度器——4-7s 随机间隔触发一个 0.15s blink。
/// PetPoseView 监听 `$blinkTrigger` `.onChange` 触发渐隐线 overlay。
///
/// 用 Timer + RunLoop.main——不引 Combine `Timer.publish` 是因为它 cancel 时
/// 需 store cancellable，复杂度对这点功能不值。
@MainActor
final class BlinkCoordinator: ObservableObject {
    /// 自增计数器——每次眨眼 +1。SwiftUI .onChange 监听其值变化触发 overlay 重建。
    @Published private(set) var blinkTrigger: Int = 0

    private var timer: Timer?

    /// 启动后台调度——AppDelegate 切到 pet 模式时调；切回 capsule 时 stop。
    func start() {
        scheduleNext()
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    /// 单测用 trigger（绕开 Timer，直接 increment）
    func fireForTesting() {
        blinkTrigger &+= 1
    }

    static func randomInterval() -> TimeInterval {
        Double.random(in: 4.0...7.0)
    }

    private func scheduleNext() {
        let interval = Self.randomInterval()
        let t = Timer(timeInterval: interval, repeats: false) { [weak self] _ in
            Task { @MainActor in
                guard let self = self else { return }
                self.blinkTrigger &+= 1
                self.scheduleNext()
            }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }
}
