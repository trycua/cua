// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation

/// The Command line page's installer of the bundled `cua`: the app core's,
/// through the cua SDK (`AppCliInstaller`), the same one the Tauri app runs.
public protocol CliInstallRunning: AnyObject, Sendable {
    func plan() async -> AppCliInstallPlan
    func install(request: AppCliInstallRequest) async throws -> AppCliInstallPlan
}

extension AppCliInstaller: CliInstallRunning {}

/// First run: the core's step order and answers, plus the writes the flow
/// runs through the shared core: installing the bundled `cua` command after
/// its page, and host setup.
@MainActor
@Observable
public final class OnboardingModel {
    public private(set) var state: AppOnboardingState
    public private(set) var completed: Bool
    let statePath: String?
    let cli: CliInstallRunning?
    let agentSetup: AgentSetupRunning?
    /// This machine's host setup (the same form and host as the main window).
    public let host: HostModel
    /// The signed-in account's token, for relay host setup.
    public var accountToken: ((_ forceRefresh: Bool) async throws -> String?)? {
        get { host.accountToken }
        set { host.accountToken = newValue }
    }
    /// Welcome was left with its usage-data switch at this value (live:
    /// writes the machine's setting when it changed, then records that the
    /// notice was shown; nothing is sent before).
    public var onWelcomeLeft: ((Bool) -> Void)?
    /// Where the first run's usage events go (the app core derives them;
    /// the Tauri app sends the same ones for the same steps), and the
    /// machine's telemetry setting Welcome's switch starts from.
    public var telemetry: TelemetryRunning?
    /// The setting was read for this run.
    private var shownOnce = false
    /// The fixed words.
    public let copy = appOnboardingCopy()

    /// Where Cua Spaces shows up was picked (menu bar only or not): the app
    /// saves it as the "Spaces tab in the notch" setting.
    public var onPresentation: ((Bool) -> Void)?
    /// The setting now (the presentation page starts on it).
    public var currentMenuBar: (() -> Bool)?
    /// Done's "Launch at login" checkbox, applied when the first run
    /// finishes (the app registers it, or not).
    public var onLaunchAtLogin: ((Bool) -> Void)?
    /// Settings, Experiments now: the pages follow them (the Cua Volume page
    /// only with Cua Volume on).
    public var currentExperiments: (() -> AppExperiments)?

    // AI agents
    public private(set) var agentStatuses: [AppAgentSetupStatus]?
    public var agentSelection: Set<String> = []
    public var agentSkills = true
    public var agentMcp = true
    /// The background computer-use card: also set up cua-driver (on by
    /// default, like the skills and MCP switches).
    public var agentDriver = true
    public private(set) var agentsBusy = false
    public private(set) var agentsError: String?
    public private(set) var agentOutcomes: [AppAgentSetupOutcomeInput]?

    public init(statePath: String?, installerMode: AppOnboardingMode? = nil, identity: String? = nil,
                cli: CliInstallRunning? = nil, host: HostRunning? = nil, agentSetup: AgentSetupRunning? = nil) {
        self.statePath = statePath
        self.cli = cli
        self.agentSetup = agentSetup
        self.host = HostModel(host: host)
        self.host.identity = identity
        self.state = appOnboardingInitial(installerMode: installerMode, identity: identity)
        self.completed = statePath.flatMap { Self.readCompleted($0) } ?? false
    }

    public var view: AppOnboardingView { appOnboardingView(state: state) }

    public func send(_ action: AppOnboardingAction) {
        let before = state
        state = appOnboardingReduce(state: before, action: action)
        // Leaving Welcome: its usage-data switch decides before anything
        // is sent (the core derives nothing while Welcome shows).
        if before.step == .welcome, state.step != .welcome {
            onWelcomeLeft?(appOnboardingView(state: before).usage?.on ?? true)
        }
        telemetry?.record(appTelemetryOnboarding(state: before, action: action))
    }

    /// The first page showed: Welcome's usage-data switch starts from the
    /// machine's setting (Settings, Privacy; `cua telemetry off`).
    /// Each time Welcome shows (Back included), `welcome-shown`: on a
    /// machine that already showed the usage notice, the core counts the
    /// run now (once per run), not only when Welcome is left.
    public func shown() {
        if !shownOnce {
            shownOnce = true
            if let x = currentExperiments?() { send(.experimentsLoaded(experiments: x)) }
            if let t = telemetry?.status() { send(.telemetryLoaded(telemetry: t)) }
        }
        if state.step == .welcome { send(.welcomeShown) }
    }

    /// Welcome's "Share anonymous usage data" switch.
    public func setShareUsage(_ on: Bool) { send(.usageDataToggled(on: on)) }

    // MARK: - The cua command

    /// Reads what installing the bundled `cua` would do (writes nothing).
    public func planCli() async {
        guard let cli, state.cliPlan == nil else { return }
        send(.cliPlanned(plan: await cli.plan()))
    }

    /// Continue (`install`) or Skip from Command line.
    public func finishCli(install: Bool) async {
        guard !state.cliInstalling else { return }
        send(.cliDone(install: install))
        guard state.cliInstalling, let cli, let plan = state.cliPlan else { return }
        do {
            send(.cliPlanned(plan: try await cli.install(request: AppCliInstallRequest(modifyPath: !plan.onPath))))
        } catch {
            send(.cliInstallFailed(error: LiveSpacesBackend.words(error)))
        }
    }

    /// Another `cua` that comes first on PATH (an old Python cua-cli, say),
    /// so typing `cua` runs it rather than the installed path Done shows:
    /// Done notes it under "cua command".
    public var cliShadowedBy: String? {
        guard state.cliTarget != nil, let other = state.cliPlan?.shadowedBy, !other.isEmpty else { return nil }
        return other
    }

    // MARK: - Where it shows up

    /// A presentation card was picked: the flow shows it and the app saves
    /// the setting now.
    public func pickPresentation(menuBar: Bool) {
        send(.presentationPicked(menuBar: menuBar))
        onPresentation?(menuBar)
    }

    // MARK: - AI agents

    /// Installed agents, detected.
    public var installedAgents: [AppAgentSetupStatus] { (agentStatuses ?? []).filter(\.installed) }

    /// Detects the coding agents (reads only).
    public func loadAgents() async {
        guard let agentSetup else {
            agentStatuses = []
            return
        }
        let all = await agentSetup.statuses()
        agentStatuses = all
        agentSelection = Set(all.filter(\.installed).map(\.id))
    }

    public var canSetUpAgents: Bool {
        !agentSelection.isEmpty && (agentSkills || agentMcp || agentDriver) && !agentsBusy
    }

    /// "Set up": the chosen parts for the ticked agents, then cua-driver
    /// when the background computer-use card is ticked.
    public func setUpAgents() async {
        guard let agentSetup, canSetUpAgents else { return }
        agentsBusy = true
        agentsError = nil
        defer { agentsBusy = false }
        let ids = installedAgents.map(\.id).filter { agentSelection.contains($0) }
        do {
            var outcomes: [AppAgentSetupOutcomeInput] = []
            if agentSkills || agentMcp {
                outcomes += try await agentSetup.setUp(agents: ids, skills: agentSkills, mcp: agentMcp)
            }
            if agentDriver { outcomes += try await agentSetup.setUpCuaDriver(agents: ids) }
            agentOutcomes = outcomes
        } catch {
            agentsError = LiveSpacesBackend.words(error)
        }
    }

    /// One line per ticked agent after setup.
    public var agentSummaries: [AppAgentSetupSummary] {
        guard let outcomes = agentOutcomes else { return [] }
        return installedAgents.filter { agentSelection.contains($0.id) }
            .map { appAgentSetupSummary(outcomes: outcomes, agent: $0.id, name: $0.name) }
    }

    /// Continue after setup (the agents that set up cleanly) or Skip.
    public func finishAgents(skipped: Bool = false) {
        let configured = skipped ? [] : installedAgents.filter { agentSelection.contains($0.id) }
            .filter { a in
                appAgentSetupSummary(outcomes: agentOutcomes ?? [], agent: a.id, name: a.name).failed.isEmpty
            }
            .map(\.name)
        send(.agentsDone(configured: configured))
    }

    // MARK: - Cua Volume

    /// The daemon's Spaces tools (`volume_mount_status`, `volume_mount`,
    /// `volume_unmount`); nil: no daemon, the page says the drive is not
    /// available.
    public var driveTools: AgentsToolRunning?
    /// Opens a URL (System Settings' file system extensions).
    public var openURL: (URL) -> Void = { NSWorkspace.shared.open($0) }

    private func mountStatus(_ tool: String) async throws -> AppDriveMountInput {
        guard let driveTools else { throw AgentsToolError(message: "No cua daemon") }
        let r = try await driveTools.agentsTool(tool, [:])
        let data = try JSONSerialization.data(withJSONObject: r)
        return try appDriveMountFromJson(json: String(decoding: data, as: UTF8.self))
    }

    /// Reads what the mount can do here (the checkbox) and where the files
    /// live now (the storage choice).
    public func checkDrive() async {
        let status = try? await mountStatus("volume_mount_status")
        send(.driveChecked(os: .macos, status: status))
        await loadDriveStorage()
    }

    /// Reads `volume_storage` (the page polls it while the agent prompt
    /// shows, so a bucket the agent connects is noticed).
    public func loadDriveStorage() async {
        if let driveTools, let raw = try? await driveTools.agentsTool("volume_storage", [:]),
           let storage = try? appStorageActionFromJson(json: StorageModel.json(["type": "loaded", "storage": raw])),
           case let .loaded(input) = storage {
            let wasBusy = state.storage.busy
            send(.driveStorageLoaded(storage: input, home: userHome()))
            // A bucket the agent connected: the core asks to adopt it.
            if !wasBusy, case .adopt? = state.storage.request { await runStorageRequest() }
        }
    }

    /// The agent prompt shows: poll for the bucket it sets up.
    public var watchingStorage: Bool {
        view.drive?.storageRows.contains { $0.id == "s3-prompt" } ?? false
    }

    /// Shows a path in Finder.
    public var reveal: (String) -> Void = { path in
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: homeExpanded(path))])
    }

    /// A bucket form edit, Test connection or a choice: runs the test the
    /// core asks for.
    public func driveStorage(_ action: AppStorageAction) async {
        send(.driveStorage(action: action))
        await runStorageRequest()
    }

    /// Runs `volume_storage_set` when the page's storage state asks for it
    /// (a test, or Continue's save), and answers the core.
    private func runStorageRequest() async {
        guard let request = state.storage.request else { return }
        let update: AppDriveStorageUpdate
        switch request {
        case let .test(u), let .save(u), let .adopt(u): update = u
        default: return
        }
        var isTest = false
        if case .test = request { isTest = true }
        var isAdopt = false
        if case .adopt = request { isAdopt = true }
        do {
            guard let driveTools else { throw AgentsToolError(message: "No cua daemon") }
            let args = (try? JSONSerialization.jsonObject(with: Data(appStorageUpdateJson(update: update).utf8)))
                as? [String: Any] ?? [:]
            let raw = try await driveTools.agentsTool("volume_storage_set", args)
            let type = isTest ? "checked" : "saved"
            guard case let .checked(check)? = try? appStorageActionFromJson(
                json: StorageModel.json(["type": "checked", "check": raw])) else {
                throw AgentsToolError(message: "Unexpected answer from volume_storage_set")
            }
            if isAdopt {
                send(.driveStorage(action: .adopted(check: check)))
            } else if type == "checked" {
                send(.driveStorage(action: .checked(check: check)))
            } else {
                send(.driveStorageSaved(check: check))
                await runMountRequest()
            }
        } catch {
            if isAdopt {
                send(.driveStorage(action: .adopted(check: AppDriveCheckInput(
                    ok: false, reachable: false, authorized: false, versioning: false,
                    detail: LiveSpacesBackend.words(error), applied: false))))
            } else if isTest {
                send(.driveStorage(action: .failed(error: LiveSpacesBackend.words(error))))
            } else {
                send(.driveStorageSaved(check: AppDriveCheckInput(
                    ok: false, reachable: false, authorized: false, versioning: false,
                    detail: LiveSpacesBackend.words(error), applied: false)))
            }
        }
    }

    private func runMountRequest() async {
        guard let request = state.driveRequest else { return }
        do {
            let status = try await mountStatus(request == .mount ? "volume_mount" : "volume_unmount")
            send(.driveMounted(status: status))
        } catch {
            send(.driveFailed(error: LiveSpacesBackend.words(error)))
        }
    }

    /// Continue on the Cua Volume page: saves the storage choice, then
    /// mounts (or unmounts) when the checkbox and the daemon disagree.
    public func continueDrive() async {
        send(.driveContinue)
        await runStorageRequest()
        await runMountRequest()
    }

    // MARK: - This machine

    public var showingHostForm: Bool { host.form != nil }

    /// "Set up for access": host setup through the SDK (relay mode signs in
    /// with the app's account), then Done.
    public func setUpHost() async {
        await host.submit()
        if host.form == nil, host.state?.configured == true {
            send(.modeChosen(mode: .host))
        }
    }

    /// The panes host setup still needs granted (macOS privacy settings).
    public var hostPermissions: [AppPermissionRow] {
        appHostPermissionRows(permissions: host.state?.permissions ?? [])
    }

    // MARK: - Done

    // MARK: - Done

    /// The account now, read from the shared session (the CLI's sign-in
    /// included). Set by the app.
    public var readIdentity: (() -> String?)?
    /// Turns the Cua Volume experiment on (Settings, Experiments) for
    /// Done's "Turn on".
    public var onTurnOnVolume: (() -> Void)?

    /// Done shows: read the machine again rather than trusting what this
    /// run saw. A `cua` from install.sh, a sign-in from `cua auth login`
    /// while the window was open, and agents set up earlier all count.
    public func refreshDone() async {
        if let cli { quietly(.cliPlanned(plan: await cli.plan())) }
        if let who = readIdentity?(), !who.isEmpty, who != state.identity {
            quietly(.signedIn(identity: who))
        }
        if let agentSetup {
            let configured = await agentSetup.statuses().filter { $0.installed && $0.cuaConfigured }.map(\.name)
            if !configured.isEmpty { quietly(.agentsDetected(configured: configured)) }
        }
    }

    /// What the machine says, not something the user did on a page: no
    /// usage event.
    private func quietly(_ action: AppOnboardingAction) {
        state = appOnboardingReduce(state: state, action: action)
    }

    /// A fix on Done's to-do list: its page opens, and leaving it comes
    /// back to Done. Cua Volume turns its experiment on first.
    public func fixTodo(_ id: AppOnboardingTodoId) async {
        if id == .volume, !state.experiments.cuaVolume {
            onTurnOnVolume?()
            var x = currentExperiments?() ?? state.experiments
            x.cuaVolume = true
            send(.experimentsLoaded(experiments: x))
            if !state.driveChecked { await checkDrive() }
        }
        if id == .cli, state.cliPlan == nil { await planCli() }
        send(.todoChosen(id: id))
    }

    /// "Start using Cua Spaces".
    public func finish() {
        telemetry?.record(appTelemetryOnboardingFinished(state: state))
        completed = true
        onLaunchAtLogin?(state.launchAtLogin)
        guard let statePath else { return }
        let mode = state.mode == .host ? "host" : "client"
        let url = URL(fileURLWithPath: statePath)
        try? FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                                 withIntermediateDirectories: true)
        try? Data("{\"completed\":true,\"mode\":\"\(mode)\"}".utf8).write(to: url, options: .atomic)
    }

    /// The account became known after the first run started (the launch
    /// reads it once the keychain allows): a first run still on Welcome
    /// starts from it, as if it had been known from the start.
    public func adoptIdentity(_ identity: String?) {
        host.identity = identity
        guard !completed, state.identity != identity else { return }
        // Signed in (from the app or the CLI, one shared session): on any
        // page, so Done never says "not signed in" for a signed-in Mac. A
        // sign-out only clears it on Welcome, before the flow asked.
        if let identity, !identity.isEmpty {
            quietly(.signedIn(identity: identity))
        } else if state.step == .welcome {
            state.identity = identity
        }
    }

    /// Shows the flow again (Settings).
    public func restart() {
        completed = false
        shownOnce = false
        host.closeForm()
        agentOutcomes = nil
        let plan = state.cliPlan
        state = appOnboardingInitial(installerMode: state.installerMode, identity: state.identity)
        if let x = currentExperiments?() { send(.experimentsLoaded(experiments: x)) }
        if let plan { send(.cliPlanned(plan: plan)) }
        Task { await checkDrive() }
    }

    static func readCompleted(_ path: String) -> Bool? {
        guard let data = FileManager.default.contents(atPath: path),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        return obj["completed"] as? Bool
    }
}
