// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CoreGraphics
import Cua
import CuaSpacesFFI
import Foundation
import Testing
@testable import CuaSpacesTeleport

@Suite(.serialized) @MainActor final class DropZoneTests {
    @Test func testDropZoneWording() {
        XCTAssertEqual(TeleportDropZone.caption, "Drop a file or window")
        XCTAssertEqual(TeleportDropZone.sendFileTitle, "Send file\u{2026}")
        XCTAssertEqual(TeleportDropZone.teleportAppTitle, "Teleport an app\u{2026}")
    }

    @Test func testDropZoneRoutesFilesWithoutTeleport() {
        let file = URL(fileURLWithPath: "/tmp/report.pdf")
        XCTAssertEqual(TeleportDropZone.route([file], teleport: nil), .files([file]))
        XCTAssertEqual(TeleportDropZone.route([URL(string: "https://cua.ai")!], teleport: nil), .none)
    }

    @Test func testDropZoneRoutesFilesThroughTheSDKParser() throws {
        let t = try Cua.embedded().teleport()
        let file = URL(fileURLWithPath: "/tmp/report.pdf")
        XCTAssertEqual(TeleportDropZone.route([file], teleport: t), .files([file]))
    }

    /// A window released inside a registered zone commits to that zone's
    /// Space; outside it, nothing commits.
    @Test func testWindowReleasedOverTheZoneCommits() throws {
        let watcher = WindowDragWatcher(teleport: try Cua.embedded().teleport())
        watcher.capturesThumbnails = false
        watcher.registerZone("local:dev") { CGRect(x: 100, y: 100, width: 200, height: 120) }
        XCTAssertEqual(watcher.zone(at: 150, 150), "local:dev")
        XCTAssertNil(watcher.zone(at: 10, 10))

        var commits: [WindowDropState.Commit] = []
        watcher.onCommit = { commits.append($0) }
        let w = TeleportWindow(windowId: 7, pid: 70, appName: "Visual Studio Code", title: "main.rs", bundlePath: nil)
        watcher.handle(TeleportWindowDragEvent(phase: "start", x: 10, y: 10, window: w, app: vscode))
        XCTAssertNil(watcher.state.overId)
        watcher.handle(TeleportWindowDragEvent(phase: "move", x: 150, y: 150, window: nil, app: nil))
        XCTAssertEqual(watcher.state.overId, "local:dev")
        watcher.handle(TeleportWindowDragEvent(phase: "end", x: 150, y: 150, window: nil, app: nil))
        XCTAssertEqual(commits.map(\.targetId), ["local:dev"])

        // A window resized from its left edge over the zone: never a drop.
        watcher.handle(TeleportWindowDragEvent(phase: "start", x: 150, y: 150, window: w, app: vscode,
                                               startFrame: AppLogicalRect(x: 170, y: 100, width: 400, height: 300),
                                               frame: AppLogicalRect(x: 150, y: 100, width: 420, height: 300)))
        XCTAssertFalse(watcher.state.active)
        watcher.handle(TeleportWindowDragEvent(phase: "end", x: 150, y: 150, window: nil, app: nil))
        XCTAssertEqual(commits.count, 1)

        watcher.unregisterZone("local:dev")
        watcher.handle(TeleportWindowDragEvent(phase: "start", x: 150, y: 150, window: w, app: vscode))
        watcher.handle(TeleportWindowDragEvent(phase: "end", x: 150, y: 150, window: nil, app: nil))
        XCTAssertEqual(commits.count, 1)
    }
}
