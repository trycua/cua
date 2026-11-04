// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpaces
import CuaSpacesStreaming
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// The window list's icons: the Space's own (`Space.appIcons`), one call per
/// list with one request per app, and nothing at all when the Space has none.
@MainActor
@Suite final class WindowIconsTests {

    static func png() -> Data {
        let image = NSImage(size: NSSize(width: 4, height: 4), flipped: false) { r in
            NSColor.red.setFill(); r.fill(); return true
        }
        let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
        return rep.representation(using: .png, properties: [:])!
    }

    func window(_ id: String, app: String = "Xfce4-terminal", appID: String = "xfce4-terminal") -> StreamWindow {
        StreamWindow(id: id, app: app, title: "t", appID: appID, processID: 247)
    }

    @Test func testOneCallPerListOneRequestPerAppAndTheImageDecodes() async {
        var calls: [[AppIconRequest]] = []
        let icons = WindowIcons { r in calls.append(r); return r.map { _ in AppIcon(data: Self.png(), contentType: "image/png") } }
        await icons.load([window("a"), window("b"), window("c", app: "Thunar", appID: "thunar")])
        XCTAssertEqual(calls.count, 1)
        XCTAssertEqual(calls.first?.map(\.appID), ["xfce4-terminal", "thunar"], "two windows of one app asked twice")
        XCTAssertNotNil(icons.image(for: window("b")))
    }

    @Test func testNoIconMeansNoImageNotAPlaceholder() async {
        let none = WindowIcons { r in r.map { _ in nil } }
        await none.load([window("a", app: "XCalc", appID: "xcalc")])
        XCTAssertNil(none.image(for: window("a", app: "XCalc", appID: "xcalc")))
        let garbage = WindowIcons { r in r.map { _ in AppIcon(data: Data("nope".utf8), contentType: "image/png") } }
        await garbage.load([window("a")])
        XCTAssertNil(garbage.image(for: window("a")))
    }

    @Test func testWindowsCarryTheAppIDAndPidTheLookupNeeds() {
        let w = StreamWindow(CuaSpaces.SpaceWindow(id: WindowID("29360131"), app: "Xfce4-terminal", title: "t",
                                                   processID: 247, appID: "xfce4-terminal"))
        XCTAssertEqual(w.appID, "xfce4-terminal")
        XCTAssertEqual(w.processID, 247)
    }
}
