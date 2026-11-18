// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import XCTest

/// UI tests for the app's windows, on fixtures (`CUA_SPACES_FIXTURES=1`: no
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

    /// A launch opens the web UI window: the app has no native main window.
    func testLaunchOpensTheWebUIWindow() {
        let app = launch()
        XCTAssertTrue(app.windows["cua-new-ui"].waitForExistence(timeout: 15))
        XCTAssertFalse(app.staticTexts["All Items"].exists, "no native sidebar")
    }

    /// New Space (⌘N, the menu bar) is the web wizard, in the same window.
    func testNewSpaceOpensTheWebUIWindow() {
        let app = launch("new-space")
        XCTAssertTrue(app.windows["cua-new-ui"].waitForExistence(timeout: 15))
        XCTAssertFalse(app.buttons["Create Space"].exists && app.otherElements["wizard-summary"].exists,
                       "no native New Space sheet")
    }

    /// The first run is the shared web onboarding in the web UI window: no
    /// native Welcome.
    func testFirstRunOpensTheNewUIWindow() {
        let app = launch(onboarded: false)
        XCTAssertTrue(app.windows["cua-new-ui"].waitForExistence(timeout: 15))
        XCTAssertFalse(app.staticTexts["Welcome to Cua Spaces"].exists, "no native onboarding")
    }
}
