import XCTest
@preconcurrency import AVFoundation
@testable import Jarvis

// PCMBuffer 是 Recognizer 内部 audio thread → main snapshot 的 ring。
// WhisperKit transcribe 没法在 CI 上测（要硬件 + 800MB 模型）；这里专测 buffer 的
// append / snapshot / reset 三件套，保证 audio 数据没被 race 弄丢/截断。
final class PCMBufferTests: XCTestCase {

    func test_append_thenSnapshot_returnsAllSamples() {
        let buf = PCMBuffer()
        let samples: [Float] = [0.1, 0.2, 0.3, -0.4, -0.5]
        let pcm = makeBuffer(samples: samples)
        buf.append(pcm)

        let snap = buf.snapshot()
        XCTAssertEqual(snap.count, samples.count)
        for (i, s) in samples.enumerated() {
            XCTAssertEqual(snap[i], s, accuracy: 1e-6)
        }
        XCTAssertEqual(buf.count, samples.count)
    }

    func test_appendMultiple_concatenatesInOrder() {
        let buf = PCMBuffer()
        buf.append(makeBuffer(samples: [1.0, 2.0]))
        buf.append(makeBuffer(samples: [3.0, 4.0, 5.0]))
        let snap = buf.snapshot()
        XCTAssertEqual(snap, [1.0, 2.0, 3.0, 4.0, 5.0])
    }

    func test_reset_clears() {
        let buf = PCMBuffer()
        buf.append(makeBuffer(samples: [1.0, 2.0, 3.0]))
        XCTAssertEqual(buf.count, 3)
        buf.reset()
        XCTAssertEqual(buf.count, 0)
        XCTAssertTrue(buf.snapshot().isEmpty)
    }

    func test_snapshot_isCopy_notSharedStorage() {
        let buf = PCMBuffer()
        buf.append(makeBuffer(samples: [1.0, 2.0]))
        let snap = buf.snapshot()
        buf.append(makeBuffer(samples: [3.0]))
        // snap 是 reset 前的拷贝——不应该被后续 append 影响
        XCTAssertEqual(snap.count, 2)
        XCTAssertEqual(buf.snapshot().count, 3)
    }

    func test_appendEmptyBuffer_isNoop() {
        let buf = PCMBuffer()
        let pcm = makeBuffer(samples: [])
        buf.append(pcm)
        XCTAssertEqual(buf.count, 0)
    }

    // MARK: helpers

    private func makeBuffer(samples: [Float]) -> AVAudioPCMBuffer {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                   sampleRate: 16_000,
                                   channels: 1,
                                   interleaved: false)!
        let cap = max(AVAudioFrameCount(samples.count), 1)
        let pcm = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: cap)!
        pcm.frameLength = AVAudioFrameCount(samples.count)
        if let ptr = pcm.floatChannelData?[0] {
            for (i, s) in samples.enumerated() {
                ptr[i] = s
            }
        }
        return pcm
    }
}
