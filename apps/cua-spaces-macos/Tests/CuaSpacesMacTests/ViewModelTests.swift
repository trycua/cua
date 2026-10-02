// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
import Testing

/// A broker stand-in: returns a fixture overview and records the commands
/// the page sends. Fixture metadata only; there are no values anywhere.
final class FakeKeyvault: KeyvaultClientProtocol, @unchecked Sendable {
    var current: KeyvaultOverview
    var commands: [KvCommand] = []

    init(_ o: KeyvaultOverview) { current = o }

    func overview() async -> KeyvaultOverview { current }

    /// The icons the vault holds (the browser's own, read locally).
    var favicons: [KvFavicon] = []
    func favicons() async -> [KvFavicon] { favicons }

    func execute(command: KvCommand) async throws -> KvOutcome {
        commands.append(command)
        switch command {
        case .setDisabled(let disabled): current.status?.disabled = disabled
        case .setAutoWipe(let on): current.status?.autoWipe = on
        case .approve(let id, _), .deny(let id): current.pending.removeAll { $0.id == id }
        case .setLocked(let ids, let locked):
            // The broker's rule: an identity provider always asks.
            var changed: [String] = [], skipped: [String] = []
            for i in current.items.indices where ids.contains(current.items[i].id) {
                if !locked && current.items[i].identityProvider { skipped.append(current.items[i].id); continue }
                if current.items[i].policy.unattended == locked { changed.append(current.items[i].id) }
                current.items[i].policy.unattended = !locked
            }
            return .locked(changed: changed, skipped: skipped)
        case .deleteItems(let ids):
            current.items.removeAll { ids.contains($0.id) }
            current.itemsTotal = UInt32(current.items.count)
            let wiped = current.deliveries.filter { d in d.items.contains { ids.contains($0) } }.map(\.importId)
            for i in current.deliveries.indices where wiped.contains(current.deliveries[i].importId) {
                current.deliveries[i].wiped = true
            }
            return .deleted(count: UInt32(ids.count), wiped: wiped)
        case .setSkipUnlockPrompt(let on): current.status?.skipUnlockPrompt = on
        case .browse:
            current.namesVisible = true
            current.items = KeyvaultFixtures.items().filter { n in current.items.contains { $0.id == n.id } }
                .map { n in var m = n; m.policy = current.items.first { $0.id == n.id }!.policy; return m }
            return .browsing(untilMs: UInt64(KeyvaultFixtures.now + 300_000))
        default: break
        }
        return .done
    }

    /// What the Keyvault would list for an app (the review's sites).
    var inventoryResult: KvInventory = KeyvaultFixtures.chromeInventory()
    var inventoryFails = false
    var inventoryAsks = 0

    func inventory(app: String, profile: String?) async throws -> KvInventory {
        inventoryAsks += 1
        if inventoryFails { throw CuaError.InvalidArgument(message: "no cookies") }
        return inventoryResult
    }

    /// Passphrases the page sent (test fixtures, never real secrets).
    var passphrases: [String] = []

    func setupWithPassphrase(passphrase: String) async throws -> String? {
        passphrases.append(passphrase)
        current.availability = "ready"
        current.status?.initialized = true
        current.status?.unlocked = true
        return "ABCDE-FGHJK"
    }

    func unlockWithPassphrase(passphrase: String) async throws {
        passphrases.append(passphrase)
        current.availability = "ready"
        current.status?.unlocked = true
    }

    func lock() async throws {
        current.availability = "locked"
        current.status?.unlocked = false
    }
}

func fixtureOverview() throws -> KeyvaultOverview {
    let url = ParityTests.dir.appendingPathComponent("keyvault-approve-deny.json")
    let flow = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
    let data = try JSONSerialization.data(withJSONObject: flow["overview"]!)
    return try kvOverviewFromJson(json: String(decoding: data, as: UTF8.self))
}

let fixtureNow = Date(timeIntervalSince1970: 1_800_000_000)

@MainActor
@Suite("View models")
struct ViewModelTests {
    func makeModel(_ backend: SpacesBackend = FixtureSpacesBackend(), kv: KeyvaultClientProtocol? = nil,
                   host: HostRunning? = nil, account: AccountRunning? = nil,
                   agents: AgentSetupRunning? = nil, billing: BillingRunning? = nil,
                   loginItem: LoginItemControlling? = nil, telemetry: TelemetryRunning = FixtureTelemetry()) -> AppModel {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-mac-tests-\(UUID().uuidString)")
        return AppModel(backend: backend,
                        keyvault: KeyvaultModel(client: kv, clock: { fixtureNow }),
                        onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                        settingsPath: dir.appendingPathComponent("settings.json").path,
                        host: host, account: account, telemetry: telemetry, agentSetup: agents,
                        billing: billing, loginItem: loginItem)
    }

    @Test func liveKeyvaultSharingShowsInTheNotchAndTheMenu() async throws {
        var o = try fixtureOverview()
        o.deliveries = [KvDelivery(importId: "imp-1", target: "dev-1", providerId: "chrome", items: ["i1"],
                                   callerFp: "fp-agent", deliveredMs: 1_799_999_000_000,
                                   expiresMs: 1_800_003_600_000, wiped: false)]
        let fake = FakeKeyvault(o)
        let model = makeModel(kv: fake)
        await model.keyvault.refresh()
        let label = "Keyvault sign-ins live in dev-1"
        #expect(model.notch.state.keyvault == label)
        #expect(model.menuBar.contains { $0.id == .status && $0.label == label })
        // Wiped (the kill switch, Release, or the TTL): the signal goes away.
        fake.current.deliveries[0].wiped = true
        await model.keyvault.refresh()
        #expect(model.notch.state.keyvault == nil)
        #expect(!model.menuBar.contains { $0.label.hasPrefix("Keyvault") })
    }

    /// A Keyvault sign-in in a Space: "Signed in" in the list (the badge
    /// opens Access on its row), the key on its notch tile. Dismiss hides
    /// the notch's indicator, line and key, never the access itself; it is
    /// remembered, and a new copy shows again.
    @Test func signedInSpacesAndDismissingFromTheNotch() async throws {
        var o = try fixtureOverview()
        o.deliveries = [KvDelivery(importId: "imp-1", target: "local:aurora", providerId: "chrome", items: ["i1"],
                                   callerFp: "fp-agent", deliveredMs: 1_799_999_000_000,
                                   expiresMs: 0, wiped: false)]
        let fake = FakeKeyvault(o)
        let model = makeModel(kv: fake)
        await model.refresh()
        await model.keyvault.refresh()
        #expect(model.signedInSpaceIds == ["local:aurora"])
        #expect(model.notch.state.signedIn == ["local:aurora"])
        #expect(model.notch.state.keyvault == "Keyvault sign-ins live in local:aurora")
        model.notch.send(.click)
        #expect(model.notch.view.tiles.first { $0.id == "local:aurora" }?.signedIn == true)
        #expect(model.notch.view.access?.dismiss == "Dismiss")
        // The badge: Access, its row brought forward.
        model.showAccess(spaceId: "local:aurora")
        #expect(model.selection == .keyvault(.category(category: .access)))
        #expect(model.keyvault.focusKey == "d:local:aurora")
        let row = try #require(model.keyvault.list.access.first { $0.kind == .delivery })
        #expect(row.detail == "until you wipe it", "auto-wipe off: no expiry")
        #expect(!model.keyvault.isDismissed(row))
        // Dismiss: the notch forgets it; the list and Access keep it.
        model.keyvault.dismiss(row.imports)
        #expect(model.notch.state.keyvault == nil)
        #expect(model.notch.state.signedIn.isEmpty)
        #expect(model.notch.view.access == nil)
        #expect(model.signedInSpaceIds == ["local:aurora"])
        #expect(model.keyvault.isDismissed(row))
        #expect(model.settings.dismissedAccess == ["imp-1"])
        #expect(!fake.commands.contains { if case .release = $0 { return true } else { return false } },
                "dismissing wipes nothing")
        // Saved: a relaunch keeps it dismissed.
        let again = AppModel(backend: FixtureSpacesBackend(), keyvault: KeyvaultModel(client: fake, clock: { fixtureNow }),
                             onboarding: OnboardingModel(statePath: nil), settingsPath: model.settingsPath)
        await again.refresh()
        await again.keyvault.refresh()
        #expect(again.notch.state.keyvault == nil)
        // A new copy (another import) shows again; the gone one is forgotten.
        fake.current.deliveries = [KvDelivery(importId: "imp-2", target: "local:aurora", providerId: "chrome",
                                              items: ["i1"], callerFp: "fp-agent",
                                              deliveredMs: 1_799_999_500_000, expiresMs: 0, wiped: false)]
        await model.keyvault.refresh()
        #expect(model.notch.state.keyvault == "Keyvault sign-ins live in local:aurora")
        #expect(model.settings.dismissedAccess.isEmpty)
    }

    /// Settings, Keyvault: auto-wipe is off by default; the switch sends the
    /// broker's setting and follows it.
    @Test func autoWipeIsASettingOffByDefault() async throws {
        var o = try fixtureOverview()
        o.status?.autoWipe = false
        let fake = FakeKeyvault(o)
        let model = makeModel(kv: fake)
        await model.loadSettings()
        let section = try #require(model.settingsPage.sections.first { $0.id == "keyvault" })
        let toggle = try #require(section.rows.first { $0.id == "keyvault-auto-wipe" })
        #expect(toggle.kind == .toggle)
        #expect(toggle.options.first { $0.id == "on" }?.active == false)
        await model.choose(row: "keyvault-auto-wipe", option: "on")
        #expect(fake.commands.last == .setAutoWipe(on: true))
        #expect(model.keyvault.autoWipe == true)
        #expect(model.settingsPage.sections.first { $0.id == "keyvault" }?.rows.first?
            .options.first { $0.id == "on" }?.active == true)
        // No broker answer: no section.
        let bare = makeModel()
        await bare.loadSettings()
        #expect(!bare.settingsPage.sections.contains { $0.id == "keyvault" })
    }

    /// The teleport's run says what it is doing, from the SDK's step events
    /// (the Keychain prompts are named before they appear).
    @Test func teleportRunStatusFollowsTheSteps() throws {
        let m = TeleportModel(spaceName: "Aurora", teleport: nil, space: nil, sources: PickerFixture.sources())
        #expect(m.status == nil)
        // To the run, as the teleport-review flow gets there.
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
        #expect(m.state.step == .running)
        #expect(m.status == nil, "no step yet")
        func ev(_ phase: AppTeleportRunPhase, _ detail: String, _ done: UInt64 = 0, _ total: UInt64 = 0) -> AppPickerEvent {
            .progress(event: AppTeleportRunEvent(step: 0, steps: 1, kind: "state", phase: phase, detail: detail,
                                                 doneBytes: done, totalBytes: total))
        }
        let reading = "Reading Chrome cookies (macOS will ask for Keychain access)\u{2026}"
        m.send(ev(.progress, reading))
        #expect(m.status == reading)
        m.send(ev(.progress, "Uploading", 12 << 20, 80 << 20))
        #expect(m.status == "Uploading 12 / 80 MB")
        // The SDK's string phases map onto the picker's.
        let sdk = TeleportRunEvent(step: 0, steps: 1, kind: "state", phase: "progress",
                                   detail: "Importing into the Space", doneBytes: 0, totalBytes: 0)
        m.send(.progress(event: appTeleportRunEvent(event: sdk)))
        #expect(m.status == "Importing into the Space")
    }

    @Test func thisMachineComesFirstAndFollowsTheHost() async {
        let host = FixtureHost()
        let model = makeModel(host: host)
        await model.refresh()
        #expect(model.spaces.contains { $0.id == "this-mac" })
        #expect(model.sidebar.thisMachine?.name == "This machine")
        #expect(model.sidebar.thisMachine?.detail == "Set up for access")
        #expect(!model.sidebar.sections.flatMap(\.rows).contains { $0.id == "this-mac" })
        model.select("this-mac")
        #expect(model.detail(model.selectedSpace!).isHost)
        #expect(model.detail(model.selectedSpace!).actions.isEmpty)
        // No host (tests, previews): no entry.
        let bare = makeModel()
        await bare.refresh()
        #expect(bare.sidebar.thisMachine == nil)
    }

    @Test func hostSetupThenSharingControls() async {
        let host = FixtureHost()
        let model = makeModel(host: host)
        await model.refresh()
        let h = model.host
        #expect(h.panel.actions.map(\.id) == [.setUp])
        await h.run(.setUp)
        #expect(h.formView?.title == "Set up this machine")
        h.send(.setName(name: "Studio"))
        await h.submit()
        #expect(h.form == nil)
        #expect(host.calls == ["setup:relay:https://relay.cua.ai"])
        let panel = h.panel
        #expect(panel.configured)
        #expect(panel.facts.first?.value == "Studio")
        #expect(panel.permissions.map(\.id) == ["screen-recording", "accessibility"])
        #expect(panel.actions.map(\.id) == [.stopSharing, .remove])
        // Stopping is one click; removing asks first.
        #expect(panel.actions.first?.confirm == nil)
        #expect(panel.actions.last?.confirm?.confirmLabel == "Remove")
        #expect(model.sidebar.thisMachine?.detail == "Sharing \u{b7} Relay")
        await h.run(.stopSharing)
        #expect(h.panel.actions.first?.id == .resumeSharing)
        #expect(model.sidebar.thisMachine?.detail == "Not sharing")
        await h.run(.resumeSharing)
        await h.run(.remove)
        #expect(!h.panel.configured)
        #expect(host.calls.suffix(3) == ["stop", "start", "remove"])
    }

    /// A spare machine: set up with the profile, then the two switches.
    @Test func aSpareMachineProvidesSpacesAndKeepsItsDesktopPrivate() async {
        let host = FixtureHost()
        let model = makeModel(host: host)
        await model.refresh()
        let h = model.host
        await h.run(.setUp)
        h.send(.setProfile(profile: "spare"))
        #expect(h.formView?.request?.profile == "spare")
        await h.submit()
        var panel = h.panel
        #expect(panel.toggles.map(\.on) == [false, true])
        #expect(panel.toggles.map(\.enabled) == [true, false], "the last setting on stays on")
        #expect(panel.limits?.contains("Apple\u{2019}s license allows two per Mac") == true)
        #expect(panel.providedEmpty == "None yet")
        #expect(panel.permissions.isEmpty, "no desktop, no screen permissions")
        await h.run(panel.toggles[0].action)
        #expect(host.calls.last == "configure:true:-")
        panel = h.panel
        #expect(panel.toggles.map(\.on) == [true, true])
    }

    @Test func thePanelListsRecentAccessAndFlagsAnAlteredLog() {
        let status = HostStatus(
            configured: true, mode: "direct", relayUrl: nil, directUrl: "http://10.0.0.5:3211", envTokenPath: nil,
            machineId: nil, name: "Studio", sharing: true, serviceInstalled: true, serviceRunning: true,
            serviceKind: "launchd", online: nil, clients: [], allow: [], permissions: [], error: nil,
            recentAccess: [HostAccessRecord(atMs: 2, via: "relay", who: "Ada (acct-1)", what: "FilesystemService")],
            accessLogError: "line 2: altered")
        let panel = appHostPanel(state: appHostState(status: status))
        #expect(panel.recentTitle == "Recent access")
        #expect(panel.recent.map(\.text) == ["Ada (acct-1) \u{b7} Files"])
        #expect(panel.accessWarning != nil)
    }

    @Test func aFailedHostSetupKeepsTheFormWithTheReason() async {
        let host = FixtureHost()
        host.failSetup = "launchctl bootstrap failed\nmore"
        let model = makeModel(host: host)
        await model.refresh()
        model.host.openForm()
        await model.host.submit()
        #expect(model.host.formView?.error == "launchctl bootstrap failed")
        #expect(model.host.formView?.canSubmit == true)
    }

    /// Settings, Experiments: every switch off; Cua Volume on shows Storage
    /// after General, is saved, recorded, and reaches the first run.
    @Test func experimentsAreOffAndCuaVolumeShowsStorage() async {
        let telemetry = FixtureTelemetry()
        let model = makeModel(telemetry: telemetry)
        let toggles = model.experimentsPage.sections.flatMap(\.rows).filter { $0.kind == .toggle }
        #expect(toggles.map(\.id) == ["experiment:cua_volume", "experiment:your_cloud", "experiment:sharing"])
        #expect(toggles.allSatisfy { $0.options.first { $0.id == "off" }?.active == true })
        #expect(!model.settingsPageWithStorage.sections.map(\.id).contains("storage"))
        model.chooseExperiment(row: "experiment:cua_volume", option: "on")
        #expect(model.settingsPageWithStorage.sections.map(\.id).prefix(3) == ["account", "general", "storage"])
        #expect(appSettingsLoad(path: model.settingsPath).experiments.cuaVolume, "saved")
        #expect(model.onboarding.state.experiments.cuaVolume)
        let toggled = telemetry.recorded.compactMap { s -> String? in
            if case let .experiment(action, experiment) = s { return "\(action) \(experiment)" }
            return nil
        }
        #expect(toggled == ["experiment_on cua_volume"])
    }

    @Test func settingsPageAndAccountFromTheCore() async {
        let account = FixtureAccount()
        let agents = FixtureAgentSetup()
        let model = makeModel(account: account, agents: agents)
        await model.loadSettings()
        let page = model.settingsPage
        #expect(page.sections.map(\.id) == ["account", "general", "privacy", "agents"])
        #expect(model.chrome.signInLabel == "Sign in")
        await model.press(row: "sign-in")
        #expect(model.identity == "you@example.com")
        #expect(model.chrome.account == "you@example.com")
        #expect(model.settingsPage.sections[0].rows[0].button == "Sign out")
        await model.choose(row: "notch", option: "hide")
        #expect(model.settings.menuBar)
        await model.press(row: "agent:claude-code")
        #expect(agents.calls == ["setup:claude-code"])
        #expect(model.agentRows?.first { $0.agent == "claude-code" }?.configured == true)
        await model.configureAllAgents()
        #expect(agents.calls.last == "setup:claude-code,codex")
    }

    @Test func menuBarAndChromeAreTheCores() async {
        let model = makeModel()
        await model.refresh()
        // The notch's count: This machine counts only while it is shared.
        let open = appOpenableCount(spaces: model.spaces)
        #expect(model.menuBar.map(\.label) == appMenuBar(spaces: open).map(\.label))
        #expect(model.notch.view.tab.count == String(open))
        #expect(model.chrome.emptyTitle == "No Spaces yet")
        #expect(model.chrome.signInLabel == nil, "no account: signing in is not offered")
    }

    @Test func sentFilesReadAsTheCoresLine() async throws {
        let backend = FixtureSpacesBackend()
        let files = try await backend.sendFiles(id: "local:aurora", paths: ["/tmp/notes.txt"])
        #expect(appDropSentText(files: files) == "notes.txt (1.0 KB) verified in /home/cua/Downloads/notes.txt")
    }

    @Test func refreshBuildsTheSidebarFromTheCore() async {
        let model = makeModel()
        await model.refresh()
        let titles = model.sidebar.sections.map(\.title)
        #expect(titles == ["Cua Cloud", "This Mac", "Connected"])
        // The container hostname defers to the name in the id.
        #expect(model.spaces.contains { $0.name == "Aurora" })
        // An unreachable Space is dimmed and cannot stream.
        let builder = model.spaces.first { $0.id == "cloud:builder" }!
        #expect(model.detail(builder).canStream == false)
        #expect(model.sidebar.sections[0].rows[0].dim)
    }

    @Test func searchFiltersThroughTheCore() async {
        let model = makeModel()
        await model.refresh()
        model.query = "auro"
        #expect(model.sidebar.sections.flatMap(\.rows).map(\.name) == ["Aurora"])
        model.query = "zzz"
        #expect(model.sidebar.emptyText == "No matches")
    }

    @Test func wizardWalksToALocalPlanAndCreates() async {
        let backend = FixtureSpacesBackend()
        let model = makeModel(backend)
        await model.openNewSpace()
        let w = model.wizard
        #expect(w.view.canContinue)
        w.send(.next)
        // The local range tops out at this Mac's cores (a CI runner has 3).
        let cpus = min(4, w.view.maxCpus)
        w.send(.setCpus(cpus: cpus))
        w.send(.next)
        w.send(.setName(name: "Bad Name"))
        #expect(w.view.nameInvalid && !w.view.canContinue)
        w.send(.setName(name: "demo"))
        w.send(.next)
        #expect(w.view.primaryLabel == "Create Space")
        model.create(w.view.plan)
        #expect(!model.showingNewSpace)
        for _ in 0..<50 where backend.created.isEmpty { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(backend.created.first?.name == "demo")
        #expect(backend.created.first?.cpus == cpus)
        #expect(backend.created.first?.memoryMb == 4096)
    }

    /// Run on lists this Mac only while no cloud is connected (and without
    /// the Your cloud experiment, no cloud at all); Cua Cloud is never
    /// offered, and New Space creates on this Mac.
    @Test func runOnIsThisMacUntilACloudIsConnected() async {
        let backend = FixtureSpacesBackend()
        backend.fixtureCloud = true
        backend.fixturePricing = AppCloudPricing(vcpuHourUsd: 0.044625, memoryGibHourUsd: 0.0223125)
        let model = makeModel(backend)
        await model.openNewSpace()
        let w = model.wizard
        #expect(w.view.placements.map(\.id) == ["local"])
        #expect(w.view.placementId == "local")
        #expect(!w.view.fields.contains { $0.id == "connect-cloud" }, "Connect a cloud needs Your cloud")
        w.send(.setPlacement(placement: .cloud))
        w.send(.setPlacement(placement: .yours))
        #expect(w.view.plan.placement == .local)
        w.send(.next)
        w.send(.next)
        w.send(.next)
        model.create(w.view.plan)
        for _ in 0..<50 where backend.created.isEmpty { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(backend.created.first?.on == "local")

        await model.openNewSpace()
        w.send(.setPlacement(placement: .local))
        w.send(.next)
        #expect(w.view.price == nil, "no price on your own hardware")
    }

    /// "Connect a cloud" tests (creating nothing), connects, and the open
    /// wizard can create there, with its machine, cost and lifetime.
    @Test func connectACloudThenCreateThere() async {
        let backend = FixtureSpacesBackend()
        let model = makeModel(backend)
        model.chooseExperiment(row: "experiment:your_cloud", option: "on")
        await model.openNewSpace()
        let w = model.wizard
        #expect(w.view.fields.contains { $0.id == "connect-cloud" })
        model.cloud.open()
        await model.cloud.refresh()
        #expect(model.cloud.view.rows.first?.found == true)
        await model.cloud.send(.test)
        #expect(model.cloud.view.result == "Ready: 1. Nothing was created.")
        await model.cloud.send(.setMakeDefault(on: true))
        await model.cloud.send(.connect)
        #expect(backend.fixtureCloudCalls.contains("cloud_connect"))
        #expect(model.cloud.showing == false)
        let aws = w.view.placements.first { $0.id == "aws" }
        #expect(aws?.label == "AWS \u{00B7} us-west-2")
        #expect(aws?.enabled == true)
        #expect(w.view.plan.placement == .yours, "made default")
        #expect(w.view.placementId == "aws")
        w.send(.next)
        #expect(w.view.price == "About $0.04/hour")
        #expect(w.view.resourceFacts.map(\.value).contains("After 8 hours"))
        w.send(.next)
        w.send(.next)
        model.create(w.view.plan)
        for _ in 0..<50 where backend.created.isEmpty { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(backend.created.first?.on == "aws")
    }

    /// Resources shows kind, architecture, the disk and the room left from
    /// the SDK's storage probe; a Linux VM's disk grows with a slider into
    /// the create; an x64-only image warns it runs emulated.
    @Test func resourcesShowTheDiskAndTheRoomAndGrowTheDisk() async {
        let backend = FixtureSpacesBackend()
        let gb: UInt64 = 1 << 30
        backend.fixtureStorage = LocalStorage(
            hostArch: "arm64", reserveBytes: 5 * gb,
            lume: StorageVolume(availableBytes: 212 * gb, totalBytes: 500 * gb, name: "Macintosh HD"),
            qemu: StorageVolume(availableBytes: 212 * gb, totalBytes: 500 * gb, name: "Macintosh HD"),
            container: StorageVolume(availableBytes: 40 * gb, totalBytes: 100 * gb, name: "Colima"),
            pulled: [])
        let model = makeModel(backend)
        await model.openNewSpace()
        let w = model.wizard
        #expect(w.env.hostArch == "arm64" && w.env.storage?.container?.name == "Colima")
        w.send(.chooseImage(imageRef: "ghcr.io/trycua/linux:24.04-disk"))
        w.send(.setPlacement(placement: .local))
        w.send(.next)
        #expect(w.view.diskEditable && w.view.minDiskGb == 20 && w.view.maxDiskGb == 500)
        #expect(w.view.resourceFacts.map(\.label) == ["Kind", "Architecture", "Download", "Available"])
        #expect(w.view.resourceFacts.last?.value == "212 GB on Macintosh HD")
        w.send(.setDisk(diskGb: 64))
        #expect(w.view.diskText == "64 GB")
        w.send(.next)
        w.send(.next)
        model.create(w.view.plan)
        for _ in 0..<50 where backend.created.isEmpty { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(backend.created.first?.diskGb == 64)

        await model.openNewSpace()
        w.send(.chooseImage(imageRef: "ghcr.io/trycua/omarchy:edge"))
        w.send(.setPlacement(placement: .local))
        w.send(.next)
        let arch = w.view.resourceFacts.first { $0.id == "arch" }
        #expect(arch?.value == "x64" && arch?.symbol == "exclamationmark.triangle")
        #expect(arch?.help == "Emulated on this Mac’s ARM processor. Performance may be degraded.")
    }

    /// The new Space is in the list, the notch and the selection the moment
    /// Create is pressed, follows the SDK's progress, and hands over to the
    /// registry row when ready; nothing waits on the create to show it.
    @Test func createShowsTheSpaceAtOnceAndFollowsItsProgress() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdCreates = true
        // Held after the pull; `ready` comes on release.
        backend.createPhases = [
            SpaceCreateProgress(phase: "preparing", fraction: nil, detail: ""),
            SpaceCreateProgress(phase: "pulling", fraction: 0.5, detail: "ghcr.io/trycua/linux:24.04"),
            SpaceCreateProgress(phase: "ready", fraction: nil, detail: ""),
        ]
        let model = makeModel(backend)
        await model.refresh()
        await model.openNewSpace()
        let w = model.wizard
        w.send(.next)
        w.send(.next)
        w.send(.setName(name: "fresh"))
        w.send(.next)
        let plan = w.view.plan
        model.create(plan)
        // Synchronously, before the create has done anything.
        let row = try #require(model.sidebar.sections.flatMap(\.rows).first { $0.name == "Fresh" })
        #expect(row.id.hasPrefix("pending:") && row.progress == 10 && row.trailing == "1%")
        #expect(model.notch.view.activity?.kind == .provisioning)
        if plan.openDesktop {
            #expect(model.selectedSpaceId == row.id)
            #expect(model.selectedSpace.map { model.detail($0).previewText } == "Starting\u{2026}")
        }
        // The SDK's pull reaches the row (bounded wait).
        // (The 4 Hz tick moves the bar within a phase meanwhile; the pull's
        // bytes put it half way through the pull's band.)
        for _ in 0..<100 where (model.sidebar.sections.flatMap(\.rows).first(where: { $0.id == row.id })?.progress ?? 0) < 300 {
            try await Task.sleep(for: .milliseconds(10))
        }
        let pulling = try #require(model.sidebar.sections.flatMap(\.rows).first { $0.id == row.id })
        #expect(pulling.progress == 425 && pulling.trailing == "42%", "\(pulling)")
        backend.releaseCreate()
        for _ in 0..<200 where model.spaces.contains(where: { $0.id == row.id }) {
            try await Task.sleep(for: .milliseconds(10))
        }
        let ids = model.spaces.map(\.id)
        #expect(ids.contains("local:fresh") && !ids.contains(row.id), "\(ids)")
        #expect(model.creates.pending.isEmpty)
        if plan.openDesktop { #expect(model.selectedSpaceId == "local:fresh") }
    }

    /// A failed create stays on its row with the reason until it is removed.
    @Test func aFailedCreateStaysInlineUntilRemoved() async throws {
        let backend = FixtureSpacesBackend()
        backend.createError = "no local runtime found"
        let model = makeModel(backend)
        await model.refresh()
        await model.openNewSpace()
        let w = model.wizard
        w.send(.next)
        w.send(.next)
        w.send(.setName(name: "broken"))
        w.send(.next)
        model.create(w.view.plan)
        var row: AppSidebarRow?
        for _ in 0..<200 {
            row = model.sidebar.sections.flatMap(\.rows).first { $0.name == "Broken" }
            if row?.trailing == "no local runtime found" { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        let failed = try #require(row)
        #expect(failed.trailing == "no local runtime found" && failed.progress == nil)
        #expect(failed.statusText == "Failed" && failed.dim)
        #expect(model.banner == nil, "the error is on the row, not a banner")
        let space = try #require(model.spaces.first { $0.id == failed.id })
        let detail = model.detail(space)
        #expect(detail.previewText == "no local runtime found" && !detail.canStream)
        #expect(detail.actions.contains { $0.id == .delete && $0.label == "Remove" && $0.enabled })
        model.delete(space)
        #expect(!model.spaces.contains { $0.id == failed.id })
        #expect(backend.removed.isEmpty, "a failed create has nothing to delete")
    }

    /// Starts a held create named `name` and returns its pending row id.
    @MainActor func startHeldCreate(_ model: AppModel, _ name: String) async throws -> String {
        await model.refresh()
        await model.openNewSpace()
        let w = model.wizard
        w.send(.next)
        w.send(.next)
        w.send(.setName(name: name))
        w.send(.next)
        model.create(w.view.plan)
        let row = try #require(model.creates.pending.first)
        return row.id
    }

    /// The download's bytes reach the Space's detail as one quiet line
    /// under the progress, and the create is started with its pending id
    /// (what Cancel finds it by).
    @Test func aDownloadShowsItsBytesRateAndTimeLeft() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdCreates = true
        backend.createPhases = [
            SpaceCreateProgress(phase: "pulling", fraction: 0.1757, detail: "ghcr.io/trycua/macos:26",
                                bytesDone: 4_200_000_000, bytesTotal: 23_900_000_000, bytesPerSecond: 85_000_000),
            SpaceCreateProgress(phase: "ready", fraction: nil, detail: ""),
        ]
        let model = makeModel(backend)
        let id = try await startHeldCreate(model, "mac")
        var text: String?
        for _ in 0..<200 {
            text = model.spaces.first { $0.id == id }.flatMap { model.detail($0).progressText }
            if text != nil { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(text == "3.9 of 22.3 GB \u{00B7} 81 MB/s \u{00B7} about 4 min")
        #expect(backend.createIds == [id])
        let row = try #require(model.sidebar.sections.flatMap(\.rows).first { $0.id == id })
        #expect(!row.name.contains("GB"), "the list row stays one line: \(row)")
        backend.releaseCreate()
    }

    /// Cancel: the row shows Cancelling (the toolbar button disabled), the
    /// SDK is asked by the pending id, and the row goes when the create
    /// ends cancelled; it never shows Failed.
    @Test func cancelShowsCancellingThenTheRowGoes() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdCreates = true
        let model = makeModel(backend)
        let id = try await startHeldCreate(model, "stopme")
        let space = try #require(model.spaces.first { $0.id == id })
        let cancel = try #require(model.detail(space).actions.first { $0.id == .cancel })
        #expect(cancel.label == "Cancel" && cancel.enabled)
        model.cancelCreate(id)
        let cancelling = try #require(model.spaces.first { $0.id == id })
        let off = try #require(model.detail(cancelling).actions.first { $0.id == .cancel })
        #expect(off.label == "Cancel" && !off.enabled)
        #expect(model.detail(cancelling).previewText == "Cancelling\u{2026}")
        for _ in 0..<200 where model.spaces.contains(where: { $0.id == id }) {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(!model.spaces.contains { $0.id == id })
        #expect(model.creates.pending.isEmpty, "\(model.creates.pending)")
        #expect(backend.cancelRequests == [id])
        #expect(model.banner == nil)
    }

    /// A create cancelled elsewhere (`cua spaces cancel`) leaves the list;
    /// it is not a failure.
    @Test func aCreateCancelledElsewhereLeavesTheList() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdCreates = true
        let model = makeModel(backend)
        let id = try await startHeldCreate(model, "elsewhere")
        backend.cancelledCreates.insert(id)
        backend.releaseCreate()
        for _ in 0..<200 where model.spaces.contains(where: { $0.id == id }) {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(!model.spaces.contains { $0.id == id })
        #expect(!model.sidebar.sections.flatMap(\.rows).contains { $0.statusText == "Failed" })
        #expect(backend.cancelRequests.isEmpty)
    }

    /// A cancel that fails says why on the row.
    @Test func aFailedCancelSaysWhyOnTheRow() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdCreates = true
        backend.cancelError = "timed out: the cua daemon did not answer"
        let model = makeModel(backend)
        let id = try await startHeldCreate(model, "stuck")
        model.cancelCreate(id)
        var row: AppSidebarRow?
        for _ in 0..<200 {
            row = model.sidebar.sections.flatMap(\.rows).first { $0.id == id }
            if row?.trailing == "timed out: the cua daemon did not answer" { break }
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(row?.trailing == "timed out: the cua daemon did not answer", "\(String(describing: row))")
        backend.releaseCreate()
    }

    /// New Space asks the SDK for GPU options: a macOS VM on Lume gets the
    /// checkbox, and checked it reaches the create.
    @Test func theGpuCheckboxReachesTheCreate() async throws {
        let backend = FixtureSpacesBackend()
        backend.fixtureBackends = ["docker", "lume"]
        backend.fixtureGpus = appGpuChoices([
            GpuSupport(runtime: "lume", options: [GpuOption(
                id: "paravirtual", label: "GPU acceleration", experimental: true, supported: true, reason: "",
                learnMore: "https://cua.ai/docs/lume/guides/gpu-passthrough", usdPerHour: nil)], reason: ""),
            GpuSupport(runtime: "qemu", options: [], reason: "no virgl here"),
        ])
        #expect(backend.fixtureGpus?.map(\.runtime) == ["lume"])
        #expect(backend.fixtureGpus?.first?.reason == nil)
        let model = makeModel(backend)
        await model.refresh()
        await model.openNewSpace()
        let w = model.wizard
        #expect(w.env.gpus?.count == 1)
        w.send(.chooseOs(os: .macos))
        w.send(.next)
        let gpu = try #require(w.view.gpu)
        #expect(gpu.label == "GPU acceleration (Experimental)" && gpu.enabled && !gpu.on)
        #expect(gpu.learnMoreUrl == "https://cua.ai/docs/lume/guides/gpu-passthrough")
        w.send(.setGpu(on: true))
        w.send(.next)
        w.send(.next)
        model.create(w.view.plan)
        for _ in 0..<100 where backend.created.isEmpty { try await Task.sleep(for: .milliseconds(10)) }
        #expect(backend.created.first?.gpu == "paravirtual")
    }

    /// Delete shows Deleting at once (a refresh during the delete keeps
    /// it), a second press does nothing, and the row goes when it returns.
    @Test func deleteShowsDeletingAtOnceUntilItReturns() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdRemoves = true
        let model = makeModel(backend)
        await model.refresh()
        let space = try #require(model.spaces.first { $0.sdk != nil })
        model.delete(space)
        // Synchronously, before the SDK has done anything.
        let row = try #require(model.sidebar.sections.flatMap(\.rows).first { $0.id == space.id })
        #expect(row.status == .deleting && row.statusText == "Deleting\u{2026}" && row.dim)
        #expect(model.notch.view.activity?.kind == .deleting)
        let deleting = try #require(model.spaces.first { $0.id == space.id })
        let detail = model.detail(deleting)
        #expect(!detail.canStream && !detail.showSections && detail.previewText == "Deleting\u{2026}")
        #expect(detail.actions.allSatisfy { !$0.enabled })
        await model.refresh()
        #expect(model.spaces.first { $0.id == space.id }?.status == .deleting, "a probe cannot undo it")
        model.delete(deleting)
        backend.releaseRemove()
        // The row goes when the delete returns; the refresh after it settles
        // the delete (both finish on their own schedule).
        for _ in 0..<200 where model.spaces.contains(where: { $0.id == space.id }) || !model.creates.deleting.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(!model.spaces.contains { $0.id == space.id })
        #expect(backend.removed == [space.id], "one delete")
        #expect(model.creates.deleting.isEmpty, "settled once the registry dropped it")
    }

    /// A Space in your cloud shows where it runs; Delete offers Delete
    /// Permanently (disabled, with why, when another device created it) and
    /// Remove from List, which only forgets it.
    @Test func aSpaceInYourCloudDeletesPermanentlyOrIsRemovedFromTheList() async throws {
        func cloudRow(_ id: String, _ name: String, _ cloud: String, _ place: String, _ delete: String) -> AppSpaceRow {
            AppSpaceRow(id: id, name: name, provider: "relay", spacesdVersion: "0.4.0", features: [], addedAt: nil,
                        os: .linux, osName: nil, osPrettyName: nil, image: nil, imageDigest: nil, kind: nil, arch: nil,
                        reachable: true, error: nil, host: nil, hostName: nil, power: nil, powerState: nil,
                        cloud: cloud, cloudPlace: place, cloudDelete: delete)
        }
        let backend = FixtureSpacesBackend(rows: [
            cloudRow("relay:cloud-0000000000000a01", "research-box", "aws", "AWS \u{b7} us-west-2", "here"),
            cloudRow("relay:cloud-0000000000000e01", "their-box", "gcp", "Google Cloud \u{b7} us-central1", "elsewhere"),
        ])
        let model = makeModel(backend)
        await model.refresh()
        let rows = model.sidebar.sections.flatMap(\.rows)
        #expect(rows.first { $0.id == "relay:cloud-0000000000000a01" }?.place == "AWS \u{b7} us-west-2")
        let mine = try #require(model.spaces.first { $0.id == "relay:cloud-0000000000000a01" })
        let theirs = try #require(model.spaces.first { $0.id == "relay:cloud-0000000000000e01" })

        let detail = model.detail(mine)
        #expect(detail.facts.contains { $0.label == "Location" && $0.value == "AWS \u{b7} us-west-2" })
        #expect(!detail.removeOnly)
        #expect(detail.confirm.confirmLabel == "Delete Permanently" && detail.confirm.confirmEnabled)
        #expect(detail.confirm.removeLabel == "Remove from List")
        let other = model.detail(theirs).confirm
        #expect(!other.confirmEnabled)
        #expect(other.disabledReason == "Created on another device: delete it there, or remove it from this list.")

        model.delete(mine)
        model.delete(theirs, removeOnly: true)
        for _ in 0..<200 where backend.removed.count < 2 || !model.creates.deleting.isEmpty {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(Set(backend.removed) == [mine.id, theirs.id])
        #expect(backend.forgotten == [theirs.id], "only Remove from List forgets")
    }

    /// A failed delete puts the row back and says why.
    @Test func aFailedDeleteRestoresTheRow() async throws {
        let backend = FixtureSpacesBackend()
        backend.removeError = "lume is not running"
        let model = makeModel(backend)
        await model.refresh()
        let space = try #require(model.spaces.first { $0.sdk != nil })
        model.delete(space)
        for _ in 0..<200 where model.banner == nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(model.banner == appDeleteFailedText(name: space.name, error: "lume is not running"))
        #expect(model.spaces.first { $0.id == space.id }?.status == space.status)
        #expect(!model.isDeleting(space.id))
    }

    /// The power button: Suspending at once with the button waiting, a
    /// second press does nothing, then Suspended with Resume once the
    /// registry shows it; Resume brings it back. A Space that cannot be
    /// turned off has no button.
    @Test func powerSuspendsWaitsAndResumes() async throws {
        let backend = FixtureSpacesBackend()
        backend.holdPower = true
        let model = makeModel(backend)
        await model.refresh()
        let row = { (id: String) in model.sidebar.sections.flatMap(\.rows).first { $0.id == id } }
        #expect(row("cloud:builder")?.power == nil, "a cloud Space cannot be turned off")
        let aurora = try #require(model.spaces.first { $0.id == "local:aurora" })
        #expect(row(aurora.id)?.power?.help == "Suspend")
        // No Share without the Sharing experiment; with it, before power.
        #expect(model.detail(aurora).actions.map(\.id) == [.teleport, .pip, .power, .delete, .open])
        model.chooseExperiment(row: "experiment:sharing", option: "on")
        #expect(model.detail(aurora).actions.map(\.id) == [.teleport, .pip, .share, .power, .delete, .open])

        model.setPower(aurora, on: false)
        // Synchronously, before the SDK has done anything.
        let busy = try #require(row(aurora.id)?.power)
        #expect(busy.busy && !busy.enabled && busy.help == "Suspending\u{2026}")
        #expect(row(aurora.id)?.statusText == "Suspending\u{2026}")
        let auroraNow = try #require(model.spaces.first { $0.id == aurora.id })
        let action = try #require(model.detail(auroraNow).actions.first { $0.id == .power })
        #expect(action.busy && !action.enabled)
        model.setPower(aurora, on: false)
        backend.releasePower()
        for _ in 0..<200 where row(aurora.id)?.power?.help != "Resume" {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(backend.powerRequests.map(\.id) == [aurora.id], "one press")
        #expect(row(aurora.id)?.statusText == "Suspended")
        #expect(model.creates.powering.isEmpty, "settled once the registry showed it off")

        let off = try #require(model.spaces.first { $0.id == aurora.id })
        model.setPower(off, on: true)
        for _ in 0..<200 where row(aurora.id)?.power?.help != "Suspend" {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(backend.powerRequests.map(\.on) == [false, true])
        #expect(row(aurora.id)?.statusText == "Running")
    }

    /// A failed power action shows why on the row and the detail, and the
    /// button works again.
    @Test func aFailedPowerActionShowsWhyInline() async throws {
        let backend = FixtureSpacesBackend()
        backend.powerError = "Docker is not running"
        let model = makeModel(backend)
        await model.refresh()
        let aurora = try #require(model.spaces.first { $0.id == "local:aurora" })
        model.setPower(aurora, on: false)
        let row = { model.sidebar.sections.flatMap(\.rows).first { $0.id == aurora.id } }
        for _ in 0..<200 where row()?.trailing == nil {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(row()?.trailing == "Could not turn it off: Docker is not running")
        #expect(row()?.power?.enabled == true)
        let failed = try #require(model.spaces.first { $0.id == aurora.id })
        #expect(model.detail(failed).powerError == "Could not turn it off: Docker is not running")
    }

    @Test func imageFieldSuggestsFiltersAndTakesCustomRefs() async {
        let backend = FixtureSpacesBackend()
        let model = makeModel(backend)
        await model.openNewSpace()
        let w = model.wizard
        let rows = { w.view.imageField.groups.flatMap(\.rows) }
        #expect(!w.view.imageField.open)
        // The selected OS's presets (Linux), grouped by catalog group, the
        // current one highlighted; never benchmark images.
        w.send(.openImageSuggestions)
        #expect(w.view.imageField.open)
        let linux = appWizardImageSuggestions(query: "").compactMap { g -> (String, [String])? in
            let refs = g.images.filter { $0.os == .linux }.map(\.imageRef)
            return refs.isEmpty ? nil : (g.label, refs)
        }
        #expect(w.view.imageField.groups.map(\.label) == linux.map(\.0))
        #expect(rows().map(\.imageRef) == linux.flatMap(\.1))
        #expect(rows().first?.highlighted == true)
        #expect(!appWizardImageSuggestions(query: "").contains { $0.id == "benchmark" })
        #expect(w.view.osTiles.filter(\.pressed).map(\.id) == ["linux"])
        // Typing is custom: no OS is pressed; every OS's presets filter.
        // Down, Return picks and fills the field.
        w.send(.setImageText(text: "macos"))
        #expect(w.view.osTiles.allSatisfy { !$0.pressed })
        let macos = appWizardImageSuggestions(query: "macos").flatMap(\.images).map(\.imageRef)
        #expect(rows().map(\.imageRef) == macos)
        #expect(macos.count >= 2)
        w.send(.moveImageSuggestion(delta: 1))
        w.send(.moveImageSuggestion(delta: 1))
        w.send(.pickImageSuggestion)
        #expect(w.view.imageField.text == macos[1])
        #expect(w.view.image.os == .macos && !w.view.imageField.open)
        // Escape dismisses.
        w.send(.setImageText(text: "ubuntu"))
        #expect(w.view.imageField.open)
        w.send(.dismissImageSuggestions)
        #expect(!w.view.imageField.open)
        // A malformed ref is rejected inline; Continue waits.
        w.send(.setImageText(text: "ghcr.io/Acme/App"))
        #expect(w.view.imageField.error == "Use lowercase letters in the image name.")
        #expect(!w.view.canContinue)
        // A custom ref is used as typed, run like the chosen system.
        w.send(.chooseOs(os: .linux))
        w.send(.setImageText(text: "ghcr.io/acme/desktop:1.2"))
        w.send(.pickImageSuggestion)
        #expect(w.view.imageField.custom && w.view.imageField.error == nil && w.view.canContinue)
        w.send(.next)
        w.send(.next)
        w.send(.next)
        model.create(w.view.plan)
        for _ in 0..<50 where backend.created.isEmpty { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(backend.created.first?.image == "ghcr.io/acme/desktop:1.2")
        #expect(backend.created.first?.kind == .container)
    }

    @Test func connectByAddressShowsTheFriendlyError() async {
        let model = makeModel()
        let w = model.wizard
        w.send(.showAddress)
        #expect(!w.view.address.canSubmit)
        w.send(.setAddress(url: "127.0.0.1:1"))
        #expect(w.view.address.canSubmit)
        await w.submitAddress { _, _, _ in throw TimeoutError.timedOut }
        #expect(w.view.address.error != nil)
        w.send(.addressFailed(error: "status: Unauthenticated"))
        #expect(w.view.address.error?.hasPrefix("The Space rejected the token") == true)
    }

    func unlockCase(_ name: String) throws -> KeyvaultOverview {
        let url = ParityTests.dir.appendingPathComponent("keyvault-unlock.json")
        let flow = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
        let c = (flow["cases"] as! [[String: Any]]).first { $0["name"] as? String == name }!
        let data = try JSONSerialization.data(withJSONObject: c["overview"]!)
        return try kvOverviewFromJson(json: String(decoding: data, as: UTF8.self))
    }

    @Test func passphraseSetupAndUnlockGoOnlyToTheBroker() async throws {
        let fake = FakeKeyvault(try unlockCase("setup-passphrase"))
        let model = makeModel(kv: fake)
        let kv = model.keyvault
        await kv.refresh()
        #expect(kv.page.form?.method == .passphrase)
        kv.passphrase = "orbit"
        #expect(!kv.canSubmitCredential)
        #expect(kv.passphraseCheck?.hint == "At least 12 characters")
        kv.passphrase = "orbit lantern pickle harbor"
        kv.passphraseConfirm = "orbit lantern pickle"
        #expect(!kv.canSubmitCredential)
        kv.passphraseConfirm = kv.passphrase
        #expect(kv.canSubmitCredential)
        await kv.submitCredential()
        #expect(fake.passphrases == ["orbit lantern pickle harbor"])
        #expect(fake.commands.isEmpty, "a passphrase never travels in a command")
        #expect(kv.passphrase.isEmpty && kv.passphraseConfirm.isEmpty)
        #expect(kv.recoveryKey == "ABCDE-FGHJK")

        fake.current = try unlockCase("unlock-passphrase")
        await kv.refresh()
        #expect(kv.recoveryKey == nil, "shown once, gone once the vault locks")
        #expect(kv.page.form?.mode == .unlock && kv.page.form?.confirmLabel == nil)
        kv.passphrase = "orbit lantern pickle harbor"
        await kv.submitCredential()
        #expect(fake.passphrases.count == 2)
        #expect(kv.page.ready)
    }

    @Test func touchIdSetupSendsTheSetupCommand() async throws {
        let fake = FakeKeyvault(try unlockCase("setup-touch-id"))
        let model = makeModel(kv: fake)
        await model.keyvault.refresh()
        #expect(model.keyvault.page.form?.method == .touchId)
        #expect(model.keyvault.canSubmitCredential)
        await model.keyvault.submitCredential()
        #expect(fake.commands == [.setup])
        #expect(fake.passphrases.isEmpty)
    }

    @Test func approvalSelectsNothingAndApprovesOnlyTicked() async throws {
        let fake = FakeKeyvault(try fixtureOverview())
        let model = makeModel(kv: fake)
        await model.keyvault.refresh()
        model.keyvault.openApproval("req-1")
        let v = try #require(model.keyvault.approvalView)
        #expect(v.rows.allSatisfy { !$0.selected })
        #expect(!v.canApprove)
        await model.keyvault.approve()
        #expect(fake.commands.isEmpty, "nothing ticked, nothing sent")
        // Rows are sites; ticking one approves exactly its items.
        model.keyvault.sendApproval(.toggle(key: "chrome|example.test"))
        #expect(model.keyvault.approvalView?.canApprove == true)
        await model.keyvault.approve()
        #expect(fake.commands == [.approve(requestId: "req-1", items: ["gh-ada", "gh-bob"])])
        #expect(model.keyvault.approval == nil)
    }

    @Test func theKillSwitchBlocksApproval() async throws {
        let fake = FakeKeyvault(try fixtureOverview())
        let model = makeModel(kv: fake)
        await model.keyvault.refresh()
        await model.keyvault.setDisabled(true)
        #expect(model.keyvault.page.disabled)
        model.keyvault.openApproval("req-1")
        model.keyvault.sendApproval(.selectAll)
        #expect(model.keyvault.approvalView?.canApprove == false)
        #expect(model.keyvault.approvalView?.blockedReason == "Keyvault is off. Turn it on to approve.")
    }

    @Test func keyvaultCategoriesAndApps() async throws {
        let model = makeModel(kv: FakeKeyvault(try fixtureOverview()))
        await model.keyvault.refresh()
        let sidebar = model.keyvault.sidebar
        #expect(sidebar.categories.map(\.title) == ["All Items", "Waiting", "Access", "Recent"])
        #expect(sidebar.apps.map(\.title) == ["Chrome", "Slack"])
        model.keyvault.selection = .category(category: .waiting)
        #expect(model.keyvault.list.pending.count == 2)
    }

    @Test func onboardingFollowsTheCoreAndPersists() throws {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-mac-onb-\(UUID().uuidString).json").path
        let o = OnboardingModel(statePath: path)
        #expect(!o.completed)
        #expect(o.view.showMark)
        o.send(.start)
        o.send(.signinDone)
        o.send(.agentsDone(configured: []))
        o.send(.presentationDone)
        o.send(.driveContinue)
        o.send(.modeChosen(mode: .client))
        #expect(o.view.step == .done)
        #expect(o.view.prompts.count == 7, "Done suggests what to ask a coding agent")
        #expect(o.view.prompts.first == "qa my app on windows, macos and linux")
        o.finish()
        #expect(OnboardingModel(statePath: path).completed)
        try? FileManager.default.removeItem(atPath: path)
    }

    @Test func notchOpensAfterTheCoresDwell() async {
        let notch = NotchModel()
        notch.spaces = [appThisMachineSpace(status: nil, nowMs: 0)]
        notch.send(.hoverEnter)
        #expect(notch.view.phase == .closed)
        for _ in 0..<60 where notch.view.phase == .closed { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(notch.view.phase == .tiles)
        notch.send(.hoverExit)
        for _ in 0..<60 where notch.view.phase != .closed { try? await Task.sleep(for: .milliseconds(20)) }
        #expect(notch.view.phase == .closed)
    }

    @Test func aWindowDragCommitsToTheTileUnderIt() {
        let notch = NotchModel()
        var committed: String?
        notch.onCommit = { id, _ in committed = id }
        notch.send(.drag(event: .start(windowId: 7, appName: "Slack")))
        #expect(notch.view.phase == .prompt)
        notch.send(.drag(event: .enterNotch))
        notch.send(.drag(event: .over(spaceId: "local:aurora")))
        notch.send(.drag(event: .drop(spaceId: nil)))
        #expect(committed == "local:aurora")
        #expect(notch.view.phase == .closed)
    }

    /// Settings hides Cua Cloud billing and offers the Teams waitlist (the
    /// website); a cloud create refused for want of credit shows one line
    /// and no Add credit.
    @Test func billingIsHiddenAndTeamsOpensTheWaitlist() async throws {
        var opened: [URL] = []
        BillingBrowser.open = { opened.append($0) }
        let model = makeModel(account: FixtureAccount(identity: "you@example.com"),
                              billing: FixtureBilling(balanceCents: 742))
        await model.loadSettings()
        let rows = model.settingsPage.sections.flatMap(\.rows)
        #expect(!rows.contains { $0.id == "billing" })
        #expect(!rows.contains { $0.id == "default-location" })
        let teams = rows.first { $0.id == "teams" }
        #expect(teams?.value == "Coming soon" && teams?.button == "Join the waitlist")
        await model.press(row: "teams")
        #expect(opened.map(\.absoluteString) == ["https://cua.ai/teams"])

        var creates = appCreatesReduce(state: AppCreatesState(pending: [], deleting: [], powering: []),
                                       action: .start(id: "pending:1", name: "cloud", os: .linux, provider: .cloud,
                                                      now: 0, image: nil, kind: nil, hostArch: nil, gpu: false))
        creates = appCreatesReduce(state: creates, action: .fail(
            id: "pending:1", error: "You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing"))
        let space = appCreatesCompose(spaces: [], state: creates)[0]
        #expect(space.detail == "You're out of Cua Cloud credit.")
        #expect(appSpaceDetail(space: space).creditNotice == nil)
    }
}