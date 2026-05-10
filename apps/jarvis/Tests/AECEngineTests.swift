import XCTest
@preconcurrency import AVFoundation
@testable import Jarvis

// AECEngine 在测试沙箱里**不能真起 audio engine**——CI/构建机无 mic 硬件，vpio 启动会
// 失败。本套测专测 dispatcher 部分（add/remove/fan-out）+ 单例幂等。
//
// 真硬件部分（vpio 起、earcon 播放、AEC 抑制效果）需要本机实测，scrip 不覆盖。
final class AECEngineTests: XCTestCase {

    // MARK: 单例幂等

    @MainActor
    func test_shared_isSingleton() {
        let a = AECEngine.shared
        let b = AECEngine.shared
        XCTAssertTrue(a === b)
    }

    // MARK: TapDispatcher fan-out

    func test_dispatcher_addAndRemove_tracksCount() {
        let d = TapDispatcher()
        XCTAssertEqual(d.count, 0)
        d.add(id: "a", format: nil, converter: nil) { _ in }
        d.add(id: "b", format: nil, converter: nil) { _ in }
        XCTAssertEqual(d.count, 2)
        XCTAssertTrue(d.contains(id: "a"))
        XCTAssertTrue(d.contains(id: "b"))

        d.remove(id: "a")
        XCTAssertEqual(d.count, 1)
        XCTAssertFalse(d.contains(id: "a"))
        XCTAssertTrue(d.contains(id: "b"))
    }

    func test_dispatcher_addSameId_replacesExisting() {
        let d = TapDispatcher()
        d.add(id: "x", format: nil, converter: nil) { _ in }
        d.add(id: "x", format: nil, converter: nil) { _ in }
        // 两次 add 同 id 算替换，count 仍 1
        XCTAssertEqual(d.count, 1)
    }

    func test_dispatcher_dispatch_passthroughBuffer_callsAllListeners() {
        let d = TapDispatcher()
        let buffer = makeSilenceBuffer(sampleRate: 48000, frames: 480)

        let counter = HitCounter()
        d.add(id: "one", format: nil, converter: nil) { _ in counter.hit() }
        d.add(id: "two", format: nil, converter: nil) { _ in counter.hit() }

        d.dispatch(buffer: buffer)
        XCTAssertEqual(counter.value, 2)

        // 派发不破坏 listener 表
        d.dispatch(buffer: buffer)
        XCTAssertEqual(counter.value, 4)
    }

    func test_dispatcher_removeAll_clears() {
        let d = TapDispatcher()
        d.add(id: "a", format: nil, converter: nil) { _ in }
        d.add(id: "b", format: nil, converter: nil) { _ in }
        d.removeAll()
        XCTAssertEqual(d.count, 0)
    }

    func test_dispatcher_removeUnknownId_isNoop() {
        let d = TapDispatcher()
        d.add(id: "a", format: nil, converter: nil) { _ in }
        d.remove(id: "ghost")
        XCTAssertEqual(d.count, 1)
    }

    /// 无 listener 时 dispatch 不爆。
    func test_dispatcher_dispatch_withNoListeners_isSafe() {
        let d = TapDispatcher()
        let buffer = makeSilenceBuffer(sampleRate: 48000, frames: 480)
        d.dispatch(buffer: buffer)
        // 没崩就过
        XCTAssertEqual(d.count, 0)
    }

    // MARK: helpers

    /// 构造一段静音 PCM buffer（mono Float32）——dispatcher 测试不关心音频内容，只看 fan-out。
    private func makeSilenceBuffer(sampleRate: Double, frames: AVAudioFrameCount) -> AVAudioPCMBuffer {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                   sampleRate: sampleRate,
                                   channels: 1,
                                   interleaved: false)!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
        buffer.frameLength = frames
        // 默认全 0，足够。
        return buffer
    }
}

/// 多 listener 闭包共享的计数器——闭包是 @Sendable 不能 mutate var，class 包一层。
private final class HitCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var counter = 0
    func hit() {
        lock.lock(); defer { lock.unlock() }
        counter += 1
    }
    var value: Int {
        lock.lock(); defer { lock.unlock() }
        return counter
    }
}
