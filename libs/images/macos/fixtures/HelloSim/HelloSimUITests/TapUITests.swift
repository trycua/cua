import XCTest

/// Drives the running app in the Simulator and leaves screenshots of what it
/// sees as test attachments (xcresulttool exports them).
final class TapUITests: XCTestCase {
    func testTapUpdatesTheLabel() throws {
        let app = XCUIApplication()
        app.launch()
        attach(app, "before")
        XCTAssertTrue(app.staticTexts["Not tapped yet"].waitForExistence(timeout: 20))
        app.buttons["tap"].tap()
        app.buttons["tap"].tap()
        XCTAssertTrue(app.staticTexts["Tapped 2 times"].waitForExistence(timeout: 10))
        attach(app, "after")
    }

    private func attach(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
