// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CoreGraphics
import Cua
import Foundation
import Testing
@testable import CuaSpacesStreaming

/// Records which sources were opened and refuses to stream, so a session
/// bound to it fails fast without a Space.
private final class RecordingProvider: SpaceStreamSourceProviding, @unchecked Sendable {
    private let lock = NSLock()
    private var opened: [StreamSource] = []

    var sources: [StreamSource] { lock.lock(); defer { lock.unlock() }; return opened }

    func availableWindows() async throws -> [StreamWindow] { [] }

    func openSession(_ source: StreamSource, frames: FrameSink, audio: AudioSink?) async throws -> SpaceStreamSession {
        lock.lock(); opened.append(source); lock.unlock()
        throw StreamError.noStream("offline test provider")
    }

    func joinPresence(name: String, color: String?) async throws -> SpacePresence {
        throw StreamError.noStream("offline test provider")
    }
}

@Suite(.serialized) @MainActor final class PiPTests {
    init() { StreamPiPController.presentsPanels = false }

    private let terminal = StreamWindow(id: "w-7", app: "Terminal", title: "cua@space: ~", epoch: 3)

    @Test func testPiPKeysAreOnePerDesktopOrWindowHandle() {
        XCTAssertEqual(StreamSource.desktop.pipKey, "desktop")
        XCTAssertEqual(StreamSource.window(terminal).pipKey, "window:w-7")
        var later = terminal
        later.epoch = 9
        later.surfaceSize = CGSize(width: 640, height: 480)
        XCTAssertEqual(StreamSource.window(later).pipKey, StreamSource.window(terminal).pipKey,
                       "a new epoch or size is the same window, so the same panel")
        XCTAssertEqual(StreamSource.desktop.pipTitle, "Desktop")
        XCTAssertEqual(StreamSource.window(terminal).pipTitle, "cua@space: ~")
        XCTAssertEqual(StreamSource.window(StreamWindow(id: "w-8", app: "Thunar", title: "")).pipTitle, "Thunar")
    }

    /// A window pop-out owns a session on that window, in a floating panel
    /// titled with the window, and stops it when the panel closes.
    @Test func testWindowPopOutOwnsAndStopsItsSession() async throws {
        let provider = RecordingProvider()
        let pips = StreamPiPSet(provider: provider)
        pips.popOut(.window(terminal))
        XCTAssertTrue(pips.isOpen(.window(terminal)))
        let controller = try #require(pips.controller(for: .window(terminal)))
        XCTAssertTrue(controller.ownsSession)
        let panel = try #require(controller.window as? NSPanel)
        XCTAssertEqual(panel.title, "cua@space: ~")
        XCTAssertEqual(panel.level, .floating)
        XCTAssertTrue(panel.isFloatingPanel)
        let session = try #require(controller.session)
        for _ in 0..<200 where provider.sources.isEmpty { try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(provider.sources, [.window(terminal)])

        pips.popIn(.window(terminal))
        XCTAssertFalse(pips.isOpen(.window(terminal)))
        XCTAssertTrue(pips.openKeys.isEmpty)
        XCTAssertNil(controller.session)
        for _ in 0..<200 where session.status != .idle { try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(session.status, .idle, "closing an owned pop-out stops its stream")
    }

    /// The desktop borrows the app's session and a window opens beside it:
    /// two panels at once, and closing the desktop one never stops the app's
    /// stream.
    @Test func testDesktopSharesAndWindowsOpenBesideIt() throws {
        let provider = RecordingProvider()
        let shared = LiveStreamSession(provider: provider)
        let pips = StreamPiPSet(provider: provider)
        pips.toggle(.desktop, sharing: shared)
        pips.toggle(.window(terminal))
        XCTAssertEqual(pips.openKeys, ["desktop", "window:w-7"])
        XCTAssertFalse(try #require(pips.controller(for: .desktop)).ownsSession)
        XCTAssertTrue(shared.isPoppedOut)
        XCTAssertTrue(pips.controller(for: .desktop)?.session === shared)

        pips.toggle(.desktop)
        XCTAssertEqual(pips.openKeys, ["window:w-7"])
        XCTAssertFalse(shared.isPoppedOut)
        XCTAssertEqual(shared.status, .idle, "the app's session is not the panel's to stop")
        pips.popInAll()
        XCTAssertTrue(pips.openKeys.isEmpty)
    }

    /// Closing the panel with its close button is the same as popping in.
    @Test func testClosingThePanelPopsIn() throws {
        let pips = StreamPiPSet(provider: RecordingProvider())
        pips.popOut(.window(terminal))
        let panel = try #require(pips.controller(for: .window(terminal))?.window)
        panel.close()
        XCTAssertFalse(pips.isOpen(.window(terminal)))
    }
}
