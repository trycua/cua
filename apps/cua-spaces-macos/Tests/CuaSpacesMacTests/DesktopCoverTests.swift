// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import SwiftUI
import Testing

/// The preview cover's state (the core's), the auto-connect setting and the
/// one thumbnail store the notch tiles and the cover share.
@MainActor
@Suite("Desktop cover")
struct DesktopCoverTests {
    init() { _ = NSApplication.shared }

    func runningDetail(_ model: AppModel) async throws -> AppSpaceDetail {
        await model.refresh()
        let space = try #require(model.spaces.first { model.detail($0).canStream })
        return model.detail(space)
    }

    @Test func autoConnectIsOnByDefaultAndTheCoverSaysConnecting() async throws {
        let model = ViewModelTests().makeModel()
        #expect(model.settings.autoConnect)
        let detail = try await runningDetail(model)
        let cover = model.cover(detail, requested: false, stream: .noSession)
        #expect(cover.kind == .connecting && cover.text == "Connecting\u{2026}" && cover.openStream)
        #expect(model.cover(detail, requested: false, stream: .streaming).kind == .stream)
    }

    @Test func turnedOffTheCoverWaitsForConnect() async throws {
        let model = ViewModelTests().makeModel()
        await model.loadSettings()
        let general = try #require(model.settingsPage.sections.first { $0.id == "general" })
        let row = try #require(general.rows.first { $0.id == "auto-connect" })
        #expect(row.kind == .toggle && row.label == "Connect to the desktop automatically")
        #expect(row.options.first { $0.id == "on" }?.active == true)
        await model.choose(row: "auto-connect", option: "off")
        #expect(!model.settings.autoConnect)
        // Saved: a new model on the same file reads it back.
        #expect(!appSettingsLoad(path: model.settingsPath).autoConnect)
        let detail = try await runningDetail(model)
        let cover = model.cover(detail, requested: false, stream: .noSession)
        #expect(cover.kind == .connect && cover.button == "Connect" && !cover.openStream)
        // Pressed: open, connecting, then live.
        let pressed = model.cover(detail, requested: true, stream: .noSession)
        #expect(pressed.kind == .connecting && pressed.openStream)
        #expect(model.cover(detail, requested: true, stream: .streaming).kind == .stream)
    }

    /// The transport says streaming as soon as it opens: until a frame
    /// arrives, the cover still says Connecting.
    @Test func connectingLastsUntilTheFirstFrame() {
        typealias R = StreamPhaseReader<EmptyView>
        #expect(R.phase(.streaming, hasFrame: false) == .connecting)
        #expect(R.phase(.streaming, hasFrame: true) == .streaming)
        #expect(R.phase(.idle, hasFrame: false) == .idle)
        #expect(R.phase(.failed("x"), hasFrame: false) == .failed)
    }

    @Test func theNotchAndTheCoverReadOneEntry() async throws {
        let model = ViewModelTests().makeModel()
        #expect(model.thumbnails === model.notch.thumbnails)
        let image = DesktopCoverTests.desktop()
        var asked: [(String, UInt64?)] = []
        model.thumbnails.fetch = { id, maxAge in
            asked.append((id, maxAge))
            return SpaceThumbnailData(image: image.tiffRepresentation!, capturedAt: Date())
        }
        // The notch's refresh fills the entry the cover reads.
        let notchImage = await model.thumbnails.refresh("local:aurora", maxAge: 3)
        #expect(notchImage != nil)
        #expect(model.notch.thumbnails["local:aurora"] === model.thumbnails["local:aurora"])
        #expect(asked.count == 1 && asked[0].0 == "local:aurora" && asked[0].1 == 3_000)
    }

    @Test func theStoreKeepsTheNewestAndForgetsGoneSpaces() async {
        let store = SpaceThumbnails()
        let a = DesktopCoverTests.desktop(), b = DesktopCoverTests.desktop()
        let now = Date()
        store.set("s1", a, at: now)
        // An older capture never replaces a newer one (a stream's last
        // frame is newer than the daemon's cache).
        store.set("s1", b, at: now.addingTimeInterval(-60))
        #expect(store["s1"] === a)
        store.set("s1", b, at: now.addingTimeInterval(1))
        #expect(store["s1"] === b)
        store.set("s2", a)
        store.retain(["s2"])
        #expect(store["s1"] == nil && store["s2"] != nil)
        store["s2"] = nil
        #expect(store.entries.isEmpty)
    }

    @Test func warmingAsksTheCacheOncePerSpace() async {
        let store = SpaceThumbnails()
        var asked: [UInt64?] = []
        store.fetch = { _, maxAge in
            asked.append(maxAge)
            return nil
        }
        await store.warm(["s1"])
        await store.warm(["s1"])
        #expect(asked.count == 1, "once per launch")
        #expect(asked == [nil], "any age: the daemon's cache answers at once")
    }

    @Test func thePolicyComesFromTheCore() {
        let p = SpaceThumbnails.policy
        #expect((60_000...120_000).contains(p.backgroundIntervalMs))
        #expect(p.openIntervalMs == 3_000)
    }

    /// A stand-in desktop for the preview: a wallpaper gradient, a menu bar
    /// and two windows.
    static func desktop(size: NSSize = NSSize(width: 640, height: 400)) -> NSImage {
        NSImage(size: size, flipped: false) { rect in
            NSGradient(colors: [NSColor(calibratedRed: 0.16, green: 0.33, blue: 0.62, alpha: 1),
                                NSColor(calibratedRed: 0.86, green: 0.47, blue: 0.36, alpha: 1)])?
                .draw(in: rect, angle: -60)
            NSColor(white: 0.97, alpha: 0.9).setFill()
            NSRect(x: 0, y: rect.height - 16, width: rect.width, height: 16).fill()
            for (frame, tone) in [(NSRect(x: 60, y: 90, width: 300, height: 220), 0.98),
                                  (NSRect(x: 300, y: 50, width: 260, height: 180), 0.2)] {
                NSColor(white: tone, alpha: 1).setFill()
                NSBezierPath(roundedRect: frame, xRadius: 8, yRadius: 8).fill()
            }
            return true
        }
    }
}
