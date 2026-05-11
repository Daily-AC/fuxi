import XCTest
@testable import Jarvis

final class PetPoseAssetCatalogTests: XCTestCase {
    func test_phase_to_pose_mapping_complete() {
        let phases: [AppState.VoicePhase] = [.idle, .listening, .sending, .waiting, .speaking]
        for p in phases {
            XCTAssertNotNil(PoseAssetCatalog.poseName(for: p), "缺 pose mapping for \(p)")
        }
    }

    func test_sending_and_waiting_share_thinking_pose() {
        XCTAssertEqual(PoseAssetCatalog.poseName(for: .sending), "thinking")
        XCTAssertEqual(PoseAssetCatalog.poseName(for: .waiting), "thinking")
    }

    func test_validate_returns_false_when_assets_missing() {
        let catalog = PoseAssetCatalog()
        _ = catalog.validate()
    }

    func test_validate_with_in_memory_bundle_all_present_returns_true() {
        let mock = MockPoseBundle(present: ["idle", "listening", "thinking", "speaking", "ack"])
        let catalog = PoseAssetCatalog(bundle: mock)
        XCTAssertTrue(catalog.validate())
    }

    func test_validate_with_one_missing_returns_false() {
        let mock = MockPoseBundle(present: ["idle", "listening", "thinking", "speaking"])
        let catalog = PoseAssetCatalog(bundle: mock)
        XCTAssertFalse(catalog.validate())
    }
}

private final class MockPoseBundle: PoseBundleSource {
    let present: Set<String>
    init(present: [String]) { self.present = Set(present) }
    func url(forPose name: String) -> URL? {
        present.contains(name) ? URL(fileURLWithPath: "/tmp/mock-\(name).png") : nil
    }
}
