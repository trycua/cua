// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// Picker fixtures: the parity flow's catalog, two of this Mac's windows and
/// two of the Space's, with generated images standing in for icons and
/// previews.
enum PickerFixture {
    static func json(_ name: String) throws -> [String: Any] {
        let url = ParityTests.dir.appendingPathComponent("\(name).json")
        return try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
    }

    static func entries() throws -> [AppCatalogEntry] {
        let flow = try json("picker-grid")
        let data = try JSONSerialization.data(withJSONObject: flow["entries"]!)
        return try appCatalogEntriesFromJson(json: String(decoding: data, as: UTF8.self))
    }

    static let windows = [
        AppOpenWindow(windowId: 41, appId: "slack", appName: "Slack", windowTitle: "general - Acme",
                      supported: true, bundlePath: "/Applications/Slack.app"),
        AppOpenWindow(windowId: 12, appId: "com.microsoft.VSCode", appName: "Code", windowTitle: "main.rs - cua",
                      supported: true, bundlePath: nil),
    ]

    static let remote = [
        AppRemoteWindow(id: "w-1", appName: "Firefox", title: "Mozilla Firefox", visible: true, appId: "firefox",
                        targetEpoch: 3, widthPx: nil, heightPx: nil, pid: 812),
        AppRemoteWindow(id: "w-2", appName: "xterm", title: "", visible: true, appId: "xterm",
                        targetEpoch: 1, widthPx: nil, heightPx: nil, pid: 904),
    ]

    /// A filled PNG of `color`, `w` x `h`.
    static func png(_ color: NSColor, _ w: Int = 64, _ h: Int = 64) -> Data {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        color.setFill()
        NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: w, height: h), xRadius: CGFloat(w) / 6,
                     yRadius: CGFloat(w) / 6).fill()
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])!
    }

    final class Calls: @unchecked Sendable {
        private let lock = NSLock()
        private var _log: [String] = []
        var log: [String] { lock.withLock { _log } }
        func note(_ s: String) { lock.withLock { _log.append(s) } }
    }

    static func sources(_ calls: Calls = Calls()) -> TeleportPickerSources {
        TeleportPickerSources(
            openWindows: { windows },
            remoteWindows: { remote },
            hostIcon: { path in
                calls.note("icon \(path)")
                return path.contains("Slack") ? png(.systemPurple) : path.contains("Code") ? png(.systemBlue) : nil
            },
            guestIcons: { r in
                calls.note("guest icons \(r.map(\.appName).joined(separator: ","))")
                return r.map { $0.appName == "Firefox" ? png(.systemOrange) : nil }
            },
            hostThumbnail: { id in
                calls.note("thumb \(id)")
                return id == 41 ? png(.systemTeal, 320, 200) : nil
            },
            guestThumbnail: { id, _ in
                calls.note("guest thumb \(id)")
                return png(.systemGray, 320, 200)
            })
    }
}

@MainActor
@Suite("Teleport picker grid")
struct TeleportGridTests {
    func model(_ calls: PickerFixture.Calls = .init()) throws -> TeleportModel {
        let m = TeleportModel(spaceName: "Aurora", teleport: nil, space: nil, sources: PickerFixture.sources(calls))
        m.send(.loaded(entries: try PickerFixture.entries()))
        return m
    }

    @Test func tabsAndAppTilesComeFromTheCore() async throws {
        let m = try model()
        #expect(m.tabs.map(\.label) == ["Apps", "Open windows", "From Aurora"])
        await m.loadWindows()
        let tiles = m.grid.sections.flatMap(\.tiles)
        #expect(tiles.map(\.title) == ["Slack", "Visual Studio Code", "Figma"])
        #expect(tiles[0].thumbnail == .hostWindow(windowId: 41))
        #expect(tiles[2].disabled && tiles[2].help == "no Linux build")
    }

    @Test func iconsAndPreviewsLoadLazilyThroughTheSources() async throws {
        let calls = PickerFixture.Calls()
        let m = try model(calls)
        await m.loadWindows()
        let slack = m.grid.sections.flatMap(\.tiles)[0]
        #expect(m.icon(for: slack) != nil)
        #expect(m.thumbnail(for: slack) == nil, "previews wait for the tile to show")
        await m.loadThumbnail(slack)
        #expect(m.thumbnail(for: slack) != nil)
        await m.loadThumbnail(slack)
        #expect(calls.log.filter { $0 == "thumb 41" }.count == 1)
    }

    @Test func arrowsMoveAndTheSpaceTabStreams() async throws {
        let m = try model()
        await m.loadWindows()
        m.step(1)
        #expect(m.state.selectedId == "code")
        m.step(3)
        #expect(m.state.selectedId == "code", "clamped at the last choosable tile")
        m.tab = .space
        await m.loadWindows()
        let tiles = m.grid.sections.flatMap(\.tiles)
        #expect(tiles.map(\.title) == ["Mozilla Firefox", "xterm"])
        var streamed: [String] = []
        m.onStreamWindow = { streamed.append($0) }
        await m.activate(tiles[0])
        #expect(streamed == ["w-1"])
    }
}
