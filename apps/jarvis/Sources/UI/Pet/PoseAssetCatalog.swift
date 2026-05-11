import Foundation
import AppKit

protocol PoseBundleSource {
    func url(forPose name: String) -> URL?
}

/// 两条 build 路径资产位置不同——SwiftPM `swift build` 进 `Bundle.module`，
/// xcodegen+xcodebuild .app 进 `Bundle.main`。先 module 后 main 兜两端。
struct DefaultPoseBundle: PoseBundleSource {
    func url(forPose name: String) -> URL? {
        let res = "Pet/poses/\(name)@2x"
        if let u = Bundle.module.url(forResource: res, withExtension: "png") {
            return u
        }
        return Bundle.main.url(forResource: res, withExtension: "png")
    }
}

struct PoseAssetCatalog {
    static let poseNames: [String] = ["idle", "listening", "thinking", "speaking", "ack"]

    let bundle: PoseBundleSource

    init(bundle: PoseBundleSource = DefaultPoseBundle()) {
        self.bundle = bundle
    }

    static func poseName(for phase: AppState.VoicePhase) -> String? {
        switch phase {
        case .idle:      return "idle"
        case .listening: return "listening"
        case .sending, .waiting: return "thinking"
        case .speaking:  return "speaking"
        }
    }

    func validate() -> Bool {
        for name in Self.poseNames {
            if bundle.url(forPose: name) == nil {
                return false
            }
        }
        return true
    }

    func image(for poseName: String) -> NSImage? {
        guard let url = bundle.url(forPose: poseName) else { return nil }
        return NSImage(contentsOf: url)
    }
}
