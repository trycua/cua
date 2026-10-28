// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import CuaSpacesNotchHelper
import CuaSpacesNotchUI
import CuaSpacesStreaming
@testable import CuaSpacesMacKit
import SwiftUI
import Testing

/// Snapshot tests of the main views on fixtures: each view renders
/// offscreen (never ordered front) and is compared with its reference in
/// Snapshots/ within a small tolerance. `SNAPSHOT_RECORD=1` (or a missing
/// reference) records; the test then fails so a recording is never a pass.
@MainActor
@Suite("Snapshots", .serialized)
struct SnapshotTests {
    static let snapshots = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent("Snapshots")
    /// The share of sampled pixels that may differ. Text rasterizes a little
    /// differently across macOS builds: the macos-26 runner's renders of
    /// references recorded on a developer Mac differ in 2.4 to 3.5% of pixels,
    /// all glyph edges and a sub-point baseline shift, with identical layout.
    static let tolerance = 0.05

    init() { _ = NSApplication.shared }

    func model(kv: Bool = true, loginItem: FixtureLoginItem = FixtureLoginItem(),
               overview: KeyvaultOverview? = nil) async throws -> AppModel {
        let m = ViewModelTests().makeModel(FixtureSpacesBackend(),
                                           kv: kv ? FakeKeyvault(try overview ?? fixtureOverview()) : nil,
                                           host: FixtureHost(), account: FixtureAccount(),
                                           agents: FixtureAgentSetup(), loginItem: loginItem)
        // Done's "Launch at login" ticked, as the first run's default had it.
        m.onboarding.finish(launchAtLogin: true)
        await m.refresh()
        await m.keyvault.refresh()
        return m
    }

    /// A Space's window while the stream opens and while it waits for
    /// Connect: the Space's thumbnail blurred and dimmed with the core's
    /// words centered, or plain black when there is no thumbnail yet.
    @Test func desktopCover() async throws {
        let m = try await model()
        let space = try #require(m.spaces.first { m.detail($0).canStream })
        let detail = m.detail(space)
        let preview = DesktopCoverTests.desktop()
        func card(_ cover: AppDesktopCover, _ image: NSImage?) -> some View {
            DesktopCoverView(cover: cover, image: image)
        }
        let size = CGSize(width: 520, height: 360)
        let connecting = m.cover(detail, requested: false, stream: .noSession)
        #expect(connecting.kind == .connecting)
        try assertSnapshot(card(connecting, preview), "space-cover-connecting", size: size)
        try assertSnapshot(card(connecting, nil), "space-cover-connecting-black", size: size)
        await m.choose(row: "auto-connect", option: "off")
        let manual = m.cover(detail, requested: false, stream: .noSession)
        #expect(manual.kind == .connect)
        try assertSnapshot(card(manual, preview), "space-cover-connect", size: size)
        try assertSnapshot(card(manual, nil), "space-cover-connect-black", size: size)
        // A Space that cannot stream: its line on the same blurred preview.
        let stopped = appDesktopCover(input: AppDesktopCoverInput(
            canStream: false, previewText: "Stopped", autoConnect: true, connectRequested: false,
            stream: .noSession, access: nil))
        try assertSnapshot(card(stopped, preview), "space-cover-stopped", size: size)
    }

    /// A Keyvault sign-in live in Aurora (auto-wipe off: no expiry).
    func signedInOverview() throws -> KeyvaultOverview {
        var o = try fixtureOverview()
        o.status?.autoWipe = false
        o.deliveries = [KvDelivery(importId: "imp-aurora", target: "local:aurora", providerId: "chrome",
                                   items: ["gh-ada"], callerFp: "fp-koala", deliveredMs: 1_799_999_000_000,
                                   expiresMs: 0, wiped: false)]
        return o
    }

    /// Settings, Keyvault (the page's, over this model): its section is there.
    @Test func settingsKeyvault() async throws {
        let m = try await model(overview: try signedInOverview())
        await m.loadSettings()
        #expect(m.settingsPage.sections.contains { $0.id == "keyvault" })
    }

    /// Launch at login waiting for approval in System Settings (a fake
    /// login item; the Mac's own is never touched).
    @Test func settingsLoginItemApproval() async throws {
        let item = FixtureLoginItem()
        item.approval = true
        _ = try await model(loginItem: item)
        #expect(item.calls == ["register"], "the first run's Done registered it")
    }

    /// The notch panel in each state, settled, at its stage size (the
    /// 14-inch MacBook Pro layout; the view draws on a clear panel).
    func assertNotch(_ name: String, _ setup: (NotchModel) -> Void) async throws {
        let m = try await model()
        setup(m.notch)
        let c = NotchController(model: m.notch)
        c.apply(appNotchLayout(screen: NotchGeometry.fallbackScreen.ffi, prompt: NotchController.needsRow(m.notch.view)))
        let g = c.geometry
        try assertSnapshot(NotchContentView(surface: c).background(Color(white: 0.85)),
                           name, size: g.stage)
        try assertHelperNotch(m.notch, name)
    }

    /// The Electron app's notch helper draws the same pixels for the same
    /// core state: the state goes through the notch protocol as JSON (what
    /// apps/cua-spaces-desktop/src/notch sends) into the helper's surface,
    /// and its render is compared with the SwiftUI app's reference.
    func assertHelperNotch(_ notch: NotchModel, _ name: String) throws {
        let core = notch.view
        var view = core.data
        // Electron fills a ring's estimate (the helper has no core).
        if var a = view.activity, a.permille == nil {
            let start = a.startedAt ?? Int64(Date().timeIntervalSince1970 * 1000)
            a.permille = appNotchEstimatedProgress(elapsedMs: Int64(Date().timeIntervalSince1970 * 1000) - start,
                                                   estimateMs: a.estimateMs)
            view.activity = a
        }
        let layout = appNotchLayout(screen: NotchGeometry.fallbackScreen.ffi, prompt: NotchController.needsRow(core)).data
        var icons: [String: NotchData.OsIcon] = [:]
        for t in view.tiles { icons[t.symbol] = NotchModel.osIcon(t.symbol) }
        let state = HelperStateLine(view: view, query: notch.state.query, layout: layout, shown: true,
                                    dragging: notch.state.drag.phase != .idle, icons: icons,
                                    highlight: notch.highlight.map(Self.raw))
        let hello = HelperHelloLine(motion: NotchModel.motion.data, radii: NotchModel.radii)
        let helper = HelperNotch(emit: { _ in })
        helper.makesPanel = false
        let encoder = JSONEncoder()
        #expect(helper.receive(try HostMessage.decode(try encoder.encode(hello))))
        #expect(helper.receive(try HostMessage.decode(try encoder.encode(state))))
        for tile in view.tiles {
            guard let image = notch.thumbnails[tile.id], let tiff = image.tiffRepresentation,
                  let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) else { continue }
            let line = #"{"type":"thumbnail","id":"\#(tile.id)","image":"\#(png.base64EncodedString())"}"#
            #expect(helper.receive(try HostMessage.decode(Data(line.utf8))))
        }
        #expect(helper.view == view)
        try assertSnapshot(NotchContentView(surface: helper).background(Color(white: 0.85)),
                           name, size: helper.geometry.stage)
    }

    /// A forced look as the protocol's `highlight` text.
    static func raw(_ h: NotchHighlight) -> String {
        let target: String
        switch h.control {
        case .tile(let id): target = "tile=\(id)"
        case .button(let b): target = b.rawValue
        case .search: target = "search"
        case .tab: target = "tab"
        }
        return h.pressed ? target + ":pressed" : target
    }

    @Test func notchClosed() async throws {
        try await assertNotch("notch-closed") { _ in }
    }

    /// Every Space stopped: nothing on, so no tab beside the notch.
    @Test func notchClosedNoneOn() async throws {
        try await assertNotch("notch-closed-none-on") { n in
            n.spaces = n.spaces.map { s in
                var s = s
                if s.id != "this-mac" { s.status = .suspended }
                return s
            }
        }
    }

    @Test func notchHover() async throws {
        try await assertNotch("notch-hover") { $0.send(.hoverEnter) }
    }

    @Test func notchExpanded() async throws {
        try await assertNotch("notch-tiles") { $0.send(.click) }
    }

    @Test func notchPermission() async throws {
        try await assertNotch("notch-permission") {
            $0.send(.dragPermission(granted: false))
            $0.send(.click)
        }
    }

    @Test func notchDragTarget() async throws {
        try await assertNotch("notch-drag-target") {
            $0.send(.drag(event: .start(windowId: nil, appName: "Notes")))
        }
    }

    @Test func notchDropPanel() async throws {
        try await assertNotch("notch-drop") {
            $0.send(.drag(event: .start(windowId: nil, appName: "Notes")))
            $0.send(.drag(event: .enterNotch))
            $0.send(.drag(event: .over(spaceId: "local:aurora")))
        }
    }

    @Test func notchSearch() async throws {
        try await assertNotch("notch-search") {
            $0.send(.click)
            $0.search("win")
        }
    }

    /// A Space starting: the ring left of the notch.
    @Test func notchActivityProvisioning() async throws {
        try await assertNotch("notch-activity-provisioning") { n in
            var s = n.spaces.first { $0.id != "this-mac" }!
            s.id = "direct:starter"
            s.name = "Starter"
            s.status = .provisioning
            s.startedAt = Int64(Date().timeIntervalSince1970 * 1000) - 40_000
            n.spaces.append(s)
        }
    }

    /// Live Keyvault access: the key left of the notch; open, the line with
    /// Dismiss and the key on the Space's tile.
    @Test func notchAccess() async throws {
        try await assertNotch("notch-activity-keyvault") {
            $0.setKeyvault(label: "Keyvault sign-ins live in local:aurora", signedIn: ["local:aurora"])
        }
        try await assertNotch("notch-access") {
            $0.setKeyvault(label: "Keyvault sign-ins live in local:aurora", signedIn: ["local:aurora"])
            $0.send(.click)
        }
    }

    @Test func notchActivityTransfer() async throws {
        try await assertNotch("notch-activity-transfer") {
            $0.setActivity(hotspot: false, transfer: AppNotchTransfer(sent: 600, total: 1000))
        }
    }

    @Test func notchActivityHotspot() async throws {
        try await assertNotch("notch-activity-hotspot") {
            $0.setActivity(hotspot: true, transfer: nil)
        }
    }

    // MARK: - Notch hover and pressed states (forced; no input replayed)

    @Test func notchTileHover() async throws {
        try await assertNotch("notch-tile-hover") {
            $0.send(.click)
            $0.highlight = NotchHighlight(.tile("local:aurora"))
        }
    }

    @Test func notchTilePressed() async throws {
        try await assertNotch("notch-tile-pressed") {
            $0.send(.click)
            $0.highlight = NotchHighlight(.tile("local:aurora"), pressed: true)
        }
    }

    @Test func notchButtonHover() async throws {
        try await assertNotch("notch-button-hover") {
            $0.send(.click)
            $0.highlight = NotchHighlight(.button(.list))
        }
    }

    /// Each tile's header: the OS logo, then where the Space runs in grey.
    /// A Space on this Mac, one on another of your machines, and one on a
    /// machine with a long name (truncated in the middle).
    @Test func notchTileLocations() async throws {
        func row(_ id: String, _ name: String, _ os: AppSpaceOs, _ osName: String, provider: String,
                 host: String? = nil, hostName: String? = nil) -> AppSpaceRow {
            AppSpaceRow(id: id, name: name, displayName: nil, provider: provider, spacesdVersion: "0.4.0",
                        features: ["desktop_stream"], addedAt: "2026-09-25T08:00:00Z",
                        os: os, osName: osName, osPrettyName: nil, image: nil, imageDigest: nil,
                        kind: nil, arch: "arm64", reachable: true, error: nil, host: host, hostName: hostName,
                        power: nil, powerState: nil, cloud: nil, cloudPlace: nil, cloudDelete: nil, stopReason: nil, sharing: nil)
        }
        let backend = FixtureSpacesBackend(rows: [
            row("local:dev", "Dev box", .linux, "Ubuntu", provider: "local"),
            row("relay:m1/studio", "Studio", .macos, "macOS", provider: "relay", host: "m1", hostName: "Mac mini"),
            row("relay:m2/ci", "CI runner", .windows, "Windows", provider: "relay", host: "m2",
                hostName: "Dillon's Mac Studio in the Back Office Rack 3"),
        ])
        let m = ViewModelTests().makeModel(backend, kv: FakeKeyvault(try fixtureOverview()),
                                           host: FixtureHost(), account: FixtureAccount(),
                                           agents: FixtureAgentSetup())
        m.onboarding.finish()
        await m.refresh()
        m.notch.send(.click)
        let places = Dictionary(uniqueKeysWithValues: m.notch.view.tiles.map { ($0.id, $0.location) })
        #expect(places["local:dev"] == "This Mac")
        #expect(places["relay:m1/studio"] == "Mac mini")
        #expect(places["relay:m2/ci"] == "Dillon's Mac Studio in the Back Office Rack 3")
        let c = NotchController(model: m.notch)
        c.apply(appNotchLayout(screen: NotchGeometry.fallbackScreen.ffi, prompt: NotchController.needsRow(m.notch.view)))
        try assertSnapshot(NotchContentView(surface: c).background(Color(white: 0.85)),
                           "notch-tile-locations", size: c.geometry.stage)
        try assertHelperNotch(m.notch, "notch-tile-locations")
    }

    @Test func notchButtonPressed() async throws {
        try await assertNotch("notch-button-pressed") {
            $0.send(.click)
            $0.highlight = NotchHighlight(.button(.settings), pressed: true)
        }
    }

    @Test func notchSearchHover() async throws {
        try await assertNotch("notch-search-hover") {
            $0.send(.click)
            $0.highlight = NotchHighlight(.search)
        }
    }

    @Test func notchTabHover() async throws {
        try await assertNotch("notch-tab-hover") { $0.highlight = NotchHighlight(.tab) }
    }

    @Test func notchTabPressed() async throws {
        try await assertNotch("notch-tab-pressed") { $0.highlight = NotchHighlight(.tab, pressed: true) }
    }

    // MARK: - Harness

    func render<V: View>(_ view: V, size: CGSize) -> NSBitmapImageRep {
        let host = NSHostingView(rootView: view.frame(width: size.width, height: size.height)
            .environment(\.colorScheme, .light))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.4))
        host.layoutSubtreeIfNeeded()
        // A fixed 2x device-RGB bitmap, not the window's backing store: the
        // references must not depend on the host display's scale (CI runners
        // are 1x) or color profile.
        let scale: CGFloat = 2
        let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: Int((size.width * scale).rounded()),
            pixelsHigh: Int((size.height * scale).rounded()),
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        rep.size = size
        host.cacheDisplay(in: host.bounds, to: rep)
        return rep
    }

    func assertSnapshot<V: View>(_ view: V, _ name: String, size: CGSize) throws {
        let rep = render(view, size: size)
        let png = rep.representation(using: .png, properties: [:])!
        let url = Self.snapshots.appendingPathComponent("\(name).png")
        // A copy of every render for review (outside the repo).
        if let dir = ProcessInfo.processInfo.environment["SNAPSHOT_EXPORT_DIR"], !dir.isEmpty {
            let out = URL(fileURLWithPath: dir)
            try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
            try? png.write(to: out.appendingPathComponent("\(name).png"))
        }
        let record = ProcessInfo.processInfo.environment["SNAPSHOT_RECORD"] == "1"
        guard !record, let reference = NSBitmapImageRep(data: (try? Data(contentsOf: url)) ?? Data()) else {
            try png.write(to: url)
            Issue.record("recorded \(url.lastPathComponent); run again to compare")
            return
        }
        let diff = difference(rep, reference)
        if diff > Self.tolerance {
            try png.write(to: Self.snapshots.appendingPathComponent("\(name).actual.png"))
        }
        #expect(diff <= Self.tolerance, "\(name) differs from its reference in \(Int(diff * 100))% of pixels")
    }

    /// The fraction of pixels whose channels differ by more than a small
    /// amount (anti-aliasing and font hinting differ across machines).
    func difference(_ a: NSBitmapImageRep, _ b: NSBitmapImageRep) -> Double {
        guard a.pixelsWide == b.pixelsWide, a.pixelsHigh == b.pixelsHigh else { return 1 }
        var differing = 0
        let step = 2
        var total = 0
        for y in stride(from: 0, to: a.pixelsHigh, by: step) {
            for x in stride(from: 0, to: a.pixelsWide, by: step) {
                total += 1
                guard let ca = a.colorAt(x: x, y: y), let cb = b.colorAt(x: x, y: y) else { continue }
                let d = abs(ca.redComponent - cb.redComponent) + abs(ca.greenComponent - cb.greenComponent)
                    + abs(ca.blueComponent - cb.blueComponent)
                if d > 0.3 { differing += 1 }
            }
        }
        return total == 0 ? 0 : Double(differing) / Double(total)
    }
}

/// The `hello` line Electron sends.
struct HelperHelloLine: Encodable {
    var type = "hello"
    var v = NotchProtocol.version
    var motion: NotchData.Motion
    var radii: NotchData.RadiiPair
}

/// The `state` line Electron sends.
struct HelperStateLine: Encodable {
    var type = "state"
    var view: NotchData.View
    var query: String
    var layout: NotchData.Layout
    var shown: Bool
    var dragging: Bool
    var icons: [String: NotchData.OsIcon]
    var highlight: String?
}
