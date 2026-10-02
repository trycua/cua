// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import CuaSpacesStreaming
import Foundation
import Testing

/// Stream rows fixtures: an Ubuntu Space with a screen target, two Firefox
/// windows, an untitled xterm and a long gedit title.
enum StreamFixture {
    static let longTitle = "Quarterly planning notes for the Spaces launch, draft 7 (unsaved changes) - gedit"

    static let windows: [StreamWindow] = [
        StreamWindow(id: "w-screen", app: "cua driver", title: "Screen", surfaceSize: CGSize(width: 1280, height: 800),
                     appID: "cua-driver"),
        StreamWindow(id: "w-1", app: "Firefox", title: "Mozilla Firefox", appID: "firefox", processID: 812),
        StreamWindow(id: "w-2", app: "Firefox", title: "Release notes", appID: "firefox", processID: 812),
        StreamWindow(id: "w-3", app: "xterm", title: "", appID: "xterm", processID: 904),
        StreamWindow(id: "w-4", app: "gedit", title: longTitle, appID: "org.gnome.gedit"),
    ]

    /// A small PNG (a filled rounded square) standing in for an app icon's bytes.
    static func iconPNG(_ color: NSColor) -> Data {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 32, pixelsHigh: 32, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                   colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        color.setFill()
        NSBezierPath(roundedRect: NSRect(x: 2, y: 2, width: 28, height: 28), xRadius: 7, yRadius: 7).fill()
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])!
    }

    /// The SDK's batched lookup, recorded: one call per load.
    final class Icons: @unchecked Sendable {
        private let lock = NSLock()
        private var _calls: [[String]] = []
        var calls: [[String]] { lock.withLock { _calls } }
        func fetch(_ requests: [SpaceAppIconRequest]) -> [Data?] {
            lock.withLock { _calls.append(requests.map { "\($0.appName) \($0.appId) \($0.pid)" }) }
            return requests.map {
                switch $0.appName {
                case "Firefox": return StreamFixture.iconPNG(.systemOrange)
                case "gedit": return StreamFixture.iconPNG(.systemBlue)
                default: return nil // xterm: the Space has no icon for it
                }
            }
        }
    }

    @MainActor static func model(windows: [StreamWindow]? = StreamFixture.windows, fail: Bool = false,
                                 display: AppStreamDisplay? = AppStreamDisplay(widthPx: 1280, heightPx: 800),
                                 icons: Icons = Icons()) -> StreamRowsModel {
        StreamRowsModel(os: .linux, osName: "Ubuntu 24.04",
                        windows: {
                            if fail { throw FixtureError(message: "window host is not up") }
                            return windows ?? []
                        },
                        display: { display },
                        icons: { icons.fetch($0) })
    }
}

@MainActor
@Suite("Stream rows")
struct StreamRowsTests {
    /// A listing that never answers does not keep "Looking for this
    /// Space's windows…" up: past the bound it reads as failed, with the
    /// Desktop row still there.
    @Test func aHungWindowListingFailsInsteadOfLoadingForever() async {
        let saved = StreamRowsModel.windowsTimeout
        StreamRowsModel.windowsTimeout = 0.2
        defer { StreamRowsModel.windowsTimeout = saved }
        let m = StreamRowsModel(os: .linux, osName: "Ubuntu 24.04",
                                windows: {
                                    try await Task.sleep(for: .seconds(3600))
                                    return []
                                },
                                display: { nil }, icons: { _ in [] })
        let started = ContinuousClock.now
        await m.refresh()
        #expect(ContinuousClock.now - started < .seconds(5))
        #expect(m.failed)
        #expect(m.windows == [])
        #expect(m.section().rows.map(\.label) == ["Desktop"])
    }

    @Test func loadingUntilTheFirstList() {
        let m = StreamFixture.model()
        let s = m.section()
        #expect(s.rows.map(\.label) == ["Desktop"])
        #expect(s.statusText == "Looking for this Space\u{2019}s windows\u{2026}")
    }

    @Test func rowsComeFromTheCore() async {
        let m = StreamFixture.model()
        await m.refresh()
        let s = m.section()
        #expect(s.rows.map(\.label) == ["Desktop (1280\u{d7}800)", "Mozilla Firefox", "Release notes", "xterm",
                                         StreamFixture.longTitle])
        #expect(s.rows[0].resolution == "1280\u{d7}800")
        #expect(s.rows[0].icon == .os(id: "os-ubuntu"))
        #expect(s.rows[3].icon == .app(appName: "xterm", appId: "xterm", pid: 904))
        #expect(s.rows[4].help == StreamFixture.longTitle)
        #expect(s.rows.allSatisfy { $0.actions.map(\.symbol) == ["pip.enter"] })
        #expect(s.rows.allSatisfy { $0.actions.map(\.help) == ["Picture in picture"] })
        #expect(s.statusText == nil)
        #expect(m.window(id: "w-3")?.app == "xterm")
    }

    @Test func openPanelsShowAsActive() async {
        let m = StreamFixture.model()
        await m.refresh()
        let s = m.section(openKeys: [StreamSource.desktop.pipKey, StreamSource.window(StreamFixture.windows[3]).pipKey])
        #expect(s.rows.filter { $0.actions[0].active }.map(\.id) == ["desktop", "w-3"])
        #expect(s.rows[0].actions[0].symbol == "pip.exit")
    }

    @Test func iconsInOneBatchPerLoadAndNoneWhenMissing() async {
        let icons = StreamFixture.Icons()
        let m = StreamFixture.model(icons: icons)
        await m.refresh()
        await m.loadIcons()
        // One call, one request per app (Firefox's two windows share one).
        #expect(icons.calls == [["Firefox firefox 812", "xterm xterm 904", "gedit org.gnome.gedit 0"]])
        let s = m.section()
        #expect(m.image(for: s.rows[1].icon) != nil)
        #expect(m.image(for: s.rows[3].icon) == nil)
        #expect(m.image(for: s.rows[0].icon) == nil)
        // No cache here: the next load asks the SDK again (its cache answers).
        await m.loadIcons()
        #expect(icons.calls.count == 2)
    }

    @Test func aFailedListSaysSo() async {
        let m = StreamFixture.model(fail: true, display: nil)
        await m.refresh()
        let s = m.section()
        #expect(s.rows.map(\.label) == ["Desktop"])
        #expect(s.statusText == "No windows: the Space\u{2019}s window host is not up.")
    }

    @Test func fixtureBackendReportsADisplay() async {
        let d = await FixtureSpacesBackend().primaryDisplay(id: "local:aurora")
        #expect(d == AppStreamDisplay(widthPx: 1280, heightPx: 800))
    }
}
