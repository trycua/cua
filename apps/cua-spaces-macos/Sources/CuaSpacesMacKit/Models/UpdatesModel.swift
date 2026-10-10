// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import AppKit
import CuaSDK
import CuaSpacesFFI
import Foundation
import Observation
import Sparkle

/// What Settings → About drives: Sparkle's `SPUUpdater` in the shipped app,
/// an in-memory updater in tests and captures.
@MainActor
public protocol UpdaterDriving: AnyObject {
    /// Checks on its own (Sparkle's `automaticallyChecksForUpdates`).
    var automaticallyChecks: Bool { get set }
    /// Downloads and installs on its own (`automaticallyDownloadsUpdates`).
    var automaticallyInstalls: Bool { get set }
    /// The last check (`lastUpdateCheckDate`).
    var lastCheck: Date? { get }
    /// Check Now can run (`canCheckForUpdates`; false while a check runs).
    var canCheck: Bool { get }
    /// Called after any of the above changes.
    var onChange: (() -> Void)? { get set }
    /// The Sparkle channels to install from besides the default one.
    var channels: [String] { get set }
    /// Checks now, with Sparkle's own progress and result windows.
    func checkNow()
    /// Told each updater step: `found`, `not_found`, `installed` or `failed`
    /// (usage telemetry; fixed words).
    var onUpdateEvent: ((String) -> Void)? { get set }
}

/// Sparkle, with its standard windows. Started only in an app bundle whose
/// Info.plist names a feed and the EdDSA key (`SUFeedURL`,
/// `SUPublicEDKey`).
@MainActor
public final class SparkleUpdater: NSObject, UpdaterDriving, SPUUpdaterDelegate, SPUStandardUserDriverDelegate,
    SUVersionDisplay {
    private var controller: SPUStandardUpdaterController!
    private var observations: [NSKeyValueObservation] = []
    public var onChange: (() -> Void)?
    public var channels: [String] = []
    public var onUpdateEvent: ((String) -> Void)?
    /// The bundle this process runs from, as it was at launch.
    private let launched = LaunchedBundle(bundle: .main)
    /// The update the last check found (its disk image, for a manual
    /// download when Sparkle cannot install it).
    private var found: SUAppcastItem?
    /// The relaunch prompt is shown once per run.
    private var askedToRelaunch = false

    private var updater: SPUUpdater { controller.updater }

    /// The updater for `bundle`, started; nil when the bundle has no feed
    /// (a development build) or Sparkle refused to start.
    public static func start(bundle: Bundle = .main, channels: [String]) -> SparkleUpdater? {
        guard bundle.bundleURL.pathExtension == "app",
              bundle.object(forInfoDictionaryKey: "SUFeedURL") is String,
              bundle.object(forInfoDictionaryKey: "SUPublicEDKey") is String else { return nil }
        let updater = SparkleUpdater(channels: channels)
        do {
            try updater.updater.start()
        } catch {
            NSLog("Cua Spaces: the updater did not start: %@", error.localizedDescription)
            return nil
        }
        return updater
    }

    private init(channels: [String]) {
        self.channels = channels
        super.init()
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: self,
                                                  userDriverDelegate: self)
        let changed: @Sendable () -> Void = { [weak self] in
            DispatchQueue.main.async { self?.onChange?() }
        }
        observations = [
            updater.observe(\.canCheckForUpdates) { _, _ in changed() },
            updater.observe(\.lastUpdateCheckDate) { _, _ in changed() },
            updater.observe(\.automaticallyChecksForUpdates) { _, _ in changed() },
            updater.observe(\.automaticallyDownloadsUpdates) { _, _ in changed() },
        ]
    }

    public var automaticallyChecks: Bool {
        get { updater.automaticallyChecksForUpdates }
        set { updater.automaticallyChecksForUpdates = newValue }
    }

    public var automaticallyInstalls: Bool {
        get { updater.automaticallyDownloadsUpdates }
        set { updater.automaticallyDownloadsUpdates = newValue }
    }

    public var lastCheck: Date? { updater.lastUpdateCheckDate }
    public var canCheck: Bool { updater.canCheckForUpdates }

    public func checkNow() {
        // A copy replaced under this process cannot install updates; say so
        // instead of letting Sparkle fail at the installer.
        if let replaced = launched.replacement() {
            askedToRelaunch = true
            Self.askToRelaunch(replaced, launched: launched)
            return
        }
        updater.checkForUpdates()
    }

    /// Checks in the background (no window unless an update is found);
    /// end-to-end runs use it through `CUA_SPACES_UPDATE_CHECK`.
    public func checkInBackground() { updater.checkForUpdatesInBackground() }

    // Sparkle calls its delegate on the main thread.

    /// No scheduled check while the bundle on disk is not the one running:
    /// its installer could not start (see `LaunchedBundle`). The user is
    /// asked once to relaunch instead.
    public nonisolated func updater(_ updater: SPUUpdater, mayPerform updateCheck: SPUUpdateCheck) throws {
        try MainActor.assumeIsolated {
            guard let replaced = launched.replacement() else { return }
            if !askedToRelaunch {
                askedToRelaunch = true
                let launched = launched
                DispatchQueue.main.async { Self.askToRelaunch(replaced, launched: launched) }
            }
            throw NSError(domain: Self.errorDomain, code: 1, userInfo: [
                NSLocalizedDescriptionKey: "Cua Spaces was replaced on disk while it was running; relaunch it to update.",
            ])
        }
    }

    public nonisolated func allowedChannels(for updater: SPUUpdater) -> Set<String> {
        MainActor.assumeIsolated { Set(channels) }
    }

    // MARK: - What a check found (usage telemetry: fixed words)

    private nonisolated func tell(_ event: String) {
        MainActor.assumeIsolated { onUpdateEvent?(event) }
    }

    public nonisolated func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        MainActor.assumeIsolated { found = item }
        tell("found")
    }
    public nonisolated func updaterDidNotFindUpdate(_ updater: SPUUpdater) { tell("not_found") }
    public nonisolated func updater(_ updater: SPUUpdater, willInstallUpdate item: SUAppcastItem) { tell("installed") }
    public nonisolated func updater(_ updater: SPUUpdater, didAbortWithError error: any Error) {
        // "No update" also arrives here; only a real failure is one.
        let code = (error as NSError).code
        if code == Int(SUError.noUpdateError.rawValue) { return }
        tell("failed")
        // A check refused because this copy was replaced has asked already.
        guard code != Int(SUError.installationCanceledError.rawValue),
              (error as NSError).domain != Self.errorDomain else { return }
        // After Sparkle's own error alert, offer the way that still works:
        // relaunch when this copy was replaced on disk (whatever step
        // failed), else download the update when its installer could not
        // start or run.
        MainActor.assumeIsolated {
            if let replaced = launched.replacement() {
                askedToRelaunch = true
                Self.askToRelaunch(replaced, launched: launched)
            } else if code == Int(SUError.installationError.rawValue), let item = found {
                Self.offerDownload(item)
            }
        }
    }

    // MARK: - When Sparkle cannot install

    private nonisolated static let errorDomain = "com.trycua.spaces.macos.updates"

    /// "Relaunch Cua Spaces to update": the copy on disk is not the one
    /// running. Relaunching runs it, and it updates normally.
    private static func askToRelaunch(_ replaced: LaunchedBundle.Replacement, launched: LaunchedBundle) {
        let alert = NSAlert()
        alert.messageText = "Relaunch Cua Spaces to update"
        if let version = replaced.version {
            let what = version.isEmpty ? "A new copy of Cua Spaces" : "Cua Spaces \(version)"
            alert.informativeText = "\(what) was installed while this copy was running. "
                + "Relaunch to finish; updates install from there."
            alert.addButton(withTitle: "Relaunch")
        } else {
            alert.informativeText = "Cua Spaces was moved or deleted while it was running. "
                + "Quit it and open it again from Applications to update."
        }
        alert.addButton(withTitle: replaced.version == nil ? "OK" : "Later")
        NSApp.activate()
        guard replaced.version != nil, alert.runModal() == .alertFirstButtonReturn else { return }
        relaunch(launched.url)
    }

    /// "Download the update": the disk image, to drag over this copy.
    private static func offerDownload(_ item: SUAppcastItem) {
        guard let url = item.fileURL else { return }
        let alert = NSAlert()
        alert.messageText = "Download Cua Spaces \(item.displayVersionString)"
        alert.informativeText = "Cua Spaces could not install the update itself. Download it, quit Cua Spaces, "
            + "and drag the new copy to Applications."
        alert.addButton(withTitle: "Download")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate()
        if alert.runModal() == .alertFirstButtonReturn { NSWorkspace.shared.open(url) }
    }

    /// Opens `bundle` again once this process has exited, then quits.
    private static func relaunch(_ bundle: URL) {
        let pid = ProcessInfo.processInfo.processIdentifier
        let waiter = Process()
        waiter.executableURL = URL(fileURLWithPath: "/bin/sh")
        waiter.arguments = ["-c", "while /bin/kill -0 \(pid) 2>/dev/null; do /bin/sleep 0.2; done; exec /usr/bin/open \"$0\"",
                            bundle.path]
        do {
            try waiter.run()
        } catch {
            NSLog("Cua Spaces: could not relaunch: %@", error.localizedDescription)
            return
        }
        NSApp.terminate(nil)
    }

    // MARK: - Versions in Sparkle's windows

    /// This app's full version (`CuaVersion`: 0.2.0-staging.5), which
    /// `CFBundleShortVersionString` (X.Y.Z only) leaves out.
    private nonisolated static var fullVersion: String? {
        Bundle.main.object(forInfoDictionaryKey: "CuaVersion") as? String
    }

    public nonisolated func standardUserDriverRequestsVersionDisplayer() -> (any SUVersionDisplay)? { self }

    // MARK: - Sparkle's alerts in front

    /// "You're up to date!" and the other alerts are modal: brought to the
    /// front, so one never waits unseen behind the New UI's window (or
    /// another app's) while the About pane says "Checking…".
    public nonisolated func standardUserDriverWillShowModalAlert() {
        MainActor.assumeIsolated { Self.bringAlertsForward() }
    }

    /// Activates the app: the modal alert that follows becomes the key
    /// window, in front of every other.
    static func bringAlertsForward() {
        NSApp.activate()
    }

    /// "0.2.0-staging.6 is now available; you have 0.2.0-staging.5" (the
    /// appcast item already carries the full version).
    public nonisolated func formatUpdateVersion(
        fromUpdate update: SUAppcastItem,
        andBundleDisplayVersion inOutBundleDisplayVersion: AutoreleasingUnsafeMutablePointer<NSString>,
        withBundleVersion bundleVersion: String
    ) -> String {
        if let full = Self.fullVersion { inOutBundleDisplayVersion.pointee = full as NSString }
        return update.displayVersionString
    }

    public nonisolated func formatBundleDisplayVersion(_ bundleDisplayVersion: String, withBundleVersion bundleVersion: String,
                                                       matchingUpdate: SUAppcastItem?) -> String {
        Self.fullVersion ?? bundleDisplayVersion
    }
}

/// The app bundle this process was launched from, and whether the copy on
/// disk is still that one.
///
/// Installing a new copy over a running Cua Spaces (dragging it from the
/// disk image, or any installer that replaces the bundle) leaves this
/// process running code whose files are gone. macOS can no longer check its
/// signature: authd refuses it (`failed to create code ref -67049`), so
/// Sparkle's installer cannot be authorized (`Failed to create authorization
/// reference: -60008`) and every update fails with "An error occurred while
/// launching the installer". Relaunching runs the new copy, which updates
/// normally.
public struct LaunchedBundle: Sendable {
    /// What is on disk now, when it is not what was launched.
    public struct Replacement: Equatable, Sendable {
        /// Its version (`CuaVersion`, else `CFBundleShortVersionString`);
        /// nil when the bundle is gone.
        public var version: String?
    }

    public let url: URL
    let executable: URL?
    /// The executable's file (device, inode) at launch.
    let file: [UInt64]?

    public init(bundle: Bundle) {
        self.init(url: bundle.bundleURL, executable: bundle.executableURL)
    }

    init(url: URL, executable: URL?) {
        self.url = url
        self.executable = executable
        file = executable.flatMap(Self.fileID)
    }

    static func fileID(_ url: URL) -> [UInt64]? {
        var st = stat()
        guard stat(url.path, &st) == 0 else { return nil }
        return [UInt64(UInt32(bitPattern: st.st_dev)), UInt64(st.st_ino)]
    }

    /// The copy on disk when it is no longer the one launched (its main
    /// executable is another file, or gone); nil while it is the same.
    public func replacement() -> Replacement? {
        guard let executable, let file else { return nil }
        let now = Self.fileID(executable)
        if now == file { return nil }
        guard now != nil else { return Replacement(version: nil) }
        let info = NSDictionary(contentsOf: url.appendingPathComponent("Contents/Info.plist")) as? [String: Any] ?? [:]
        let full = (info["CuaVersion"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        return Replacement(version: full ?? info["CFBundleShortVersionString"] as? String ?? "")
    }
}

/// An in-memory updater (tests, captures): nothing is checked or stored.
@MainActor
public final class FixtureUpdater: UpdaterDriving {
    public var automaticallyChecks = true { didSet { onChange?() } }
    public var automaticallyInstalls = false { didSet { onChange?() } }
    public var lastCheck: Date?
    public var canCheck = true
    public var onChange: (() -> Void)?
    public var channels: [String] = []
    public var onUpdateEvent: ((String) -> Void)?
    public private(set) var checks = 0
    /// What the next check finds (`not_found` by default).
    public var nextResult = "not_found"

    public init(lastCheck: Date? = nil) { self.lastCheck = lastCheck }

    public func checkNow() {
        checks += 1
        lastCheck = Date()
        onUpdateEvent?(nextResult)
        onChange?()
    }
}

/// Settings → About: the core's `appAboutView` over the updater's state.
/// The channel is an app setting (`AppSettings.updateChannel`); the rest is
/// Sparkle's own.
@MainActor
@Observable
public final class UpdatesModel {
    public let updater: UpdaterDriving?
    private let info: AboutBundleInfo
    /// Bumped when the updater changes, so the view recomputes.
    private var revision = 0
    /// The chosen channel and where it is saved.
    public var channel: AppUpdateChannel {
        didSet {
            updater?.channels = appAboutAllowedChannels(channel: channel)
            saveChannel(channel)
        }
    }
    @ObservationIgnored public var saveChannel: (AppUpdateChannel) -> Void = { _ in }
    /// Where usage events go (the app's telemetry; nil records nothing).
    @ObservationIgnored public var telemetry: TelemetryRunning?
    /// A check the user started runs until the updater answers.
    private var userCheck = false

    public init(updater: UpdaterDriving?, channel: AppUpdateChannel = .stable,
                info: AboutBundleInfo = AboutBundleInfo(bundle: .main)) {
        self.updater = updater
        self.info = info
        self.channel = channel
        updater?.channels = appAboutAllowedChannels(channel: channel)
        updater?.onChange = { [weak self] in self?.revision += 1 }
        // Do people update: what each check found, and installs (the Tauri
        // app records the same words).
        updater?.onUpdateEvent = { [weak self] event in self?.record(event) }
    }

    private func record(_ action: String) {
        let trigger = userCheck ? "user" : "background"
        if action != "found" { userCheck = false }
        var signals: [AppTelemetrySignal] = []
        if action == "found" || action == "not_found" {
            signals.append(.appUpdate(action: "checked", channel: channel == .beta ? "beta" : "stable", trigger: trigger))
        }
        signals.append(.appUpdate(action: action, channel: channel == .beta ? "beta" : "stable", trigger: trigger))
        telemetry?.record(signals)
    }

    /// The pane, from the core.
    public var view: AppAboutView { appAboutView(input: input) }

    /// What the pane is built from: the app, and the updater's state.
    public var input: AppAboutInput {
        _ = revision
        return AppAboutInput(
            platform: "macos", version: info.version, build: info.build, os: info.os,
            updater: updater != nil,
            autoCheck: updater?.automaticallyChecks ?? false,
            autoInstall: updater?.automaticallyInstalls ?? false,
            channel: channel,
            lastCheck: updater?.lastCheck.map(Self.dateText),
            checking: !(updater?.canCheck ?? true))
    }

    static func dateText(_ date: Date) -> String {
        date.formatted(date: .abbreviated, time: .shortened)
    }

    public func setAutoCheck(_ on: Bool) {
        updater?.automaticallyChecks = on
        revision += 1
    }

    public func setAutoInstall(_ on: Bool) {
        updater?.automaticallyInstalls = on
        revision += 1
    }

    public func choose(channel id: String) {
        channel = id == "beta" ? .beta : .stable
    }

    public func checkNow() {
        userCheck = true
        updater?.checkNow()
    }
}

/// The version and machine lines the About pane shows.
public struct AboutBundleInfo: Sendable {
    /// The full version (`CuaVersion`, else `CFBundleShortVersionString`).
    public var version: String
    /// `CFBundleVersion`.
    public var build: String
    /// "macOS 26.0 (arm64)".
    public var os: String

    public init(version: String, build: String, os: String) {
        self.version = version
        self.build = build
        self.os = os
    }

    public init(bundle: Bundle) {
        let info = bundle.infoDictionary ?? [:]
        let short = info["CFBundleShortVersionString"] as? String ?? "0.0.0"
        version = (info["CuaVersion"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? short
        build = info["CFBundleVersion"] as? String ?? ""
        let v = ProcessInfo.processInfo.operatingSystemVersion
        let patch = v.patchVersion > 0 ? ".\(v.patchVersion)" : ""
        #if arch(arm64)
        let arch = "arm64"
        #else
        let arch = "x86_64"
        #endif
        os = "macOS \(v.majorVersion).\(v.minorVersion)\(patch) (\(arch))"
    }
}
