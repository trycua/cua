// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// A teleport handed from the notch to a Space's detail.
public struct PendingTeleport: Equatable {
    public var spaceId: String
    public var entry: TeleportCatalogEntry?
    public var files: [String]
}

/// The Settings window's tabs.
public enum SettingsTab: Hashable, Sendable {
    case general
    case agents
    case devices
    case experiments
    case about
}

/// What the main window shows.
public enum MainSelection: Hashable {
    case space(String)
    case keyvault(KvSelection)
    /// Persistent agents.
    case agents
    /// The Cua Volume.
    case drive
    /// The notifications feed.
    case notifications
}

/// The app's root model. It holds the core's Space list state and forwards
/// every decision to the app core (`appRosterReduce`, `appSidebar`,
/// `appSpaceDetail`); views render what it exposes.
@MainActor
@Observable
public final class AppModel {
    public let backend: SpacesBackend
    /// The launch until the live services are in (ready at once for
    /// fixtures and tests): the window shows it instead of its content.
    public let startup: StartupModel
    /// The SDK backend, once the launch made it (nil in fixtures, and when
    /// it could not start).
    public private(set) var live: LiveSpacesBackend?
    private var whenLive: [(LiveSpacesBackend) -> Void] = []
    public private(set) var roster: AppRosterState
    public var query = ""
    public var selection: MainSelection?
    public var rosterError: String?
    public var banner: String?
    public var bannerIsError = false
    public var showingNewSpace = false
    public var confirmDeleteId: String?
    /// A teleport the notch started: the Space and the dropped app.
    public var pendingTeleport: PendingTeleport?
    public private(set) var loaded = false

    public let wizard: WizardModel
    public let keyvault: KeyvaultModel
    public let onboarding: OnboardingModel
    public let notch: NotchModel
    /// This machine: its state, page and the host setup form.
    public let host: HostModel
    /// This Mac's enrollment on the relay, the account's devices, approvals.
    public let devices: DevicesModel
    /// Persistent agents, the Cua Volume and the notifications feed.
    public let persistent: PersistentModel
    /// Settings, Storage: the Cua Volume's store, Finder volume and cache.
    public internal(set) var storage: StorageModel
    /// Settings, Agents: the provider keys agents get (kept by the daemon).
    public let agentKeys: AgentKeysModel
    /// The user's own clouds: the "Your cloud" tile and "Connect a cloud".
    public let cloud: CloudModel
    /// The Settings tab showing.
    public var settingsTab: SettingsTab = .general
    /// A Settings section to scroll to when Settings opens (captures).
    public var settingsSection: String?
    /// The Cua account, the telemetry switch and the coding agents.
    let account: AccountRunning?
    let telemetry: TelemetryRunning?
    let agentSetup: AgentSetupRunning?
    /// The signed-in account.
    public private(set) var identity: String?
    /// Cua Cloud can be used.
    public private(set) var cloudConfigured = false
    /// The Settings sign-in in progress.
    public private(set) var signIn: AppSignInPhase = .idle
    /// The page a waiting sign-in finishes on ("Open the browser again").
    public private(set) var signInURL: URL?
    /// A sign-in still waiting after this fails (a device code lives
    /// about this long).
    public var signInTimeout: Duration = .seconds(600)
    /// Tells one sign-in from the next (a timeout of an old one is ignored).
    private var signInAttempt = 0
    public private(set) var telemetryInput: AppTelemetryInput?
    /// The account's Cua Cloud billing (Settings, Billing), once read.
    public private(set) var billingStatus: AppBillingStatus?
    /// Cua Cloud billing (nil: no Billing row).
    let billing: BillingRunning?
    /// This app as a login item (nil: no Launch at login row).
    let loginItem: LoginItemControlling?
    /// What the system reports for it (nil until read).
    public private(set) var loginItemStatus: AppLoginItemStatus?
    public private(set) var loginItemBusy = false
    public private(set) var loginItemError: String?
    public private(set) var agentRows: [AppAgentSettingsRow]?
    public private(set) var agentsBusy = false
    public private(set) var agentsPending: [String] = []
    /// Why the last agent action failed, per agent.
    var agentFailures: [String: String] = [:]
    /// The registry's Spaces (before This machine is added).
    var registrySpaces: [AppSpace] = []
    /// Memory and storage use of the Spaces whose detail is showing.
    public private(set) var usage: [String: AppSpaceUsage] = [:]
    /// Spaces being created: each shows in the list the moment its create
    /// starts and follows the SDK's progress (`spaces::creating`). Spaces
    /// being deleted, which show Deleting until the delete returns.
    public private(set) var creates = AppCreatesState(pending: [], deleting: [], powering: [])
    /// Sidebar rows as targets for a dragged real window.
    public let dropTargets = SidebarDropTargets()
    public var settings: AppSettings
    let settingsPath: String
    /// Settings → About and the updater (none until `AppEnvironment`
    /// starts one).
    public var updates = UpdatesModel(updater: nil)

    public init(backend: SpacesBackend, keyvault: KeyvaultModel,
                onboarding: OnboardingModel, settingsPath: String,
                host: HostRunning? = nil, account: AccountRunning? = nil,
                telemetry: TelemetryRunning? = nil, agentSetup: AgentSetupRunning? = nil,
                billing: BillingRunning? = nil, devices: DevicesRunning? = nil,
                presence: PresenceChecking = LivePresence(), loginItem: LoginItemControlling? = nil,
                startup: StartupModel? = nil) {
        self.backend = backend
        self.startup = startup ?? StartupModel()
        self.devices = DevicesModel(devices: devices, presence: presence)
        self.persistent = PersistentModel(tools: backend as? AgentsToolRunning)
        self.storage = StorageModel(tools: backend as? AgentsToolRunning)
        self.agentKeys = AgentKeysModel(tools: backend as? AgentsToolRunning)
        self.cloud = CloudModel(tools: backend as? CloudToolRunning)
        self.billing = billing
        self.loginItem = loginItem
        self.host = HostModel(host: host)
        self.account = account
        self.telemetry = telemetry
        self.agentSetup = agentSetup
        self.roster = appRosterInitial(spaces: [])
        self.keyvault = keyvault
        self.onboarding = onboarding
        self.settingsPath = settingsPath
        let settings = appSettingsLoad(path: settingsPath)
        self.settings = settings
        self.wizard = WizardModel(env: AppWizardEnv(
            defaultLocation: settings.defaultLocation, cloudAvailable: false,
            localAvailable: true, localReason: nil, localBackends: nil, localDetails: nil,
            maxCpus: UInt32(max(2, min(16, ProcessInfo.processInfo.activeProcessorCount))),
            hostArch: Self.hostArch, lumeSource: nil, linuxSource: nil, storage: nil, cloudPricing: nil, clouds: [], hosts: [],
            experiments: settings.experiments, gpus: nil))
        self.notch = NotchModel()
        // The notch tiles and the preview cover read one thumbnail store,
        // filled from the SDK's shared cache.
        notch.thumbnails.fetch = { [backend] id, maxAgeMs in await backend.thumbnail(id: id, maxAgeMs: maxAgeMs) }
        // The app core's usage events for each step (the Tauri app sends
        // the same ones): the first run, Settings, Storage, enrollment.
        onboarding.telemetry = telemetry
        self.storage.telemetry = telemetry
        self.devices.telemetry = telemetry
        // Live Keyvault sign-ins show left of the notch (and in the menu).
        // Dismissed ones stay hidden across launches (Settings file).
        keyvault.dismissed = settings.dismissedAccess
        keyvault.siteIconsFromGoogle = { [weak self] in self?.settings.keyvaultSiteIcons ?? true }
        keyvault.onSharing = { [weak self] _ in self?.syncNotchKeyvault() }
        keyvault.onDismissed = { [weak self] ids in
            guard let self else { return }
            self.settings.dismissedAccess = ids
            self.saveSettings()
        }
        self.host.onChange = { [weak self] in self?.recompose() }
        // The first run's "Where should Cua Spaces show up?" is this setting.
        onboarding.onPresentation = { [weak self] menuBar in
            guard let self else { return }
            self.settings.menuBar = menuBar
            self.saveSettings()
        }
        onboarding.currentMenuBar = { [weak self] in self?.settings.menuBar ?? false }
        // Done's "Launch at login" checkbox is the user's choice.
        onboarding.onLaunchAtLogin = { [weak self] on in self?.setLaunchAtLogin(on) }
        // Settings, Experiments decide the first run's pages (Cua Volume).
        onboarding.currentExperiments = { [weak self] in
            self?.settings.experiments ?? AppExperiments(cuaVolume: false, yourCloud: false, sharing: false, webUi: false)
        }
        // Which experiments are on: the day's `cua_app_active` carries them.
        telemetry?.record(appTelemetryExperimentsOn(experiments: settings.experiments))
        // The notifications marker lives in the settings file, so a restart
        // never posts the same entry again.
        persistent.seenMs = { [weak self] in self?.settings.notificationsSeenMs ?? 0 }
        persistent.saveSeenMs = { [weak self] seen in
            guard let self else { return }
            self.settings.notificationsSeenMs = seen
            self.saveSettings()
        }
        self.identity = account?.identity()
        self.host.identity = identity
        self.devices.signedIn = identity != nil
        // "Sign in again" in the enroll sheet is the account's sign-in.
        self.devices.signIn = { [weak self] in
            guard let self else { return false }
            await self.beginSignIn()
            if case .failed = self.signIn { return false }
            return self.identity != nil
        }
        // "Set up for access" signs in inline when relay setup has no
        // account, then carries on.
        self.host.signIn = { [weak self] in
            guard let self else { return false }
            await self.beginSignIn()
            if case .failed = self.signIn { return false }
            return self.identity != nil
        }
        // Relay sharing follows the sign-in: whose account this Mac is
        // shared with, read from the local session (no network).
        self.host.currentAccount = { [weak self] in
            guard let self, let account = self.account, let who = account.identity() else { return nil }
            let profile = account.profile()
            let name = profile?.name.flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
            return AppHostAccount(id: profile?.subject, email: profile?.email, display: name ?? who)
        }
        // Onboarding's This machine step has its own HostModel: give it the
        // same inline sign-in, or a skipped sign-in fails setup with "Not
        // signed in" instead of opening the browser and carrying on.
        self.onboarding.host.signIn = self.host.signIn
        // A row takes a window drop by the core's rule (the notch tiles' too).
        dropTargets.isDropTarget = { [weak self] id in
            self?.spaces.first { $0.id == id }.map { appSpaceAcceptsDrop(space: $0) } ?? false
        }
    }

    // MARK: - Launch

    /// Runs `f` with the SDK backend once the launch has it (now, when it
    /// already does). Never runs without one.
    public func onLive(_ f: @escaping (LiveSpacesBackend) -> Void) {
        if let live { f(live) } else { whenLive.append(f) }
    }

    /// The live services are in: read the account again (it was unknown
    /// until now) and show what the backend has.
    func attachLive(_ live: LiveSpacesBackend?) {
        identity = account?.identity()
        host.identity = identity
        onboarding.host.identity = identity
        devices.signedIn = identity != nil
        onboarding.adoptIdentity(identity)
        if let live {
            self.live = live
            let queued = whenLive
            whenLive = []
            for f in queued { f(live) }
        }
        watchListFromNow()
        Task {
            await refresh()
            await keyvault.refresh()
            await devices.refresh()
        }
    }

    // MARK: - Space list

    public var spaces: [AppSpace] { roster.spaces }

    /// The sidebar, from the core.
    public var sidebar: AppSidebarView {
        appSidebar(spaces: spaces, query: query, selectedId: selectedSpaceId ?? "")
    }

    public var selectedSpaceId: String? {
        if case .space(let id) = selection { return id }
        return nil
    }

    /// The selected Space and its detail, from the core.
    public var selectedSpace: AppSpace? {
        guard case .space = selection else { return nil }
        let id = sidebar.selectedId
        return spaces.first { $0.id == id }
    }

    /// Every Space's latest thumbnail: the notch tiles' and the preview
    /// cover's one store.
    public var thumbnails: SpaceThumbnails { notch.thumbnails }

    /// The Spaces that can stream now (running and reachable).
    public var streamableSpaceIds: [String] {
        spaces.filter { detail($0).canStream }.map(\.id)
    }

    /// What the Space's preview card shows over (or instead of) its live
    /// desktop, from the core.
    public func cover(_ detail: AppSpaceDetail, requested: Bool, stream: AppStreamPhase) -> AppDesktopCover {
        appDesktopCover(input: AppDesktopCoverInput(
            canStream: detail.canStream, previewText: detail.previewText,
            autoConnect: settings.autoConnect, connectRequested: requested, stream: stream,
            access: detail.access))
    }

    /// The detail without what Settings, Experiments hides (Share while
    /// Sharing is off), as this Mac sees it: a machine reached through the
    /// relay has its Connect greyed out while this Mac is not enrolled, and
    /// one that does not share its desktop shows why in its place.
    public func detail(_ space: AppSpace) -> AppSpaceDetail {
        appSpaceDetailFor(space: space, usage: usage[space.id], hostArch: Self.hostArch,
                          experiments: settings.experiments, access: devices.accessNotice)
    }

    /// The Spaces one of your machines provides (its detail lists them).
    public func hostedRows(_ machineId: String) -> [AppSidebarRow] {
        appHostedRows(spaces: spaces, machineSpaceId: machineId, selectedId: sidebar.selectedId ?? "")
    }


    /// Reads a Space's memory and storage use now.
    public func refreshUsage(_ id: String) async {
        if let u = await backend.usage(id: id) { usage[id] = u }
    }

    /// Refreshes a Space's memory and storage use at the core's low rate
    /// while its detail shows (the caller's task is cancelled when it hides),
    /// until a delete starts.
    public func pollUsage(_ id: String) async {
        while !Task.isCancelled, !isDeleting(id) {
            await refreshUsage(id)
            try? await Task.sleep(for: .milliseconds(Int(appUsageRefreshMs())))
        }
    }

    public func send(_ action: AppRosterAction) {
        roster = appRosterReduce(state: roster, action: action)
        notch.spaces = roster.spaces
        syncNotchKeyvault()
    }

    // MARK: - Keyvault sign-ins in Spaces

    /// The Spaces a Keyvault sign-in is live in ("Signed in" in the list;
    /// dismissing hides only the notch's indicator).
    public var signedInSpaceIds: Set<String> { Set(keyvault.signedIn(spaces)) }

    /// The notch's key indicator, its line and the tiles' key, without the
    /// dismissed copies.
    func syncNotchKeyvault() {
        notch.setKeyvault(label: keyvault.notchLabel, signedIn: keyvault.signedIn(spaces, notch: true))
    }

    /// A Space's "Signed in" badge: the Keyvault's Access page, with that
    /// Space's row brought forward.
    public func showAccess(spaceId: String) {
        keyvault.focusKey = spaces.first { $0.id == spaceId }.flatMap { keyvault.accessKey(for: $0) }
        keyvault.selection = .category(category: .access)
        selection = .keyvault(.category(category: .access))
    }

    /// The roster: This machine first (when this app manages a host), then
    /// the registry's Spaces.
    func recompose() {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        // This Mac's own relay entry is "This machine", not one of "My machines".
        let listed = appWithoutThisRelayMachine(spaces: registrySpaces, thisMachineId: host.machineId)
        let spaces = host.host == nil ? listed
            : appWithThisMachine(spaces: listed, status: host.summaryInput, nowMs: now)
        send(.syncSpaces(spaces: appCreatesCompose(spaces: spaces, state: creates)))
    }

    /// Advances the Spaces being created and redraws the list.
    func sendCreate(_ action: AppCreateAction) {
        // A create started, reached ready or failed (how long it took).
        telemetry?.record(appTelemetryCreates(state: creates, action: action, nowMs: Self.nowMs()))
        creates = appCreatesReduce(state: creates, action: action)
        recompose()
        tickWhileCreating()
    }

    /// While a create runs, time moves its bar within a phase that reports
    /// no fraction: the core computes it from `now` (4 Hz, as the Tauri app
    /// does).
    private var createTicker: Task<Void, Never>?

    private func tickWhileCreating() {
        let running = creates.pending.contains { $0.error == nil && $0.spaceId == nil }
        if !running {
            createTicker?.cancel()
            createTicker = nil
            return
        }
        guard createTicker == nil else { return }
        createTicker = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(Self.createTickMs))
                guard let self, !Task.isCancelled else { return }
                self.sendTick()
                self.recompose()
            }
        }
    }

    /// A tick moves the bars (and may stall a row) without redrawing twice.
    private func sendTick() {
        let now = Self.nowMs()
        telemetry?.record(appTelemetryCreates(state: creates, action: .tick(now: now), nowMs: now))
        creates = appCreatesReduce(state: creates, action: .tick(now: now))
    }

    static let createTickMs = 250
    /// How often the app's refresh checks relay sharing against the sign-in.
    static let accountCheckInterval: TimeInterval = 120

    static func nowMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

    // MARK: - List health

    /// How long one read of the registry may take before this refresh gives
    /// up on it (the read keeps going; the next refresh waits on it rather
    /// than asking again).
    public var listTimeout: Double = 20
    /// How long the host's own reads in a refresh may take.
    public var hostTimeout: Double = 15
    /// With no successful list read for this long, the daemon connection
    /// is made again (`reconnect`).
    public var listStaleAfter: TimeInterval = 45
    /// The last time the registry was read.
    public private(set) var lastListOk: Date?
    /// Since when the list has been watched (the live services came in, or
    /// the last reconnect).
    private var listWatchedSince = Date()
    /// Makes a new SDK client of the daemon (starting the daemon again when
    /// it is gone) and hands it to the backend; false when it could not.
    /// Set once the live services are in (`AppEnvironment.attach`).
    public var reconnect: (@MainActor () async -> Bool)?
    public private(set) var reconnecting = false
    /// Reconnects so far (tests and logs).
    public private(set) var reconnects = 0
    /// The registry read in flight: refreshes share it, so a read that
    /// never returns is asked once, not once a poll.
    private var rowsInFlight: Task<[AppSpaceRow], Error>?
    private var hostInFlight: Task<Void, Never>?

    /// The list poll: a refresh every `interval`, each one bounded, and a
    /// reconnect when the list went stale. The window, the notch and New UI
    /// all show what this keeps fresh.
    public func startListPoll(every interval: Duration = .seconds(10)) -> Task<Void, Never> {
        Task { @MainActor [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.refresh()
                await self.checkListHealth()
                try? await Task.sleep(for: interval)
            }
        }
    }

    /// Reconnects when the list has not been read for `listStaleAfter`
    /// (nothing before the live services are in).
    func checkListHealth(now: Date = Date()) async {
        guard startup.isReady, reconnect != nil, !reconnecting else { return }
        let since = max(lastListOk ?? listWatchedSince, listWatchedSince)
        guard now.timeIntervalSince(since) >= listStaleAfter else { return }
        await reconnectNow(reason: "the Space list was not read for \(Int(now.timeIntervalSince(since))) s")
    }

    /// A new connection to the daemon, then the list again. One at a time.
    func reconnectNow(reason: String) async {
        guard let reconnect, !reconnecting else { return }
        reconnecting = true
        reconnects += 1
        NSLog("Cua Spaces: reconnecting to the cua daemon (%@)", reason)
        // What hung on the old connection is left behind.
        rowsInFlight = nil
        hostInFlight = nil
        let ok = await reconnect()
        listWatchedSince = Date()
        reconnecting = false
        if !ok { NSLog("Cua Spaces: the reconnect did not make a new connection; trying again later") }
        await refresh()
    }

    /// The live services just came in: watch the list from now.
    func watchListFromNow() {
        listWatchedSince = Date()
    }

    /// The host's reads, shared and bounded.
    private func refreshHost() async {
        let task = hostInFlight ?? Task { @MainActor [weak self] in
            guard let self else { return }
            await self.host.refresh()
            // At launch, then every few minutes: a session that expired for
            // good (or a `cua auth logout`) pauses relay sharing too.
            await self.host.reconcileAccount(ifOlderThan: Self.accountCheckInterval)
        }
        hostInFlight = task
        let finished = await withDeadline(seconds: hostTimeout) { await task.value }
        if finished, hostInFlight == task { hostInFlight = nil }
    }

    /// The registry, shared and bounded (`TimeoutError` past `listTimeout`).
    private func readRows() async throws -> [AppSpaceRow] {
        let task = rowsInFlight ?? Task { [backend] in try await backend.rows() }
        rowsInFlight = task
        let result = await withTimeout(seconds: listTimeout) { try await task.value }
        if case .failure(TimeoutError.timedOut) = result {
            // Still running: the next refresh waits on the same read.
        } else if rowsInFlight == task {
            rowsInFlight = nil
        }
        return try result.get()
    }

    /// Re-reads the registry (and the host). Bounded: a daemon that stopped
    /// answering leaves the list as it was, and the poll reconnects.
    public func refresh() async {
        await refreshHost()
        cloudConfigured = await backend.cloudAvailable()
        do {
            let rows = try await readRows()
            lastListOk = Date()
            let now = Int64(Date().timeIntervalSince1970 * 1000)
            registrySpaces = appRowsToSpaces(rows: rows, nowMs: now)
            creates = appCreatesSettle(state: creates, spaces: registrySpaces)
            recompose()
            loaded = true
            rosterError = nil
            if selection == nil, let first = sidebar.selectedId { selection = .space(first) }
            // A preview for every running Space from the daemon's cache
            // (it survives restarts), before one is opened.
            let running = streamableSpaceIds
            Task { await thumbnails.warm(running) }
        } catch TimeoutError.timedOut {
            // The daemon is slow or gone: the list stays as it was, and the
            // poll reconnects when it stays stale (`checkListHealth`).
        } catch {
            // Errors may contain server bodies or credential-bearing URLs.
            rosterError = loaded
                ? "Could not refresh Spaces. Previously loaded rows may be out of date."
                : "Could not load Spaces. Try refreshing again."
        }
    }

    public func select(_ id: String) {
        selection = .space(id)
        send(.select(id: id, now: Int64(Date().timeIntervalSince1970 * 1000)))
    }

    // MARK: - New Space

    /// Opens New UI's New Space wizard (`on`: "Run on" preset, or nil);
    /// false when there is no New UI window to show it in. Set by the New UI
    /// window while the `web_ui` experiment is on.
    public var openWebNewSpace: ((_ on: String?) -> Bool)?
    /// Closes the New UI window (the `web_ui` experiment was turned off).
    public var closeWebUI: (() -> Void)?

    /// New Space with "Run on" set to `on` (`host:<machine>`): a machine's
    /// "New Space on <name>…".
    public func openNewSpace(on: String) async {
        if openInWebUI(on: on) { return }
        await openNativeNewSpace()
        wizard.send(.choosePlacement(on: on))
    }

    /// New Space: New UI's wizard while the `web_ui` experiment is on, else
    /// (the fallback) the native sheet.
    /// `quick`: the empty home's one click: that OS's default Space, created at once
    /// when the core says it can be; else New Space opens on it and says why.
    public func openNewSpace(quick os: AppSpaceOs? = nil) async {
        if os == nil, openInWebUI(on: nil) { return }
        await openNativeNewSpace(quick: os)
    }

    private func openInWebUI(on: String?) -> Bool {
        guard settings.experiments.webUi, let open = openWebNewSpace else { return false }
        return open(on)
    }

    /// The native New Space sheet (the web UI is off), or `quick`'s one click.
    public func openNativeNewSpace(quick os: AppSpaceOs? = nil) async {
        wizard.reset(env: await newSpaceEnv())
        if let os {
            wizard.send(.chooseOs(os: os))
            if wizard.view.canContinue { create(wizard.view.plan); return }
        }
        showingNewSpace = true
        telemetry?.record([.spaceWizard(action: "opened")])
        self.cloud.onConnected = { [weak self] in self?.cloudsChanged() }
    }

    /// What New Space knows, freshly probed (this Mac's runtimes, storage,
    /// GPUs, your machines that provide Spaces, your clouds, pricing): the
    /// native sheet's env, and New UI's (`spaces.createOptions`).
    /// Every probe runs at once and each is bounded, so the env comes in
    /// seconds even when one of them hangs (New UI's `spaces.createOptions`
    /// must answer before the page stops waiting).
    public func newSpaceEnv() async -> AppWizardEnv {
        async let runtimesProbe = backend.localRuntimes()
        async let storageProbe = backend.localStorage()
        async let pricingProbe = backend.cloudPricing()
        async let gpusProbe = backend.gpuChoices()
        async let hostsProbe = backend.hosts()
        async let cloudProbe = backend.cloudAvailable()
        async let lumeProbe = backend.lumeSource()
        async let linuxProbe = backend.linuxSource()
        async let macosVmsProbe = backend.runningMacosVms()
        await self.cloud.refresh()
        let (runtimes, storage, pricing, gpus, cloud) = await (runtimesProbe, storageProbe, pricingProbe, gpusProbe, cloudProbe)
        (lumeSource, linuxSource) = await (lumeProbe, linuxProbe)
        hosts = await hostsProbe
        // Lume did not answer this time: keep what it said last (a VM that
        // stopped since shows on the next read that answers).
        runningMacosVms = await macosVmsProbe ?? runningMacosVms
        return wizardEnv(cloud: cloud, runtimes: runtimes, storage: storage, pricing: pricing, gpus: gpus)
    }

    /// What New Space knows without asking anything (no probe, no wait):
    /// the env New UI gets while the live services are still starting.
    public func knownNewSpaceEnv() -> AppWizardEnv {
        wizardEnv(cloud: false, runtimes: nil, storage: nil, pricing: nil, gpus: nil)
    }

    /// The live services are in (the launch's stand-ins have them): until
    /// then every probe would wait for them.
    public var servicesIn: Bool {
        guard let pending = backend as? PendingSpacesBackend else { return true }
        return pending.gate.current != nil
    }

    /// macOS VMs running on this Mac (Lume's, Spaces or not), as New Space
    /// last read them; nil when Lume did not say. Apple's license allows two.
    public private(set) var runningMacosVms: Int?

    /// The connected clouds changed ("Connect a cloud"): the open wizard
    /// reads them again, where it is.
    func cloudsChanged() {
        wizard.update(env: wizardEnv(from: wizard.env))
    }

    /// Your machines that provide Spaces, as New Space last read them.
    var hosts: [AppSpaceHost] = []

    /// Which Lume macOS Spaces run on (`runtime.lume`), once read.
    public private(set) var lumeSource: String?
    /// Which engine local Linux Spaces run on (`runtime.linux`), once read.
    public private(set) var linuxSource: String?

    /// New Space's "Use built-in Lume" / "Use built-in runtime": switches
    /// `runtime.lume` or `runtime.linux`, then the open wizard reads the
    /// runtimes again (This Mac can run it now).
    public func applyRuntimeSwitch(_ change: AppRuntimeSwitch) async {
        let linux = change.setting == "runtime.linux"
        do {
            if linux {
                try await backend.setLinuxSource(change.value)
            } else {
                try await backend.setLumeSource(change.value)
            }
        } catch {
            show(error: LiveSpacesBackend.words(error))
            return
        }
        if linux {
            linuxSource = await backend.linuxSource() ?? change.value
        } else {
            lumeSource = await backend.lumeSource() ?? change.value
        }
        let runtimes = await backend.localRuntimes()
        var e = wizard.env
        let backends = runtimes?.ready
        e.localAvailable = backends.map { !$0.isEmpty } ?? true
        e.localReason = backends?.isEmpty == true ? "No local runtime found (Docker or Lume)." : nil
        e.localBackends = backends
        e.localDetails = runtimes?.details
        e.lumeSource = lumeSource
        e.linuxSource = linuxSource
        wizard.update(env: e)
    }

    private func wizardEnv(from e: AppWizardEnv) -> AppWizardEnv {
        var e = e
        e.clouds = cloud.clouds
        e.hosts = hosts
        e.experiments = settings.experiments
        if appIsCloudWord(on: cloud.defaultOn ?? "") { e.defaultLocation = .yours }
        return e
    }

    private func wizardEnv(cloud available: Bool, runtimes: (ready: [String], details: [String: String])?,
                           storage: LocalStorage?, pricing: AppCloudPricing?,
                           gpus: [AppGpuChoice]?) -> AppWizardEnv {
        let backends = runtimes?.ready
        return wizardEnv(from: AppWizardEnv(
            defaultLocation: settings.defaultLocation, cloudAvailable: available,
            localAvailable: backends.map { !$0.isEmpty } ?? true,
            localReason: backends?.isEmpty == true ? "No local runtime found (Docker or Lume)." : nil,
            localBackends: backends, localDetails: runtimes?.details, maxCpus: wizard.env.maxCpus,
            hostArch: Self.hostArch, lumeSource: lumeSource, linuxSource: linuxSource,
            storage: storage.map(Self.wizardStorage),
            cloudPricing: available ? pricing : nil, clouds: [], hosts: [],
            experiments: settings.experiments, gpus: gpus))
    }

    /// The New Space panel's Cancel.
    public func cancelNewSpace() {
        showingNewSpace = false
        telemetry?.record([.spaceWizard(action: "cancelled")])
    }

    /// Records a Spaces app feature (a fixed name; the Tauri app uses the
    /// same words).
    public func recordFeature(_ name: String) {
        telemetry?.record(appTelemetryFeature(feature: name))
    }

    /// Where views send usage events.
    public var telemetrySink: TelemetryRunning? { telemetry }

    /// This Mac's CPU architecture, as the image catalog spells it (the
    /// app core's): the wizard's platform, a pending create's, and the
    /// emulation warning on a Space's Architecture.
    static let hostArch = appHostArch()

    /// The SDK's storage probe as the wizard env takes it.
    static func wizardStorage(_ s: LocalStorage) -> AppLocalStorage {
        func volume(_ v: StorageVolume?) -> AppStorageVolume? {
            v.map { AppStorageVolume(availableBytes: $0.availableBytes, totalBytes: $0.totalBytes, name: $0.name) }
        }
        return AppLocalStorage(reserveBytes: s.reserveBytes, lume: volume(s.lume), qemu: volume(s.qemu),
                               container: volume(s.container), pulled: s.pulled)
    }

    /// "Create Space": the new Space shows in the list and the notch at
    /// once (the core's pending row), follows the SDK's create progress, and
    /// hands over to the registry's row when it is ready. A failure stays on
    /// its row; a cancelled create (Cancel, or `cua spaces cancel`
    /// elsewhere) only leaves the list.
    public func create(_ plan: AppCreatePlan) {
        showingNewSpace = false
        telemetry?.record([.spaceWizard(action: "submitted")])
        let args = appWizardCreateArgs(plan: plan)
        let pendingId = "pending:\(UUID().uuidString.lowercased())"
        startCreate(args, os: plan.image.os, pendingId: pendingId, select: plan.openDesktop)
        // A failure stays on its row.
        Task { _ = try? await followCreate(args, pendingId: pendingId) }
    }

    /// Runs a create from the core's create arguments: the native sheet's
    /// (`create`) and New UI's (`spaces.create`, with the page's pending id).
    /// The new Space shows in the list and the notch at once (the core's
    /// pending row), follows the SDK's progress (also handed to `progress`)
    /// and hands over to the registry's row when it is ready; returns its
    /// id. A failure stays on its row; a cancelled create (Cancel, or `cua
    /// spaces cancel` elsewhere) only leaves the list. Either way it throws.
    @discardableResult
    public func runCreate(_ args: AppCreateSpaceArgs, os: AppSpaceOs, pendingId: String,
                          progress: (@MainActor @Sendable (SpaceCreateProgress) -> Void)? = nil) async throws -> String {
        startCreate(args, os: os, pendingId: pendingId, select: false)
        return try await followCreate(args, pendingId: pendingId, progress: progress)
    }

    /// The pending row (selected when `open`).
    private func startCreate(_ args: AppCreateSpaceArgs, os: AppSpaceOs, pendingId: String, select open: Bool) {
        // A Space on this Mac is reached over the local network (a macOS
        // VM on vmnet): ask now, while the person who pressed Create is
        // here, not when the VM boots after the download.
        if args.on == "local" { host.requestLocalNetwork() }
        // A create on one of your machines (`host:<machine>`) says which, so
        // that machine's own record of the Space it is creating is not a
        // second row next to this one, and a failure names the machine.
        let machine = args.on.hasPrefix("host:") ? String(args.on.dropFirst("host:".count)) : nil
        sendCreate(.start(id: pendingId, name: args.name ?? "", os: os,
                          provider: args.on == "cloud" ? .cloud : args.on == "local" ? .local : .relay,
                          now: Self.nowMs(),
                          image: args.image, kind: args.kind, hostArch: Self.hostArch, gpu: args.gpu != nil,
                          host: machine, hostName: machine.flatMap(machineName(of:))))
        // The detail shows the live desktop inline: selecting the new Space
        // opens it (`plan.openDesktop`), starting with its progress.
        if open { select(pendingId) }
    }

    /// The name of one of your machines, from the Spaces it provides (or its
    /// own entry on the relay).
    private func machineName(of id: String) -> String? {
        spaces.first { $0.id == "relay:\(id)" }?.name
            ?? spaces.first { $0.host == id && $0.hostName != nil }?.hostName
    }

    /// How long the daemon may take to take a create (its first progress
    /// report, "Preparing", comes as it starts): past it the create fails
    /// at once, with Try again, instead of after the 4 min stall.
    public var createAcceptTimeout: Double = 30

    /// The create the daemon never took.
    struct CreateNotAccepted: LocalizedError {
        let seconds: Int
        var errorDescription: String? {
            "Cua's background service didn't start this create within \(seconds) s. "
                + "Cua Spaces reconnected to it. Try again."
        }
    }

    /// The SDK's create, through to the registry's row.
    private func followCreate(_ args: AppCreateSpaceArgs, pendingId: String,
                              progress: (@MainActor @Sendable (SpaceCreateProgress) -> Void)? = nil) async throws -> String {
        do {
            let accepted = CreateAcceptance()
            let backend = self.backend
            let create = Task {
                try await backend.create(args, createId: pendingId) { p in
                    accepted.mark()
                    Task { @MainActor [weak self] in
                        self?.sendCreate(.progress(id: pendingId, phase: p.phase,
                                                   fraction: p.fraction, now: Self.nowMs(),
                                                   bytesDone: p.bytesDone, bytesTotal: p.bytesTotal,
                                                   bytesPerSecond: p.bytesPerSecond))
                        progress?(p)
                    }
                }
            }
            let id = try await awaitAccepted(create, accepted: accepted, pendingId: pendingId)
            sendCreate(.finish(id: pendingId, spaceId: id))
            await refresh()
            if selectedSpaceId == pendingId { select(id) }
            // The registry lists it now; the pending row is gone from
            // the list already, so drop it from the state too.
            sendCreate(.dismiss(id: pendingId))
            return id
        } catch where isCancelled(error) {
            // Cancelled, here or elsewhere: the row goes; it did not fail.
            sendCreate(.cancelDone(id: pendingId))
            throw error
        } catch {
            sendCreate(.fail(id: pendingId, error: LiveSpacesBackend.words(error)))
            throw error
        }
    }

    /// The create's result, or `CreateNotAccepted` when the daemon reported
    /// nothing within `createAcceptTimeout` (then the create is cancelled
    /// where it may still arrive, and the connection is made again).
    private func awaitAccepted(_ create: Task<String, Error>, accepted: CreateAcceptance,
                               pendingId: String) async throws -> String {
        let limit = createAcceptTimeout
        let first = await withTimeout(seconds: limit) { try await create.value }
        switch first {
        case .success(let id): return id
        case .failure(TimeoutError.timedOut) where !accepted.done:
            NSLog("Cua Spaces: the cua daemon did not take create %@ within %d s", pendingId, Int(limit))
            let backend = self.backend
            // Best effort: it may still reach the daemon over the old
            // connection; never wait on it.
            Task.detached { _ = await withTimeout(seconds: 30) { try await backend.cancelCreate(createId: pendingId) } }
            Task { @MainActor [weak self] in await self?.reconnectNow(reason: "a create was not taken") }
            throw CreateNotAccepted(seconds: Int(limit.rounded(.up)))
        case .failure(TimeoutError.timedOut):
            // Taken: it runs as long as its phases do.
            return try await create.value
        case .failure(let error):
            throw error
        }
    }

    /// Cancel on a Space still being created: the row shows Cancelling
    /// until the SDK stopped the create and removed what it made, then goes;
    /// a cancel that fails says why on the row.
    public func cancelCreate(_ pendingId: String) {
        guard appCreatesIsPending(id: pendingId),
              creates.pending.contains(where: { $0.id == pendingId && !$0.cancelling }) else { return }
        sendCreate(.cancelStart(id: pendingId))
        Task {
            do {
                try await backend.cancelCreate(createId: pendingId)
                sendCreate(.cancelDone(id: pendingId))
            } catch {
                sendCreate(.cancelFail(id: pendingId, error: LiveSpacesBackend.words(error)))
            }
        }
    }

    public func addByAddress(url: String, token: String?, name: String?) async throws {
        try await backend.add(url: url, token: token, name: name)
        showingNewSpace = false
        await refresh()
    }

    /// Whether the Space is being deleted (its row shows Deleting).
    public func isDeleting(_ id: String) -> Bool {
        appCreatesIsDeleting(state: creates, id: id)
    }

    /// Delete (after the confirmation): the row shows Deleting at once and
    /// nothing streams from it any more; it goes when the SDK's delete
    /// returns. A failure restores it with the banner. A second Delete on a
    /// Space already deleting does nothing. `removeOnly` forgets any Space
    /// and keeps it running (Remove from List for a Space in your cloud).
    public func delete(_ space: AppSpace, removeOnly: Bool = false) {
        confirmDeleteId = nil
        // A failed create is only removed from the list.
        if appCreatesIsPending(id: space.id) {
            sendCreate(.dismiss(id: space.id))
            return
        }
        guard !isDeleting(space.id) else { return }
        recordFeature("space_delete")
        let detail = appSpaceDetail(space: space)
        sendCreate(.deleteStart(id: space.id, now: Int64(Date().timeIntervalSince1970 * 1000)))
        usage[space.id] = nil
        notch.thumbnails[space.id] = nil
        Task {
            do {
                try await backend.remove(id: space.id, removeOnly: removeOnly || detail.removeOnly)
                sendCreate(.deleteDone(id: space.id))
                await refresh()
            } catch {
                sendCreate(.deleteFail(id: space.id))
                show(error: appDeleteFailedText(name: space.name, error: LiveSpacesBackend.words(error)))
            }
        }
    }

    /// The power button next to Delete: turns the Space off (`on` false:
    /// suspended or stopped, as its provider can) or back on. The row and
    /// the detail say Suspending (and the like) at once and the button
    /// waits; they keep saying so until the registry shows the Space off (or
    /// on). A failure shows inline on the row and the detail. A second
    /// press while one runs does nothing.
    public func setPower(_ space: AppSpace, on: Bool) {
        guard !appCreatesIsPowering(state: creates, id: space.id) else { return }
        sendCreate(.powerStart(id: space.id, on: on, now: Self.nowMs()))
        if !on {
            usage[space.id] = nil
            notch.thumbnails[space.id] = nil
        }
        Task {
            do {
                try await backend.setPower(id: space.id, on: on)
                sendCreate(.powerDone(id: space.id))
                await refresh()
            } catch {
                sendCreate(.powerFail(id: space.id, error: LiveSpacesBackend.words(error)))
            }
        }
    }

    // MARK: - Settings and banners

    public func saveSettings() {
        try? appSettingsSave(path: settingsPath, settings: settings)
    }

    public func show(error: String) {
        banner = error
        bannerIsError = true
    }

    public func show(info: String) {
        banner = info
        bannerIsError = false
    }

    /// The menu bar item's status line.
    /// Counts the Spaces the user can open, the notch's count (This machine
    /// only while it is shared and reachable).
    public var statusLine: String { appStatusLine(count: appOpenableCount(spaces: spaces)) }

    /// The menu bar item's menu: the count, Cua Volume's sync state next to
    /// it and its conflicts, then the actions.
    public var menuBar: [AppMenuItem] {
        let items = appMenu(input: AppMenuInput(spaces: spaces, keyvault: keyvault.sharingLabel, sync: persistent.driveSync,
                                   nowMs: UInt64(Date().timeIntervalSince1970 * 1000),
                                   backend: persistent.driveBackend, experiments: settings.experiments))
        // Still launching (opened at login, waiting for Keychain access):
        // say so first, so the menu explains why nothing shows yet.
        let title = startup.copy.title
        guard !startup.isReady, !title.isEmpty else { return items }
        return [AppMenuItem(id: .status, label: title, shortcut: nil, enabled: false),
                AppMenuItem(id: .separator, label: "", shortcut: nil, enabled: false)] + items
    }

    /// The window chrome (account line, New Space, empty state).
    public var chrome: AppMainChrome {
        appMainChrome(input: AppChromeInput(identity: identity, cloudConfigured: cloudConfigured,
                                            canSignIn: account != nil, experiments: settings.experiments))
    }

    // MARK: - Account

    /// "Sign in" / "Sign in to Cua": the browser flow, the code while it waits.
    public func beginSignIn() async {
        guard let account, signIn != .starting else { return }
        signIn = .starting
        signInAttempt += 1
        let current = signInAttempt
        do {
            let attempt = try await account.beginSignIn()
            // Cancelled while it started.
            guard signInAttempt == current, signIn == .starting else { return }
            signIn = .waiting(userCode: attempt.userCode)
            signInURL = attempt.url
            let timeout = signInTimeout
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: timeout)
                guard let self, self.signInAttempt == current, case .waiting = self.signIn else { return }
                self.signInFailed("The sign-in timed out. Try again.")
            }
            let who = try await attempt.wait()
            guard signInAttempt == current, case .waiting = signIn else { return }
            identity = who ?? account.identity()
            host.identity = identity
            onboarding.host.identity = identity
            devices.signedIn = identity != nil
            // Activation funnel: the first run records `signed_in` on its
            // sign-in page; a sign-in from the main window (first run done or
            // not showing, so its state is on Welcome) records it here.
            if onboarding.state.step == .welcome, let who = identity, !who.isEmpty,
               who != onboarding.state.identity {
                telemetry?.record([.step(step: "signed_in", ok: true)])
            }
            onboarding.send(.signedIn(identity: identity ?? ""))
            signIn = .idle
            signInURL = nil
            // Relay sharing paused while signed out comes back for its
            // owner (another account is asked to set it up again).
            await host.reconcileAccount()
        } catch {
            guard signInAttempt == current, signIn != .idle else { return }
            signInFailed(LiveSpacesBackend.words(error))
        }
    }

    /// Shown where the sign-in was started; counted by its kind only (the
    /// core keeps the words).
    private func signInFailed(_ message: String) {
        signIn = .failed(message: message)
        signInURL = nil
        telemetry?.record(appTelemetrySignInFailed(message: message))
    }

    /// Stops waiting for the browser (the page can start again).
    public func cancelSignIn() {
        switch signIn {
        case .starting, .waiting:
            signInAttempt += 1
            telemetry?.record(appTelemetrySignInFailed(message: nil))
        default: break
        }
        signIn = .idle
        signInURL = nil
    }

    public func signOut() async {
        try? await account?.signOut()
        identity = nil
        host.identity = nil
        onboarding.host.identity = nil
        devices.signedIn = false
        // Signed out: relay sharing stops (the setup stays to resume).
        await host.reconcileAccount()
        await devices.refresh()
        signIn = .idle
    }

    // MARK: - Settings page

    public var settingsPage: AppSettingsPage {
        appSettingsPage(input: AppSettingsInput(
            identity: identity, apiKeyClient: nil, signIn: signIn, canSignOut: account != nil && identity != nil,
            menuBar: settings.menuBar, defaultLocation: settings.defaultLocation, locationLockedBy: nil,
            telemetry: telemetryInput, agents: agentRows, agentsBusy: agentsBusy, agentsPending: agentsPending,
            billing: identity == nil ? nil : billingStatus, loginItem: loginItemInput,
            experiments: settings.experiments, keyvaultAutoWipe: keyvault.autoWipe,
            keyvaultUnlockPrompt: keyvault.unlockPromptShows,
            keyvaultSiteIcons: settings.keyvaultSiteIcons, keyvaultProtection: keyvault.page.protection,
            autoConnect: settings.autoConnect, lumeSource: lumeSource, linuxSource: linuxSource))
    }

    /// Settings, General with Settings, Storage after General while the Cua
    /// Volume experiment is on (the core's rule).
    public var settingsPageWithStorage: AppSettingsPage {
        appSettingsWithStorage(page: settingsPage, storage: storage.section, experiments: settings.experiments)
    }

    // MARK: - Experiments

    /// Settings, Experiments: a switch and one line per experiment.
    public var experimentsPage: AppSettingsPage { appExperimentsPage(experiments: settings.experiments) }

    /// A switch in Settings, Experiments: saved, recorded, and followed by
    /// the first run and an open New Space. Off hides; nothing is undone.
    public func chooseExperiment(row: String, option: String) {
        let before = settings.experiments
        let after = appExperimentsChoose(experiments: before, row: row, option: option)
        let signals = appTelemetryExperimentsChanged(before: before, after: after)
        guard !signals.isEmpty else { return }
        settings.experiments = after
        saveSettings()
        telemetry?.record(signals)
        onboarding.send(.experimentsLoaded(experiments: after))
        // New UI off: its window goes with it (the native one takes over).
        if before.webUi, !after.webUi { closeWebUI?() }
        wizard.update(env: wizardEnv(from: wizard.env))
        // The Volume page went with Cua Volume: back to the Spaces.
        if selection == .drive, chrome.volumeLabel == nil { selection = nil }
    }

    // MARK: - Launch at login

    /// The Settings toggle's state: the system's, never the one asked for.
    var loginItemInput: AppLoginItemInput? {
        loginItemStatus.map {
            AppLoginItemInput(status: $0, busy: loginItemBusy, error: loginItemError,
                              providesSpaces: providesSpaces, runsAgents: persistent.agentCount > 0)
        }
    }

    /// This Mac provides Spaces to your other devices.
    var providesSpaces: Bool { host.state.map { $0.configured && $0.provideSpaces } ?? false }

    /// Reads what the system reports.
    public func readLoginItem() {
        loginItemStatus = loginItem?.status()
    }

    /// The user turned it on or off (Settings, or the first run's Done):
    /// saved as their choice, applied, and read back.
    public func setLaunchAtLogin(_ on: Bool) {
        guard let loginItem, !loginItemBusy else { return }
        settings.launchAtLogin = on
        saveSettings()
        loginItemBusy = true
        loginItemError = nil
        do {
            if on { try loginItem.register() } else { try loginItem.unregister() }
            telemetry?.record([.feature(feature: on ? "launch_at_login_on" : "launch_at_login_off")])
        } catch {
            loginItemError = error.localizedDescription
        }
        loginItemStatus = loginItem.status()
        loginItemBusy = false
    }

    /// At launch, once the first run is done: an install that never chose
    /// (its first run predates the setting) is turned on when this Mac
    /// provides Spaces or runs persistent agents (the core's rule); a choice
    /// stands.
    public func applyLaunchAtLogin() async {
        guard let loginItem, onboarding.completed, settings.launchAtLogin == nil else {
            readLoginItem()
            return
        }
        await host.refresh()
        await persistent.loadAgents()
        let status = loginItem.status()
        let plan = appLoginItemLaunchPlan(choice: settings.launchAtLogin, onboarded: true,
                                          serves: providesSpaces || persistent.agentCount > 0, status: status)
        if plan.register {
            do { try loginItem.register() } catch { NSLog("Cua Spaces: launch at login: %@", "\(error)") }
        }
        if let record = plan.record {
            settings.launchAtLogin = record
            saveSettings()
        }
        readLoginItem()
    }

    /// Reads what Settings shows (telemetry, the coding agents).
    public func loadSettings() async {
        lumeSource = await backend.lumeSource()
        linuxSource = await backend.linuxSource()
        telemetryInput = telemetry?.status()
        readLoginItem()
        await storage.load()
        await reloadBilling()
        await reloadAgents()
        await keyvault.refresh()
    }

    func reloadBilling() async {
        guard identity != nil, let billing else {
            billingStatus = nil
            return
        }
        billingStatus = try? await billing.status()
    }

    /// Opens a website page (Manage billing, Add credit, the Teams waitlist).
    public func openBillingPage(_ raw: String) {
        guard let url = URL(string: raw), url.scheme == "https" || url.scheme == "http" else { return }
        BillingBrowser.open(url)
    }

    func reloadAgents() async {
        guard let agentSetup else {
            agentRows = []
            return
        }
        let rows = appAgentSettingsRows(statuses: await agentSetup.statuses(), total: agentSetup.skillsTotal())
        agentRows = rows.map { r in
            guard let why = agentFailures[r.agent] else { return r }
            var r = r
            r.configured = false
            r.detail = why
            return r
        }
    }

    /// A choice row changed.
    public func choose(row: String, option: String) async {
        switch row {
        case "notch":
            settings.menuBar = option == "hide"
            saveSettings()
        case "macos-runtime":
            do {
                try await backend.setLumeSource(option)
                lumeSource = await backend.lumeSource() ?? option
            } catch {
                show(error: LiveSpacesBackend.words(error))
            }
        case "linux-runtime":
            do {
                try await backend.setLinuxSource(option)
                linuxSource = await backend.linuxSource() ?? option
            } catch {
                show(error: LiveSpacesBackend.words(error))
            }
        case "default-location":
            settings.defaultLocation = option == "cloud" ? .cloud : .local
            saveSettings()
        case "auto-connect":
            settings.autoConnect = option == "on"
            saveSettings()
        case "launch-at-login":
            setLaunchAtLogin(option == "on")
        case "keyvault-auto-wipe":
            await keyvault.setAutoWipe(option == "on")
            if let error = keyvault.error { show(error: error) }
        case "keyvault-site-icons":
            settings.keyvaultSiteIcons = option == "on"
            saveSettings()
            // Turning it on asks for the rows' icons again.
            if settings.keyvaultSiteIcons { await keyvault.loadIcons() }
        case "keyvault-unlock-prompt":
            // On shows the prompt (Never ask again off); off is the stored
            // "Never ask again".
            await keyvault.setSkipUnlockPrompt(option != "on")
            if let error = keyvault.error { show(error: error) }
        case "telemetry":
            do {
                telemetryInput = try telemetry?.setEnabled(option == "on")
            } catch {
                show(error: LiveSpacesBackend.words(error))
            }
        default: break
        }
    }

    /// A row's button.
    public func press(row: String) async {
        switch row {
        case "account": await signOut()
        case "sign-in": await beginSignIn()
        case "sign-in-code": cancelSignIn()
        case "welcome": onboarding.restart()
        case "launch-at-login-approve":
            loginItem?.openSystemSettings()
        case "billing", "teams":
            // A website page: billing (hidden while the apps do not offer
            // Cua Cloud) or the Teams waitlist.
            if let url = settingsPage.sections.flatMap(\.rows).first(where: { $0.id == row })?.linkUrl {
                openBillingPage(url)
            }
        default:
            guard row.hasPrefix("agent:"), let r = agentRows?.first(where: { "agent:\($0.agent)" == row }) else { return }
            await agentAction([r.agent], remove: r.configured)
        }
    }

    /// "Configure all detected agents".
    public func configureAllAgents() async {
        guard !agentsBusy else { return }
        let ids = (agentRows ?? []).filter(\.installed).map(\.agent)
        agentsBusy = true
        await agentAction(ids, remove: false)
        agentsBusy = false
    }

    func agentAction(_ ids: [String], remove: Bool) async {
        guard let agentSetup, !ids.isEmpty else { return }
        agentsPending += ids
        defer { agentsPending.removeAll { ids.contains($0) } }
        do {
            let outcomes = remove ? try await agentSetup.remove(agents: ids)
                : try await agentSetup.setUp(agents: ids, skills: true, mcp: true)
            for id in ids {
                let s = appAgentSetupSummary(outcomes: outcomes, agent: id, name: id)
                agentFailures[id] = s.failed.isEmpty ? nil : s.failed.joined(separator: "; ")
            }
        } catch {
            for id in ids { agentFailures[id] = LiveSpacesBackend.words(error) }
        }
        await reloadAgents()
    }
}

/// Whether the daemon reported anything about a create yet.
final class CreateAcceptance: @unchecked Sendable {
    private let lock = NSLock()
    private var reported = false
    func mark() { lock.withLock { reported = true } }
    var done: Bool { lock.withLock { reported } }
}
