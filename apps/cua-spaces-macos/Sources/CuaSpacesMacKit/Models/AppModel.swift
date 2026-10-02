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
    public private(set) var roster: AppRosterState
    public var query = ""
    public var selection: MainSelection?
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
                presence: PresenceChecking = LivePresence(), loginItem: LoginItemControlling? = nil) {
        self.backend = backend
        self.devices = DevicesModel(devices: devices, presence: presence)
        self.persistent = PersistentModel(tools: backend as? AgentsToolRunning)
        self.storage = StorageModel(tools: backend as? AgentsToolRunning)
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
            hostArch: Self.hostArch, storage: nil, cloudPricing: nil, clouds: [], hosts: [],
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
            self?.settings.experiments ?? AppExperiments(cuaVolume: false, yourCloud: false, sharing: false)
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
        // A row takes a window drop by the core's rule (the notch tiles' too).
        dropTargets.isDropTarget = { [weak self] id in
            self?.spaces.first { $0.id == id }.map { appSpaceAcceptsDrop(space: $0) } ?? false
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
        spaces.filter { appSpaceDetail(space: $0).canStream }.map(\.id)
    }

    /// What the Space's preview card shows over (or instead of) its live
    /// desktop, from the core.
    public func cover(_ detail: AppSpaceDetail, requested: Bool, stream: AppStreamPhase) -> AppDesktopCover {
        appDesktopCover(input: AppDesktopCoverInput(
            canStream: detail.canStream, previewText: detail.previewText,
            autoConnect: settings.autoConnect, connectRequested: requested, stream: stream))
    }

    /// The detail without what Settings, Experiments hides (Share while
    /// Sharing is off).
    public func detail(_ space: AppSpace) -> AppSpaceDetail {
        appSpaceDetailWith(space: space, usage: usage[space.id], hostArch: Self.hostArch,
                           experiments: settings.experiments)
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
        let spaces = host.host == nil ? registrySpaces
            : appWithThisMachine(spaces: registrySpaces, status: host.summaryInput, nowMs: now)
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

    static func nowMs() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }

    /// Re-reads the registry (and the host).
    public func refresh() async {
        await host.refresh()
        cloudConfigured = await backend.cloudAvailable()
        do {
            let rows = try await backend.rows()
            let now = Int64(Date().timeIntervalSince1970 * 1000)
            registrySpaces = appRowsToSpaces(rows: rows, nowMs: now)
            creates = appCreatesSettle(state: creates, spaces: registrySpaces)
            recompose()
            loaded = true
            if selection == nil, let first = sidebar.selectedId { selection = .space(first) }
            // A preview for every running Space from the daemon's cache
            // (it survives restarts), before one is opened.
            let running = streamableSpaceIds
            Task { await thumbnails.warm(running) }
        } catch {
            show(error: "Could not list Spaces: \(LiveSpacesBackend.words(error))")
        }
    }

    public func select(_ id: String) {
        selection = .space(id)
        send(.select(id: id, now: Int64(Date().timeIntervalSince1970 * 1000)))
    }

    // MARK: - New Space

    public func openNewSpace() async {
        async let runtimesProbe = backend.localRuntimes()
        async let storageProbe = backend.localStorage()
        async let pricingProbe = backend.cloudPricing()
        async let gpusProbe = backend.gpuChoices()
        async let hostsProbe = backend.hosts()
        let cloud = await backend.cloudAvailable()
        await self.cloud.refresh()
        let (runtimes, storage, pricing, gpus) = await (runtimesProbe, storageProbe, pricingProbe, gpusProbe)
        hosts = await hostsProbe
        wizard.reset(env: wizardEnv(cloud: cloud, runtimes: runtimes, storage: storage, pricing: pricing,
                                    gpus: gpus))
        showingNewSpace = true
        telemetry?.record([.spaceWizard(action: "opened")])
        self.cloud.onConnected = { [weak self] in self?.cloudsChanged() }
    }

    /// The connected clouds changed ("Connect a cloud"): the open wizard
    /// reads them again, where it is.
    func cloudsChanged() {
        wizard.update(env: wizardEnv(from: wizard.env))
    }

    /// Your machines that provide Spaces, as New Space last read them.
    var hosts: [AppSpaceHost] = []

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
            hostArch: Self.hostArch, storage: storage.map(Self.wizardStorage),
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
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        sendCreate(.start(id: pendingId, name: args.name ?? "", os: plan.image.os,
                          provider: args.on == "cloud" ? .cloud : args.on == "local" ? .local : .relay,
                          now: now,
                          image: args.image, kind: args.kind, hostArch: Self.hostArch, gpu: args.gpu != nil))
        // The detail shows the live desktop inline: selecting the new Space
        // opens it (`plan.openDesktop`), starting with its progress.
        if plan.openDesktop { select(pendingId) }
        Task {
            do {
                let id = try await backend.create(args, createId: pendingId) { progress in
                    Task { @MainActor [weak self] in
                        self?.sendCreate(.progress(id: pendingId, phase: progress.phase,
                                                   fraction: progress.fraction, now: Self.nowMs(),
                                                   bytesDone: progress.bytesDone, bytesTotal: progress.bytesTotal,
                                                   bytesPerSecond: progress.bytesPerSecond))
                    }
                }
                sendCreate(.finish(id: pendingId, spaceId: id))
                await refresh()
                if selectedSpaceId == pendingId { select(id) }
                // The registry lists it now; the pending row is gone from
                // the list already, so drop it from the state too.
                sendCreate(.dismiss(id: pendingId))
            } catch where isCancelled(error) {
                // Cancelled, here or elsewhere: the row goes; it did not fail.
                sendCreate(.cancelDone(id: pendingId))
            } catch {
                sendCreate(.fail(id: pendingId, error: LiveSpacesBackend.words(error)))
            }
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
        appMenu(input: AppMenuInput(spaces: spaces, keyvault: keyvault.sharingLabel, sync: persistent.driveSync,
                                   nowMs: UInt64(Date().timeIntervalSince1970 * 1000),
                                   backend: persistent.driveBackend, experiments: settings.experiments))
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
        do {
            let attempt = try await account.beginSignIn()
            signIn = .waiting(userCode: attempt.userCode)
            let who = try await attempt.wait()
            guard case .waiting = signIn else { return }
            identity = who ?? account.identity()
            host.identity = identity
            devices.signedIn = identity != nil
            onboarding.send(.signedIn(identity: identity ?? ""))
            signIn = .idle
        } catch {
            signIn = .failed(message: LiveSpacesBackend.words(error))
        }
    }

    public func cancelSignIn() { signIn = .idle }

    public func signOut() async {
        try? await account?.signOut()
        identity = nil
        host.identity = nil
        devices.signedIn = false
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
            autoConnect: settings.autoConnect))
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
