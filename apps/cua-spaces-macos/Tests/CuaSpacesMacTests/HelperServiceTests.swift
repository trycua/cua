// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The host service's login item, per channel: the released app keeps the
/// label, plist and spec it always had (a bundle with no stamped keys), and
/// Cua Spaces Local reads its own from the bundle.
@Suite("Helper service names")
struct HelperServiceTests {
    let home = "/Users/ada"

    @Test func aBundleWithoutKeysIsTheReleasedApp() {
        for info in [nil, [:], ["CuaChannel": "stable"], ["CuaHelperLabel": "  ", "CuaHome": ""]] as [[String: Any]?] {
            #expect(HelperService.label(info: info) == "com.trycua.spaces.helper")
            #expect(HelperService.plistName(info: info) == "com.trycua.spaces.helper.plist")
            #expect(HelperService.specPath(info: info, home: home) == "/Users/ada/.cua/helper/service.json")
        }
    }

    @Test func theReleasedAppStampedWithItsOwnNamesKeepsThem() {
        let info: [String: Any] = ["CuaChannel": "stable", "CuaHelperLabel": "com.trycua.spaces.helper", "CuaHome": "~/.cua"]
        #expect(HelperService.label(info: info) == "com.trycua.spaces.helper")
        #expect(HelperService.plistName(info: info) == "com.trycua.spaces.helper.plist")
        #expect(HelperService.specPath(info: info, home: home) == "/Users/ada/.cua/helper/service.json")
    }

    @Test func localReadsItsOwnHelperFromTheBundle() {
        let info: [String: Any] = [
            "CuaChannel": "local", "CuaHelperLabel": "com.trycua.spaces.local.helper", "CuaHome": "~/.cua-local",
        ]
        #expect(HelperService.label(info: info) == "com.trycua.spaces.local.helper")
        #expect(HelperService.plistName(info: info) == "com.trycua.spaces.local.helper.plist")
        #expect(HelperService.specPath(info: info, home: home) == "/Users/ada/.cua-local/helper/service.json")
    }

    @Test func anAbsoluteHomeIsUsedAsIs() {
        #expect(HelperService.specPath(info: ["CuaHome": "/tmp/h"], home: home) == "/tmp/h/helper/service.json")
        #expect(HelperService.specPath(info: ["CuaHome": "~"], home: home) == "/Users/ada/helper/service.json")
    }

    @Test func aBundleOfAnotherChannelNeverReadsTheReleasedAppsSpec() {
        // No home, or one that is not a path: run nothing rather than the
        // released app's helper.
        #expect(HelperService.specPath(info: ["CuaChannel": "local"], home: home) == nil)
        #expect(HelperService.specPath(info: ["CuaChannel": "local", "CuaHome": "cua-local"], home: home) == nil)
        #expect(HelperService.specPath(info: ["CuaHome": "cua-local"], home: home) == nil)
    }

    /// The host service runs from Contents/Helpers, outside Contents/MacOS:
    /// from Contents/MacOS, AppKit (its cursor overlay) registered it with
    /// LaunchServices as the app itself, and opening Cua Spaces activated
    /// the windowless helper instead of showing a window.
    @Test func theHostServiceRunsFromContentsHelpers() {
        let exe = URL(fileURLWithPath: "/Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac")
        let daemon = HelperService.daemonPath(executable: exe)
        #expect(daemon.path == "/Applications/Cua Spaces.app/Contents/Helpers/cua-spacesd")
        #expect(!daemon.path.contains("/Contents/MacOS/"))
        #expect(HelperService.daemonInContents == "Helpers/cua-spacesd")
    }
}
