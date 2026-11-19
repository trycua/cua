// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import AppKit
import Foundation
import Sparkle
import Testing

/// Settings → About over an in-memory updater, and the first launch after
/// an update: the version recorded in the settings file, whose daemon
/// restarts and what `cua agents update` failing shows. No process runs:
/// the bundled `cua` is a scripted fake.
@Suite("Updates")
@MainActor
struct UpdatesTests {
    static let info = AboutBundleInfo(version: "0.2.0-staging.5", build: "0.2.0.105", os: "macOS 26.0 (arm64)")

    /// Do people update: each check, what it found and installs, with the
    /// channel and who started it (the Tauri app records the same words).
    @Test func updaterStepsAreRecordedAsFixedWords() {
        let updater = FixtureUpdater()
        let telemetry = FixtureTelemetry()
        let model = UpdatesModel(updater: updater, channel: .stable, info: Self.info)
        model.telemetry = telemetry
        model.checkNow()
        updater.nextResult = "found"
        // A switch checks the new channel at once.
        model.choose(channel: "nightly")
        // Sparkle installs what it found on its own schedule.
        updater.onUpdateEvent?("installed")
        let got = telemetry.recorded.map { ParityTests.signalFrame($0) }
            .map { "\($0["action"]!) \($0["channel"]!) \($0["trigger"]!)" }
        #expect(got == [
            "checked stable user", "not_found stable user",
            "checked nightly user", "found nightly user", "installed nightly user",
        ])
        // A scheduled check is the background's.
        updater.nextResult = "not_found"
        updater.onUpdateEvent?("not_found")
        #expect(telemetry.recorded.suffix(2).map { ParityTests.signalFrame($0)["trigger"] as! String }
            == ["background", "background"])
    }

    @Test func theAboutPaneFollowsTheUpdater() {
        let updater = FixtureUpdater()
        let model = UpdatesModel(updater: updater, channel: .stable, info: Self.info)
        var v = model.view
        #expect(v.title == "Cua Spaces for macOS")
        #expect(v.versionLine == "Version 0.2.0-staging.5 (0.2.0.105)")
        #expect(v.links.map(\.label) == ["Acknowledgements", "Privacy Policy", "Terms of Service", "System Status", "Report an Issue\u{2026}"])
        #expect(v.updates?.lastCheck == "Last check: Never")
        #expect(v.updates?.autoInstallEnabled == true)

        model.setAutoCheck(false)
        #expect(!updater.automaticallyChecks)
        #expect(model.view.updates?.autoInstallEnabled == false)
        model.setAutoInstall(true)
        #expect(updater.automaticallyInstalls)

        model.checkNow()
        #expect(updater.checks == 1)
        v = model.view
        #expect(v.updates?.lastCheck.hasPrefix("Last check: ") == true)
        #expect(v.updates?.lastCheck != "Last check: Never")
    }

    /// Updates check and download in the background out of the box, like
    /// Windows and Linux; only a click on "Restart to update" restarts.
    @Test func updatesDownloadInTheBackgroundByDefault() throws {
        let plist = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Support/Info.plist")
        let info = try #require(try PropertyListSerialization.propertyList(from: Data(contentsOf: plist), format: nil) as? [String: Any])
        #expect(info["SUEnableAutomaticChecks"] as? Bool == true)
        #expect(info["SUAutomaticallyUpdate"] as? Bool == true)
    }

    @Test func noUpdaterNoControls() {
        let model = UpdatesModel(updater: nil, info: Self.info)
        #expect(model.view.updates == nil)
        #expect(model.view.links.count == 5)
    }

    @Test func theChannelIsSavedAndGivesSparkleItsChannels() {
        let updater = FixtureUpdater()
        let model = UpdatesModel(updater: updater, channel: .stable, info: Self.info)
        var saved: [AppUpdateChannel] = []
        model.saveChannel = { saved.append($0) }
        #expect(updater.channels.isEmpty)
        #expect(updater.feedURL == nil)
        #expect(model.view.updates?.channels.map(\.id) == ["stable", "nightly"])
        model.choose(channel: "nightly")
        #expect(updater.channels == ["nightly"])
        #expect(updater.feedURL == "https://github.com/trycua/cua/releases/download/cua-spaces-nightly/cua-spaces-nightly-appcast.xml")
        #expect(saved == [.nightly])
        #expect(model.view.updates?.channels.first(where: \.active)?.id == "nightly")
        // A switch checks the new channel at once.
        #expect(updater.checks == 1)
        model.choose(channel: "stable")
        #expect(updater.channels.isEmpty)
        #expect(updater.feedURL == nil)
        #expect(saved == [.nightly, .stable])
        // A page from before Nightly asks for Beta: that is Nightly.
        model.choose(channel: "beta")
        #expect(model.channel == .nightly)
    }

    /// A nightly's About says when it was built, and its notes are the nightly release.
    @Test func aNightlyShowsItsBuildDayAndNotes() {
        let info = AboutBundleInfo(version: "0.21.0-nightly.20261105.42+55e1f5d", build: "0.20.0.202611050731", os: "macOS 26.0 (arm64)")
        let model = UpdatesModel(updater: FixtureUpdater(), channel: .nightly, info: info)
        #expect(model.view.versionLine == "Version 0.21.0-nightly.20261105.42+55e1f5d, built 5 Nov 2026")
        #expect(UpdateState.notesURL(version: "0.21.0-nightly.20261105.42+55e1f5d")
            == "https://github.com/trycua/cua/releases/tag/cua-spaces-nightly")
        #expect(UpdateState.notesURL(version: "0.20.0") == "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.20.0")
    }

    // MARK: - The in-app update flow

    @Test func aFoundUpdateDownloadsThenWaitsForRestart() {
        let updater = FixtureUpdater()
        let telemetry = FixtureTelemetry()
        let model = UpdatesModel(updater: updater, channel: .stable, info: Self.info)
        model.telemetry = telemetry
        #expect(model.updateState.status == .idle)
        #expect(model.updateState.currentVersion == "0.2.0-staging.5")
        updater.simulateFound(version: "0.2.0-staging.6")
        #expect(model.updateState.status == .available)
        #expect(model.updateState.notesUrl == "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.2.0-staging.6")
        updater.simulateProgress(percent: 40)
        #expect(model.updateState.status == .downloading && model.updateState.percent == 40)
        #expect(!(model.input.checking))
        updater.simulateDownloaded()
        #expect(model.updateState.status == .downloaded && model.updateState.percent == nil)
        // Nothing restarts until the user asks.
        #expect(updater.installs == 0)
        model.installUpdate()
        #expect(updater.installs == 1)
        #expect(model.updateState.status == .idle)
        let words = telemetry.recorded.map { ParityTests.signalFrame($0)["action"] as! String }
        #expect(words.contains("downloaded") && words.contains("installed"))
    }

    @Test func installingWithNothingDownloadedChangesNothing() {
        let updater = FixtureUpdater()
        let model = UpdatesModel(updater: updater, channel: .stable, info: Self.info)
        model.installUpdate()
        updater.simulateFound(version: "0.2.0-staging.6")
        model.installUpdate()
        #expect(updater.installs == 0)
        #expect(model.updateState.status == .available)
    }

    @Test func theStateJSONHasTheContractsKeys() {
        let idle = UpdateState.idle(currentVersion: "0.9.2").json
        #expect(Set(idle.keys) == ["status", "currentVersion", "version", "percent", "notesUrl", "message"])
        #expect(idle["status"] as? String == "idle")
        #expect(idle["version"] is NSNull && idle["percent"] is NSNull && idle["notesUrl"] is NSNull && idle["message"] is NSNull)
        let busy = UpdateState(status: .downloading, currentVersion: "0.9.2", version: "0.9.3", percent: 12,
                               notesUrl: UpdateState.notesURL(version: "0.9.3")).json
        #expect(busy["percent"] as? Int == 12)
        #expect(busy["notesUrl"] as? String == "https://github.com/trycua/cua/releases/tag/cua-spaces-v0.9.3")
    }

    @Test func aFlowReducesSparklesStepsToPercent() {
        var flow = UpdateFlow(currentVersion: "0.9.2")
        flow.userCheckStarted()
        #expect(flow.state.status == .checking)
        flow.found(version: "0.9.3")
        flow.downloadStarted()
        flow.expectedLength(1000)
        flow.receivedData(500)
        #expect(flow.state.percent == 45)
        flow.receivedData(500)
        #expect(flow.state.percent == 90)
        flow.extractionStarted()
        flow.extractionProgress(0.5)
        #expect(flow.state.percent == 95)
        flow.downloaded()
        #expect(flow.state.status == .downloaded && flow.state.percent == nil && flow.state.version == "0.9.3")
        flow.dismissed()
        #expect(flow.state.status == .idle && flow.state.version == nil)
        // Unknown length stays at 0, never past 100.
        flow.found(version: "0.9.4")
        flow.receivedData(10)
        #expect(flow.state.percent == 0)
    }

    final class Replies {
        var choices: [SPUUserUpdateChoice] = []
        var acks = 0
    }

    @Test func theDriverHoldsTheInstallReplyUntilTheUserRestarts() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        let replies = Replies()
        var events: [String] = []
        var changes = 0
        driver.onEvent = { events.append($0) }
        driver.onChange = { changes += 1 }
        // A background check finds an update: download at once.
        driver.handleFound(version: "0.9.3", informationOnly: false) { replies.choices.append($0) }
        #expect(replies.choices == [.install])
        #expect(driver.state.status == .available)
        driver.showDownloadInitiated {}
        driver.showDownloadDidReceiveExpectedContentLength(100)
        driver.showDownloadDidReceiveData(ofLength: 50)
        #expect(driver.state.status == .downloading && driver.state.percent == 45)
        driver.showDownloadDidStartExtractingUpdate()
        driver.showExtractionReceivedProgress(1)
        #expect(driver.state.percent == 100)
        // Ready to install: held, not replied.
        driver.showReady(toInstallAndRelaunch: { replies.choices.append($0) })
        #expect(driver.state.status == .downloaded)
        #expect(events == ["downloaded"])
        #expect(replies.choices == [.install])
        #expect(changes > 0)
        #expect(driver.installNow())
        #expect(replies.choices == [.install, .install])
        // Once is enough.
        #expect(!driver.installNow() || replies.choices.count == 2)
        driver.dismissUpdateInstallation()
        #expect(driver.state.status == .idle)
    }

    @Test func theSilentPathReachesDownloaded() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var events: [String] = []
        driver.onEvent = { events.append($0) }
        driver.silentlyDownloaded(version: "0.9.3")
        #expect(driver.state.status == .downloaded && driver.state.version == "0.9.3")
        #expect(events == ["downloaded"])
    }

    /// Sparkle ignores the install block unless the delegate takes the
    /// install (returns true), and taking it loses the install on quit
    /// (CUA-3065). So the delegate leaves it to Sparkle.
    @Test func quitInstallStaysWithSparkle() {
        #expect(SparkleUpdater.handlesQuitInstallItself == false)
    }

    /// 0.21.1: "Restart to update" did nothing on the silent path, since the
    /// install block was dead. It must resume the update and install it.
    @Test func restartToUpdateInstallsASilentlyDownloadedUpdate() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var resumes = 0
        var choices: [SPUUserUpdateChoice] = []
        driver.resumeSilentUpdate = { resumes += 1 }
        // Nothing downloaded: nothing to do.
        #expect(!driver.installNow() && resumes == 0)
        driver.silentlyDownloaded(version: "0.9.3")
        #expect(driver.installNow())
        #expect(resumes == 1)
        // The resumed session asks to install: answered at once.
        driver.showReady(toInstallAndRelaunch: { choices.append($0) })
        #expect(choices == [.install])
        // A later session's prompt is held again, not auto-answered.
        driver.dismissUpdateInstallation()
        driver.showReady(toInstallAndRelaunch: { choices.append($0) })
        #expect(choices == [.install])
    }

    /// 0.20.0 to 0.22.0: the click did nothing. When the app is still running
    /// a few seconds after "Restart to update", it quits, so Sparkle's own
    /// installer applies the staged update; if that does not end the app, the
    /// page says to quit and reopen.
    @Test func aRestartThatDoesNothingQuitsTheApp() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var timers: [() -> Void] = []
        var quits = 0
        driver.schedule = { _, work in timers.append(work) }
        driver.quitApp = { quits += 1 }
        driver.resumeSilentUpdate = {}
        driver.silentlyDownloaded(version: "0.9.3")
        #expect(driver.installNow())
        #expect(quits == 0 && timers.count == 1)
        timers.removeFirst()()
        #expect(quits == 1 && driver.state.status != .error)
        // Still running after the quit: say what to do.
        timers.removeFirst()()
        #expect(driver.state.status == .error)
        #expect(driver.state.message == UpdateUserDriver.restartStuckMessage)
    }

    @Test func aNewerClickSupersedesAnOlderRestartTimer() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var timers: [() -> Void] = []
        var quits = 0
        driver.schedule = { _, work in timers.append(work) }
        driver.quitApp = { quits += 1 }
        driver.resumeSilentUpdate = {}
        driver.silentlyDownloaded(version: "0.9.3")
        #expect(driver.installNow())
        #expect(driver.installNow())
        timers.removeFirst()()
        #expect(quits == 0)
        timers.removeFirst()()
        #expect(quits == 1)
    }

    @Test func aHeldReplyRestartIsWatchedToo() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var timers: [() -> Void] = []
        var quits = 0
        driver.schedule = { _, work in timers.append(work) }
        driver.quitApp = { quits += 1 }
        var choices: [SPUUserUpdateChoice] = []
        driver.handleFound(version: "0.9.3", informationOnly: false) { _ in }
        driver.showReady(toInstallAndRelaunch: { choices.append($0) })
        #expect(driver.installNow() && choices == [.install])
        timers.removeFirst()()
        #expect(quits == 1)
    }

    @Test func aFailedResumeDoesNotInstallALaterPrompt() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var choices: [SPUUserUpdateChoice] = []
        driver.resumeSilentUpdate = {}
        driver.silentlyDownloaded(version: "0.9.3")
        #expect(driver.installNow())
        driver.handleError("offline")
        driver.showReady(toInstallAndRelaunch: { choices.append($0) })
        #expect(choices.isEmpty)
    }

    @Test func nothingFoundAndBackgroundErrorsStayQuiet() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        var acks = 0
        driver.showUpdateNotFoundWithError(NSError(domain: "t", code: 1)) { acks += 1 }
        #expect(acks == 1 && driver.state.status == .idle)
        driver.showUpdaterError(NSError(domain: "t", code: 2, userInfo: [NSLocalizedDescriptionKey: "offline"])) { acks += 1 }
        #expect(acks == 2)
        #expect(driver.state.status == .error && driver.state.message == "offline")
        driver.dismissUpdateInstallation()
        #expect(driver.state.status == .error)
    }

    /// A failed check is a plain line in the menus and Settings, never a dialog.
    @Test func aUserCheckErrorStaysInTheState() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        driver.beginUserCheck()
        #expect(driver.state.status == .checking)
        driver.showUpdaterError(NSError(domain: "t", code: 2, userInfo: [NSLocalizedDescriptionKey: "offline"])) {}
        #expect(driver.state.status == .error && driver.state.message == "offline")
    }

    @Test func aUserCheckThatFindsNothingEndsQuietly() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        driver.beginUserCheck()
        driver.showUpdateNotFoundWithError(NSError(domain: "t", code: 1)) {}
        #expect(driver.state.status == .idle)
    }

    @Test func anInformationOnlyItemIsNotInstalled() {
        let driver = UpdateUserDriver(currentVersion: "0.9.2")
        let replies = Replies()
        driver.handleFound(version: "1.0.0", informationOnly: true) { replies.choices.append($0) }
        #expect(replies.choices == [.dismiss])
        #expect(driver.state.status == .idle)
    }

    // MARK: - The launch after an update

    func settingsFile() -> String {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-updates-\(UUID().uuidString)", isDirectory: true)
        return dir.appendingPathComponent("settings.json").path
    }

    @Test func aVersionChangeRefreshesOnce() {
        let path = settingsFile()
        defer { try? FileManager.default.removeItem(atPath: (path as NSString).deletingLastPathComponent) }
        // A fresh install records its version and refreshes nothing.
        #expect(!UpdateRefresh.record(settingsPath: path, version: "0.2.0-staging.5", build: "0.2.0.105", onboarded: false))
        #expect(appSettingsLoad(path: path).lastSeenVersion == "0.2.0-staging.5 (0.2.0.105)")
        // Relaunching the same build: nothing.
        #expect(!UpdateRefresh.record(settingsPath: path, version: "0.2.0-staging.5", build: "0.2.0.105", onboarded: true))
        // The update to staging.6: refresh, once.
        #expect(UpdateRefresh.record(settingsPath: path, version: "0.2.0-staging.6", build: "0.2.0.106", onboarded: true))
        #expect(appSettingsLoad(path: path).lastSeenVersion == "0.2.0-staging.6 (0.2.0.106)")
        #expect(!UpdateRefresh.record(settingsPath: path, version: "0.2.0-staging.6", build: "0.2.0.106", onboarded: true))
    }

    @Test func recordingKeepsTheOtherSettings() {
        let path = settingsFile()
        defer { try? FileManager.default.removeItem(atPath: (path as NSString).deletingLastPathComponent) }
        var s = appSettingsLoad(path: path)
        s.menuBar = true
        s.updateChannel = .nightly
        try? appSettingsSave(path: path, settings: s)
        // A build from before the updater kept no record: after the first
        // run, that is an update.
        #expect(UpdateRefresh.record(settingsPath: path, version: "0.2.0", build: "0.2.0.2", onboarded: true))
        let after = appSettingsLoad(path: path)
        #expect(after.menuBar && after.updateChannel == .nightly)
    }

    /// Answers `cua` the way the real CLI does, recording each call.
    final class FakeCua: CommandRunning, @unchecked Sendable {
        var pid: Int32?
        var stopWorks = true
        var startStatus: Int32 = 0
        var agents = CommandResult(status: 0, output: #"{"outcomes":[]}"#)
        private(set) var calls: [String] = []
        private let lock = NSLock()

        func run(_ executable: String, _ arguments: [String], timeout: TimeInterval) -> CommandResult {
            lock.lock()
            defer { lock.unlock() }
            let call = arguments.joined(separator: " ")
            calls.append(call)
            switch call {
            case "--json daemon status":
                // Pretty-printed, as the CLI prints it.
                guard let pid else {
                    return CommandResult(status: 1, output: "{\n  \"mode\": \"embedded\",\n  \"pid\": null\n}\n")
                }
                return CommandResult(status: 0, output: "{\n  \"mode\": \"daemon\",\n  \"pid\": \(pid)\n}\n")
            case "daemon stop":
                if stopWorks { pid = nil }
                return CommandResult(status: 0, output: "stopped")
            case "daemon start":
                if startStatus == 0 { pid = 4242 }
                return CommandResult(status: startStatus, output: "")
            case "--json agents update":
                return agents
            default:
                return CommandResult(status: 2, output: "")
            }
        }
    }

    static let bundle = "/Applications/Cua Spaces.app"

    func refresh(_ cua: FakeCua) -> UpdateRefresh {
        UpdateRefresh(cua: Self.bundle + "/Contents/MacOS/cua", bundle: Self.bundle, runner: cua)
    }

    @Test func refreshedQuietly() {
        let cua = FakeCua()
        #expect(refresh(cua).run() == nil)
        #expect(cua.calls.contains("--json agents update"))
    }

    @Test func aFailedAgentsUpdateShowsOneNotice() {
        let cua = FakeCua()
        cua.agents = CommandResult(status: 1, output: """
            {
              "outcomes": [
                {"item": "cua", "change": "updated", "detail": ""},
                {"item": "cua-spaces", "change": "failed", "detail": "permission denied"}
              ]
            }
            """)
        let notice = refresh(cua).run()
        #expect(notice == "Cua Spaces updated, but could not refresh the Cua skills in your AI agents (cua-spaces: permission denied). Run `cua agents update` to try again.")
    }

    @Test func theCLIsPrettyJSONIsRead() {
        #expect(UpdateRefresh.json("{\n  \"pid\": 7\n}\n")?["pid"] as? Int == 7)
        #expect(UpdateRefresh.json("warning: something\n{\n  \"pid\": 8\n}")?["pid"] as? Int == 8)
        #expect(UpdateRefresh.json("no json") == nil)
    }

    @Test func theDaemonsExecutableIsReadFromItsPid() {
        let me = UpdateRefresh.executablePath(ProcessInfo.processInfo.processIdentifier)
        #expect(me != nil && FileManager.default.isExecutableFile(atPath: me!))
        #expect(UpdateRefresh.executablePath(-1) == nil)
        // The path it was started from (what an updated daemon still has
        // once its file is gone) names the same executable.
        #expect(UpdateRefresh.launchPath(ProcessInfo.processInfo.processIdentifier).map {
            URL(fileURLWithPath: $0).resolvingSymlinksInPath().path
        } == URL(fileURLWithPath: me!).resolvingSymlinksInPath().path)
    }
}

/// A copy installed over the running app: Sparkle cannot authorize its
/// installer for a process whose files are gone, so the app asks to relaunch.
@Suite("Launched bundle")
struct LaunchedBundleTests {
    static func bundle(version: String) throws -> (URL, URL) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("launched-\(UUID().uuidString)")
        let app = root.appendingPathComponent("Cua Spaces.app")
        try write(app, version: version)
        return (app, app.appendingPathComponent("Contents/MacOS/CuaSpacesMac"))
    }

    static func write(_ app: URL, version: String) throws {
        let macos = app.appendingPathComponent("Contents/MacOS")
        try FileManager.default.createDirectory(at: macos, withIntermediateDirectories: true)
        try Data("binary \(version)".utf8).write(to: macos.appendingPathComponent("CuaSpacesMac"))
        let info: NSDictionary = ["CFBundleShortVersionString": version, "CuaVersion": version]
        try info.write(to: app.appendingPathComponent("Contents/Info.plist"))
    }

    @Test func theLaunchedCopyIsNotAReplacement() throws {
        let (app, exe) = try Self.bundle(version: "0.5.0")
        defer { try? FileManager.default.removeItem(at: app.deletingLastPathComponent()) }
        #expect(LaunchedBundle(url: app, executable: exe).replacement() == nil)
    }

    @Test func aCopyInstalledOverTheRunningOneIsAReplacement() throws {
        let (app, exe) = try Self.bundle(version: "0.5.0")
        defer { try? FileManager.default.removeItem(at: app.deletingLastPathComponent()) }
        let launched = LaunchedBundle(url: app, executable: exe)
        // What dragging the new copy from the disk image does: the old
        // bundle goes, the new one's files arrive.
        try FileManager.default.removeItem(at: app)
        try Self.write(app, version: "0.6.0")
        #expect(launched.replacement() == LaunchedBundle.Replacement(version: "0.6.0"))
    }

    @Test func aDeletedCopyHasNoVersion() throws {
        let (app, exe) = try Self.bundle(version: "0.5.0")
        defer { try? FileManager.default.removeItem(at: app.deletingLastPathComponent()) }
        let launched = LaunchedBundle(url: app, executable: exe)
        try FileManager.default.removeItem(at: app)
        #expect(launched.replacement() == LaunchedBundle.Replacement(version: nil))
    }
}

/// "Check for Updates…" in the app menu and the menu bar menu, and the
/// up-arrow badge: all from the one `UpdatesModel`.
@Suite("Update menu")
@MainActor
struct UpdateMenuTests {
    static let info = AboutBundleInfo(version: "0.2.0-staging.5", build: "0.2.0.105", os: "macOS 26.0 (arm64)")

    func made() -> (UpdatesModel, FixtureUpdater) {
        let updater = FixtureUpdater()
        updater.state = .idle(currentVersion: Self.info.version)
        return (UpdatesModel(updater: updater, channel: .stable, info: Self.info), updater)
    }

    @Test func upToDateOffersACheckAndNoBadge() throws {
        let (model, _) = made()
        let item = try #require(model.menuItem)
        #expect(item.title == "Check for Updates\u{2026}" && item.detail == nil)
        #expect(item.action == .check && item.enabled && !item.badge)
        #expect(!item.title.contains("\u{00B7}"))
    }

    @Test func aFoundUpdateShowsTheBadgeAndTheVersionLine() throws {
        let (model, updater) = made()
        updater.simulateFound(version: "0.17.0-beta.11")
        var item = try #require(model.menuItem)
        #expect(item.title == "Version 0.17.0-beta.11 available" && item.detail == "Click to install")
        #expect(item.badge && item.action == .install)
        updater.simulateProgress(percent: 40)
        item = try #require(model.menuItem)
        #expect(item.detail == "Downloading 40%" && item.badge && !item.enabled)
        updater.simulateDownloaded()
        item = try #require(model.menuItem)
        #expect(item.title == "Version 0.17.0-beta.11 available" && item.detail == "Restart to update")
        #expect(item.badge && item.action == .install)
    }

    @Test func installingClearsTheBadge() throws {
        let (model, updater) = made()
        updater.simulateFound(version: "1.0.0")
        updater.simulateDownloaded()
        model.perform(try #require(model.menuItem).action)
        #expect(updater.installs == 1)
        let item = try #require(model.menuItem)
        #expect(!item.badge && item.title == "Check for Updates\u{2026}")
    }

    @Test func aClickBeforeTheDownloadEndsInstallsWhenItEnds() throws {
        let (model, updater) = made()
        updater.simulateFound(version: "1.0.0")
        model.perform(.install)
        #expect(updater.installs == 0)
        updater.simulateDownloaded()
        #expect(updater.installs == 1)
    }

    @Test func aFailedCheckIsAPlainLineWithNoBadge() throws {
        let (model, updater) = made()
        updater.simulateError("offline")
        let item = try #require(model.menuItem)
        #expect(item.title == "Check for Updates\u{2026}" && item.detail == "Could not check for updates")
        #expect(!item.badge && item.action == .check)
    }

    @Test func aCheckInProgressIsNotClickable() throws {
        let (model, updater) = made()
        updater.state = UpdateState(status: .checking, currentVersion: Self.info.version)
        let item = try #require(model.menuItem)
        #expect(item.title == "Checking for Updates\u{2026}" && !item.enabled && !item.badge)
    }

    @Test func theMenuCheckIsTheSettingsCheck() throws {
        let (model, updater) = made()
        model.perform(try #require(model.menuItem).action)
        #expect(updater.checks == 1)
        model.checkNow()
        #expect(updater.checks == 2)
    }

    @Test func noUpdaterNoMenuItem() {
        #expect(UpdatesModel(updater: nil, info: Self.info).menuItem == nil)
    }

    /// Both menus and Settings read the AppModel's one `UpdatesModel`: the
    /// state the menus show is the page's `update.get`.
    @Test func bothMenusAndSettingsShareTheOneModel() throws {
        let (model, updater) = made()
        updater.simulateFound(version: "1.2.3")
        updater.simulateDownloaded()
        let menuBar = UpdateMenuButton(updates: model)
        let appMenu = UpdateMenuButton(updates: model)
        #expect(menuBar.updates === appMenu.updates)
        #expect(menuBar.updates.menuItem == appMenu.updates.menuItem)
        #expect(model.updateState.status == .downloaded && model.updateState.version == "1.2.3")
        #expect(model.menuItem?.badge == true)
    }

    @Test func theBadgedIconStaysATemplate() throws {
        let base = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { _ in true }
        #expect(MenuBarIcon.badge(base).isTemplate)
    }
}
