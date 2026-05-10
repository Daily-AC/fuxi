import XCTest
@testable import Jarvis

@MainActor
final class PetBlinkCoordinatorTests: XCTestCase {
    func test_random_interval_in_range() {
        for _ in 0..<100 {
            let interval = BlinkCoordinator.randomInterval()
            XCTAssertGreaterThanOrEqual(interval, 4.0)
            XCTAssertLessThanOrEqual(interval, 7.0)
        }
    }

    func test_blink_trigger_increments() {
        let c = BlinkCoordinator()
        let before = c.blinkTrigger
        c.fireForTesting()
        XCTAssertEqual(c.blinkTrigger, before + 1)
    }
}
