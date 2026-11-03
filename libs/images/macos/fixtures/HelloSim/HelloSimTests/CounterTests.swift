import XCTest
@testable import HelloSim

final class CounterTests: XCTestCase {
    func testStartsUntapped() {
        XCTAssertEqual(Counter().label, "Not tapped yet")
    }

    func testTapsAreCounted() {
        var c = Counter()
        c.tap()
        XCTAssertEqual(c.label, "Tapped 1 time")
        c.tap()
        XCTAssertEqual(c.label, "Tapped 2 times")
    }
}
