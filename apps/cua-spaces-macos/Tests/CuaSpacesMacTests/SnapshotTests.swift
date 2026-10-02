// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
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
        m.onboarding.finish()
        await m.refresh()
        await m.keyvault.refresh()
        return m
    }

    @Test func onboardingWelcome() throws {
        let o = OnboardingModel(statePath: nil)
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding", size: CGSize(width: 820, height: 560))
    }

    /// Every first-run page (the Tauri app's pages, from the same core).
    @Test func onboardingPages() async throws {
        let o = OnboardingModel(statePath: nil, agentSetup: FixtureAgentSetup())
        let size = CGSize(width: 820, height: 560)
        // Every page, the Volume page included (its experiment on).
        o.send(.experimentsLoaded(experiments: AppExperiments(cuaVolume: true, yourCloud: false, sharing: false)))
        o.send(.start)
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-signin", size: size)
        o.send(.signinDone)
        await o.loadAgents()
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-agents", size: size)
        o.finishAgents(skipped: true)
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-presentation", size: size)
        o.send(.presentationDone)
        o.send(.driveChecked(os: .macos, status: try appDriveMountFromJson(
            json: #"{"enabled":false,"state":"off","method":"fskit","volume_name":"Cua Volume"}"#)))
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-drive", size: size)
        o.send(.driveContinue)
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-mode", size: size)
        o.send(.modeChosen(mode: .client))
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-done", size: size)
    }

    /// The AI agents page with twelve agents at the window's minimum size:
    /// the list scrolls in two columns, so Back and Set up stay in view.
    @Test func onboardingAgentsLongListAtMinimumSize() async throws {
        let names = ["Claude Code", "Codex", "Cursor", "Gemini CLI", "GitHub Copilot", "Windsurf", "Cline",
                     "Roo Code", "OpenCode", "Goose", "Amp", "Hermes"]
        let statuses = names.map { n in
            AppAgentSetupStatus(id: n.lowercased().replacingOccurrences(of: " ", with: "-"), name: n, installed: true,
                                skillsDir: "~/.agents/skills", mcpConfig: nil, cuaConfigured: false,
                                skillsInstalled: [], skillsOutdated: [], error: nil)
        }
        let o = OnboardingModel(statePath: nil, agentSetup: FixtureAgentSetup(statuses: statuses))
        o.send(.start)
        o.send(.signinDone)
        await o.loadAgents()
        #expect(o.installedAgents.count == 12)
        #expect(o.view.canBack)
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-agents-long", size: CGSize(width: 720, height: 520))
    }

    /// The AI agents page's background computer-use card at beats of its
    /// loop (the agent gliding while the user drags a selection, the agent
    /// clicking a box, both idle at the loop's end), ticked, and the Reduce
    /// Motion still.
    @Test func driverCard() throws {
        let copy = appOnboardingCopy()
        func card(_ ms: UInt32?, on: Bool = false, still: Bool? = nil) -> some View {
            DriverCardView(title: copy.agentsDriver, imageLabel: copy.agentsDriverImage, isOn: .constant(on),
                           fixedMs: ms, still: still)
                .padding(12)
        }
        let size = CGSize(width: 380, height: 180)
        for (ms, name) in [(UInt32(1500), "working"), (1750, "click"), (4799, "idle")] {
            try assertSnapshot(card(ms), "onboarding-driver-\(name)", size: size)
        }
        try assertSnapshot(card(nil, on: true, still: true), "onboarding-driver-still-on", size: size)
        let still = appDriverPreviewStill()
        #expect(still.ripple > 0 && still.checked[0] == 1 && !still.agentPressed)
    }

    /// The presentation cards' miniatures (both cards) at beats of the loop:
    /// the pointer gliding up, the notch's hover cue, the notch springing
    /// open while the menu drops down, both expanded; then the Reduce Motion
    /// still (the expanded state, no loop).
    @Test func presentationPreviews() async throws {
        let o = OnboardingModel(statePath: nil, agentSetup: FixtureAgentSetup())
        o.send(.start)
        o.send(.signinDone)
        o.finishAgents(skipped: true)
        let cards = o.view.presentations
        #expect(cards.count == 2)
        func pair(_ ms: UInt32?, still: Bool? = nil) -> some View {
            VStack(spacing: 10) {
                ForEach(cards, id: \.id) { PresentationCardView(card: $0, fixedMs: ms, still: still) {} }
            }
            .padding(12)
        }
        let size = CGSize(width: 320, height: 300)
        for (ms, name) in [(UInt32(900), "glide"), (1550, "dwell"), (1760, "opening"), (2400, "open")] {
            try assertSnapshot(pair(ms), "onboarding-preview-\(name)", size: size)
        }
        try assertSnapshot(pair(nil, still: true), "onboarding-preview-still", size: size)
        let still = [false, true].map { appPresentationPreviewStill(menuBar: $0) }
        #expect(still[0].open == 1 && still[0].content == 1, "the notch open")
        #expect(still[1].open == 1 && still[1].highlighted != nil, "the menu down")
    }

    @Test func newSpaceWizard() async throws {
        let m = try await model()
        await m.openNewSpace()
        try assertSnapshot(NewSpaceWizardView(wizard: m.wizard, onCreate: { _ in }, onAdd: { _, _, _ in },
                                              onCancel: {}), "new-space", size: CGSize(width: 560, height: 520))
    }

    /// The Space detail's Stream section: Desktop with the OS mark and the
    /// resolution, window rows by title with their app icons (none for
    /// xterm), a truncated long title, one panel open.
    @Test func streamSection() async throws {
        let m = StreamFixture.model()
        await m.refresh()
        await m.loadIcons()
        let open: Set<String> = [StreamSource.window(StreamFixture.windows[2]).pipKey]
        let view = Form {
            Section("Stream") {
                StreamRowList(section: m.section(openKeys: open), image: m.image(for:)) { _ in }
            }
        }
        .formStyle(.grouped)
        try assertSnapshot(view, "stream-section", size: CGSize(width: 520, height: 280))
    }

    /// The detail's facts from the core with live usage: Image (copy),
    /// the full System string, Memory, Identifier (copy), before and right
    /// after a copy (never the real pasteboard).
    @Test func spaceFacts() async throws {
        let backend = FixtureSpacesBackend()
        let space = appRowsToSpaces(rows: FixtureSpacesBackend.sample, nowMs: 0)[0]
        let usage = await backend.usage(id: space.id)
        let facts = appSpaceDetailLive(space: space, usage: usage, hostArch: nil).facts
        let size = CGSize(width: 520, height: 300)
        func form(_ copied: Bool) -> some View {
            Form { Section { FactRows(facts: facts, write: { _ in }, copied: copied) } }.formStyle(.grouped)
        }
        try assertSnapshot(form(false), "space-facts", size: size)
        try assertSnapshot(form(true), "space-facts-copied", size: size)
    }

    /// The live desktop's card: its content runs flush to the card's edges
    /// (no row inset, no letterbox) at the default desktop's 16:10, with
    /// only the card's corner radius, above the facts card.
    @Test func spacePreview() async throws {
        let space = appRowsToSpaces(rows: FixtureSpacesBackend.sample, nowMs: 0)[0]
        let facts = appSpaceDetail(space: space).facts
        let view = Form {
            Section {
                PreviewCard(session: nil) {
                    LinearGradient(colors: [.indigo, .orange], startPoint: .top, endPoint: .bottom)
                }
            }
            Section { FactRows(facts: facts, write: { _ in }, copied: false) }
        }
        .formStyle(.grouped)
        try assertSnapshot(view, "space-preview", size: CGSize(width: 520, height: 480))
    }

    /// The preview card while the stream opens and while it waits for
    /// Connect: the Space's thumbnail blurred and dimmed with the core's
    /// words centered, or plain black when there is no thumbnail yet.
    @Test func desktopCover() async throws {
        let m = try await model()
        let space = try #require(m.spaces.first { m.detail($0).canStream })
        let detail = m.detail(space)
        let preview = DesktopCoverTests.desktop()
        func card(_ cover: AppDesktopCover, _ image: NSImage?) -> some View {
            Form {
                Section {
                    PreviewCard(session: nil) { DesktopCoverView(cover: cover, image: image) }
                }
            }
            .formStyle(.grouped)
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
            stream: .noSession))
        try assertSnapshot(card(stopped, preview), "space-cover-stopped", size: size)
    }

    /// Settings, General: "Connect to the desktop automatically", on by
    /// default (the `settings` reference), then off.
    @Test func settingsAutoConnectOff() async throws {
        let m = try await model()
        m.settings.experiments.cuaVolume = true
        await m.choose(row: "auto-connect", option: "off")
        await m.loadSettings()
        try assertSnapshot(SettingsView(model: m), "settings-auto-connect-off", size: CGSize(width: 520, height: 640))
    }

    /// "Teleport an app…" as the core's grid: sections, a tile previewing
    /// its app's frontmost window, icons, the app that cannot move dimmed.
    @Test func teleportPickerGrid() async throws {
        let m = TeleportModel(spaceName: "Aurora", teleport: nil, space: nil, sources: PickerFixture.sources())
        m.send(.loaded(entries: try PickerFixture.entries()))
        await m.loadWindows()
        for tile in m.grid.sections.flatMap(\.tiles) { await m.loadThumbnail(tile) }
        try assertSnapshot(TeleportPickerSheet(teleport: m, onClose: {}), "teleport-picker-grid",
                           size: CGSize(width: 640, height: 540))
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

    /// "Signed in" beside the Space a Keyvault sign-in is live in.
    @Test func mainWindowSidebarSignedIn() async throws {
        let m = try await model(overview: try signedInOverview())
        m.select("cloud:builder")
        try assertSnapshot(Sidebar(model: m).frame(width: 260), "sidebar-signed-in", size: CGSize(width: 260, height: 520))
    }

    /// Access from that badge: the Space's copy brought forward, kept until
    /// wiped (auto-wipe off), with Dismiss beside Wipe.
    @Test func keyvaultAccessFocused() async throws {
        let m = try await model(overview: try signedInOverview())
        m.showAccess(spaceId: "local:aurora")
        m.keyvault.selection = .category(category: .access)
        #expect(m.keyvault.focusKey == "d:local:aurora")
        try assertSnapshot(CategoryList(keyvault: m.keyvault, page: m.keyvault.page), "keyvault-access",
                           size: CGSize(width: 640, height: 360))
    }

    /// Settings, Keyvault: the auto-wipe switch, off by default.
    @Test func settingsKeyvault() async throws {
        let m = try await model(overview: try signedInOverview())
        await m.loadSettings()
        #expect(m.settingsPage.sections.contains { $0.id == "keyvault" })
        try assertSnapshot(SettingsView(model: m), "settings-keyvault", size: CGSize(width: 520, height: 1000))
    }

    /// The teleport run says which step it is on: reading the browser's
    /// cookies (naming the Keychain prompt), then the upload's bytes.
    @Test func teleportRunning() async throws {
        let m = TeleportModel(spaceName: "Aurora", teleport: nil, space: nil, sources: PickerFixture.sources())
        let flow = try PickerFixture.json("teleport-review")
        func json(_ v: Any?) throws -> String {
            String(decoding: try JSONSerialization.data(withJSONObject: v!), as: UTF8.self)
        }
        m.send(.loaded(entries: try appCatalogEntriesFromJson(json: json(flow["entries"]))))
        m.send(.select(id: "slack"))
        m.send(.choose(id: nil))
        m.send(.move(moves: .appWithState))
        m.send(.plan)
        m.send(.planned(plan: try appTeleportPlanFromJson(json: json(flow["plan"]))))
        m.send(.acknowledge(value: true))
        m.send(.acknowledgeRelayPlaintext(value: true))
        m.send(.confirm)
        func ev(_ detail: String, _ done: UInt64 = 0, _ total: UInt64 = 0) -> AppPickerEvent {
            .progress(event: AppTeleportRunEvent(step: 1, steps: 3, kind: "state", phase: .progress, detail: detail,
                                                 doneBytes: done, totalBytes: total))
        }
        #expect(m.state.step == .running)
        let size = CGSize(width: 640, height: 540)
        m.send(ev("Reading Slack cookies (macOS will ask for Keychain access)\u{2026}"))
        try assertSnapshot(TeleportPickerSheet(teleport: m, onClose: {}), "teleport-running-reading", size: size)
        m.send(ev("Uploading", 12 << 20, 80 << 20))
        try assertSnapshot(TeleportPickerSheet(teleport: m, onClose: {}), "teleport-running-upload", size: size)
    }

    @Test func mainWindowSidebar() async throws {
        let m = try await model()
        m.select("local:aurora")
        try assertSnapshot(Sidebar(model: m).frame(width: 260), "sidebar", size: CGSize(width: 260, height: 520))
    }

    /// The power button next to Delete on the rows: a suspended Space
    /// (selected: Resume), a macOS VM turning off (a spinner), and one whose
    /// turn-off failed (why, inline).
    @Test func mainWindowSidebarPower() async throws {
        func row(_ id: String, _ os: AppSpaceOs, _ osName: String, power: String, state: String,
                 reachable: Bool) -> AppSpaceRow {
            AppSpaceRow(id: id, name: String(id.dropFirst("local:".count)), provider: "local",
                        spacesdVersion: "0.4.0", features: ["desktop_stream"], addedAt: "2026-09-25T08:00:00Z",
                        os: os, osName: osName, osPrettyName: nil, image: nil, imageDigest: nil,
                        kind: os == .macos ? .vm : .container, arch: "arm64", reachable: reachable,
                        error: nil, host: nil, hostName: nil, power: power, powerState: state,
                        cloud: nil, cloudPlace: nil, cloudDelete: nil)
        }
        let backend = FixtureSpacesBackend(rows: [
            row("local:aurora", .linux, "Ubuntu", power: "suspend", state: "suspended", reachable: false),
            row("local:mac-studio", .macos, "macOS", power: "stop", state: "running", reachable: true),
            row("local:build-box", .linux, "Debian", power: "suspend", state: "running", reachable: true),
        ])
        let m = ViewModelTests().makeModel(backend, kv: FakeKeyvault(try fixtureOverview()),
                                           host: FixtureHost(), account: FixtureAccount(),
                                           agents: FixtureAgentSetup())
        m.onboarding.finish()
        await m.refresh()
        await m.keyvault.refresh()
        m.select("local:aurora")
        backend.powerError = "Docker is not running"
        m.setPower(try #require(m.spaces.first { $0.id == "local:build-box" }), on: false)
        for _ in 0..<200 where m.creates.powering.first?.error == nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        backend.holdPower = true
        m.setPower(try #require(m.spaces.first { $0.id == "local:mac-studio" }), on: false)
        try assertSnapshot(Sidebar(model: m).frame(width: 260), "sidebar-power", size: CGSize(width: 260, height: 520))
        backend.releasePower()
    }

    @Test func thisMachine() async throws {
        let m = try await model()
        await m.host.refresh()
        // Before setup: the explainer and both choices inline.
        #expect(m.host.panel.intro != nil)
        #expect(m.host.panel.setupChoices.map(\.id) == ["desktop", "spare"])
        try assertSnapshot(ThisMachineView(host: m.host), "this-machine-unset", size: CGSize(width: 640, height: 420))
        m.host.openForm()
        try assertSnapshot(ThisMachineView(host: m.host), "host-setup", size: CGSize(width: 640, height: 420))
        m.host.send(.setName(name: "Studio"))
        await m.host.submit()
        try assertSnapshot(ThisMachineView(host: m.host), "this-machine", size: CGSize(width: 640, height: 520))
    }

    @Test func settings() async throws {
        let m = try await model()
        // With Storage (the Cua Volume experiment on), as recorded.
        m.settings.experiments.cuaVolume = true
        await m.loadSettings()
        try assertSnapshot(SettingsView(model: m), "settings", size: CGSize(width: 520, height: 640))
    }

    /// Launch at login waiting for approval in System Settings (a fake
    /// login item; the Mac's own is never touched).
    @Test func settingsLoginItemApproval() async throws {
        let item = FixtureLoginItem()
        item.approval = true
        let m = try await model(loginItem: item)
        #expect(item.calls == ["register"], "the first run's Done registered it")
        m.settings.experiments.cuaVolume = true
        await m.loadSettings()
        try assertSnapshot(SettingsView(model: m), "settings-login-approval", size: CGSize(width: 520, height: 640))
    }

    @Test func keyvaultAll() async throws {
        let m = try await model()
        let sel = KvSelection.category(category: .all)
        m.keyvault.selection = sel
        try assertSnapshot(VaultList(keyvault: m.keyvault, page: m.keyvault.page), "keyvault-all",
                           size: CGSize(width: 640, height: 520))
    }

    @Test func approvalSheet() async throws {
        let m = try await model()
        m.keyvault.openApproval("req-1")
        m.keyvault.sendApproval(.toggle(key: "chrome|example.test"))
        try assertSnapshot(ApprovalSheet(keyvault: m.keyvault), "approval", size: CGSize(width: 480, height: 360))
    }

    /// Settings → Devices on an account with this Mac enrolled, a new
    /// device and an expired one asking, and recent access.
    @Test func devicesPage() async throws {
        let d = DevicesTests().model()
        await d.refresh()
        d.approval = nil
        try assertSnapshot(DevicesSettingsView(devices: d), "devices", size: CGSize(width: 520, height: 700))
    }

    /// A new device asking: its name and the code it shows, Approve (after
    /// Touch ID) and Deny; then this Mac waiting with its own code.
    @Test func deviceSheets() async throws {
        let d = DevicesTests().model()
        await d.refresh()
        d.setCode("k7qxm2rp")
        try assertSnapshot(DeviceApprovalSheet(devices: d), "device-approval", size: CGSize(width: 420, height: 250))
        var snap = FixtureDevices.sample(now: DevicesTests.now)
        snap.devices[0].state = "pending"
        let waiting = DevicesTests().model(FixtureDevices(snapshot: snap))
        waiting.pollLimit = 1
        waiting.startEnroll(in: .main)
        await waiting.chooseEnroll(.approve)
        try assertSnapshot(EnrollSheet(devices: waiting), "device-enroll-code", size: CGSize(width: 440, height: 300))
    }

    /// The notch panel in each state, settled, at its stage size (the
    /// 14-inch MacBook Pro layout; the view draws on a clear panel).
    func assertNotch(_ name: String, _ setup: (NotchModel) -> Void) async throws {
        let m = try await model()
        setup(m.notch)
        let c = NotchController(model: m.notch)
        c.apply(appNotchLayout(screen: NotchGeometry.fallbackScreen, prompt: NotchController.needsRow(m.notch.view)))
        let g = c.geometry
        try assertSnapshot(NotchContentView(model: m.notch, controller: c).background(Color(white: 0.85)),
                           name, size: g.stage)
    }

    @Test func notchClosed() async throws {
        try await assertNotch("notch-closed") { _ in }
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
            s.id = "cloud:starter"
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
            AppSpaceRow(id: id, name: name, provider: provider, spacesdVersion: "0.4.0",
                        features: ["desktop_stream"], addedAt: "2026-09-25T08:00:00Z",
                        os: os, osName: osName, osPrettyName: nil, image: nil, imageDigest: nil,
                        kind: nil, arch: "arm64", reachable: true, error: nil, host: host, hostName: hostName,
                        power: nil, powerState: nil, cloud: nil, cloudPlace: nil, cloudDelete: nil)
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
        c.apply(appNotchLayout(screen: NotchGeometry.fallbackScreen, prompt: NotchController.needsRow(m.notch.view)))
        try assertSnapshot(NotchContentView(model: m.notch, controller: c).background(Color(white: 0.85)),
                           "notch-tile-locations", size: c.geometry.stage)
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

    /// The host form inside onboarding: its Back and Set up take the
    /// bottom corners like every other page.
    @Test func onboardingHostForm() async throws {
        let o = OnboardingModel(statePath: nil, host: FixtureHost(), agentSetup: FixtureAgentSetup())
        // The page dots as recorded (the Cua Volume experiment on).
        o.send(.experimentsLoaded(experiments: AppExperiments(cuaVolume: true, yourCloud: false, sharing: false)))
        o.send(.start)
        o.send(.signinDone)
        o.finishAgents(skipped: true)
        o.send(.presentationDone)
        o.send(.driveContinue)
        o.host.openForm()
        try assertSnapshot(OnboardingView(onboarding: o), "onboarding-host-form", size: CGSize(width: 820, height: 560))
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
