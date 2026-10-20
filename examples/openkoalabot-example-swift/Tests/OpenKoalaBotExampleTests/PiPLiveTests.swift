// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpacesStreaming
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// Picture-in-picture against a live Space: the desktop (a shared session)
/// and one Space window (a session of its own) popped out at once, both
/// decoding new frames. Skips without a live Space (`LiveSpace`). Opens two
/// of this process's own floating panels and closes them.
@Suite(.liveSpace, .serialized) @MainActor final class PiPLiveTests {
    @Test func testDesktopAndWindowPopOutsBothStream() async throws {
        let target = try LiveSpace.require("the PiP live test")
        let space = try await target.client.sdkSpace(target.space)
        let provider = SpaceStreamProvider(space: space)
        let desktop = LiveStreamSession(provider: provider)
        await desktop.refreshWindows()
        let window = try #require(desktop.windows.first, "the Space lists no windows to pop out")
        await desktop.select(.desktop)

        let pips = StreamPiPSet(provider: provider)
        pips.popOut(.desktop, sharing: desktop)
        pips.popOut(.window(window))
        defer { pips.popInAll() }
        let owned = try #require(pips.controller(for: .window(window))?.session)
        XCTAssertEqual(pips.openKeys, ["desktop", StreamSource.window(window).pipKey])

        func frames(after seconds: Int) async throws -> (Int, Int) {
            try await Task.sleep(for: .seconds(seconds))
            return (desktop.decodedFrameCount, owned.decodedFrameCount)
        }
        let first = try await frames(after: 6)
        XCTAssertGreaterThan(first.0, 0, "the desktop pop-out decoded nothing: \(desktop.status)")
        XCTAssertGreaterThan(first.1, 0, "the window pop-out decoded nothing: \(owned.status)")
        XCTAssertEqual(owned.source, .window(window))

        pips.popIn(.window(window))
        for _ in 0..<100 where owned.status != .idle { try await Task.sleep(for: .milliseconds(50)) }
        XCTAssertEqual(owned.status, .idle, "closing the window pop-out stops its stream")
        pips.popIn(.desktop)
        await desktop.stop()
    }
}
