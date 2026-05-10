import Foundation
import AppKit

protocol PoseBundleSource {
    func url(forPose name: String) -> URL?
}

struct DefaultPoseBundle: PoseBundleSource {
    func url(forPose name: String) -> URL? {
        Bundle.module.url(forResource: "Pet/poses/\(name)@2x", withExtension: "png")
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
