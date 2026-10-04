// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import XCTest

/// UI tests for the main views, on fixtures (`CUA_SPACES_FIXTURES=1`: no
/// daemon, no Space, no Keyvault broker; nothing leaves the process). They
/// drive only this app. Built by project.yml (XcodeGen) on a macOS runner
/// with Xcode; `swift test` cannot run XCUITest.
final class CuaSpacesMacUITests: XCTestCase {
    private func launch(_ view: String? = nil, onboarded: Bool = true) -> XCUIApplication {
        let app = XCUIApplication()
        let home = NSTemporaryDirectory() + "cua-uitest-\(UUID().uuidString)"
        let support = home + "/Library/Application Support/com.trycua.spaces.macos"
        try? FileManager.default.createDirectory(atPath: support, withIntermediateDirectories: true)
        if onboarded {
            FileManager.default.createFile(atPath: support + "/onboarding.json",
                                           contents: Data("{\"completed\":true,\"mode\":\"client\"}".utf8))
        }
        app.launchEnvironment = ["CUA_SPACES_FIXTURES": "1", "HOME": home, "CUA_HOME": home + "/.cua"]
        if let view { app.launchEnvironment["CUA_SPACES_START_VIEW"] = view }
        app.launch()
        return app
    }

    func testSidebarListsSpacesByLocation() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Aurora"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Cua Cloud"].exists)
        XCTAssertTrue(app.staticTexts["Connected"].exists)
        XCTAssertTrue(app.staticTexts["All Items"].exists, "the Keyvault categories are in the sidebar")
    }

    func testNewSpaceWalksToCreate() {
        let app = launch("new-space")
        let next = app.buttons["Continue"]
        XCTAssertTrue(next.waitForExistence(timeout: 10))
        next.click()
        next.click()
        next.click()
        XCTAssertTrue(app.buttons["Create Space"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.otherElements["wizard-summary"].exists || app.staticTexts["Runs on"].exists)
    }

    func testOnboardingStartsWithWelcome() {
        let app = launch(onboarded: false)
        XCTAssertTrue(app.staticTexts["Welcome to Cua Spaces"].waitForExistence(timeout: 10))
        app.buttons["Get started"].click()
        // First run no longer has a Command line page: Sign in follows Welcome.
        XCTAssertTrue(app.staticTexts["Connect your machines and your team."].waitForExistence(timeout: 5))
    }

    func testSpaceDetailShowsFactsAndTheDropWell() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Aurora"].waitForExistence(timeout: 10))
        app.staticTexts["Aurora"].click()
        XCTAssertTrue(app.staticTexts["Identifier"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Drop a file or window"].exists)
    }
}
