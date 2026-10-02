// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import CuaSpacesFFI
@testable import CuaSpacesMacKit
import Foundation
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
        model.choose(channel: "beta")
        model.checkNow()
        // Sparkle installs what it found on its own schedule.
        updater.onUpdateEvent?("installed")
        let got = telemetry.recorded.map { ParityTests.signalFrame($0) }
            .map { "\($0["action"]!) \($0["channel"]!) \($0["trigger"]!)" }
        #expect(got == [
            "checked stable user", "not_found stable user",
            "checked beta user", "found beta user", "installed beta user",
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
        #expect(v.links.map(\.label) == ["Acknowledgements", "Privacy Policy", "Terms of Service", "Report an Issue\u{2026}"])
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

    @Test func noUpdaterNoControls() {
        let model = UpdatesModel(updater: nil, info: Self.info)
        #expect(model.view.updates == nil)
        #expect(model.view.links.count == 4)
    }

    @Test func theChannelIsSavedAndGivesSparkleItsChannels() {
        let updater = FixtureUpdater()
        let model = UpdatesModel(updater: updater, channel: .stable, info: Self.info)
        var saved: [AppUpdateChannel] = []
        model.saveChannel = { saved.append($0) }
        #expect(updater.channels.isEmpty)
        model.choose(channel: "beta")
        #expect(updater.channels == ["beta"])
        #expect(saved == [.beta])
        #expect(model.view.updates?.channels.first(where: \.active)?.id == "beta")
        model.choose(channel: "stable")
        #expect(updater.channels.isEmpty)
        #expect(saved == [.beta, .stable])
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
        s.updateChannel = .beta
        try? appSettingsSave(path: path, settings: s)
        // A build from before the updater kept no record: after the first
        // run, that is an update.
        #expect(UpdateRefresh.record(settingsPath: path, version: "0.2.0", build: "0.2.0.2", onboarded: true))
        let after = appSettingsLoad(path: path)
        #expect(after.menuBar && after.updateChannel == .beta)
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

    // MARK: - Acknowledgements

    @Test func theNoticesParseIntoBlocks() {
        let blocks = NoticeBlock.parse("""
            # Third-party notices

            Intro line one
            line two.

            | Project | Used in | What |
            |---|---|---|
            | [A](https://a) | `x.swift` | a thing |

            ## Sparkle

            ```
            Copyright (c) 2006 Someone
              indented
            ```
            """)
        #expect(blocks == [
            .heading(1, "Third-party notices"),
            .paragraph("Intro line one line two."),
            .row(["[A](https://a)", "`x.swift`", "a thing"]),
            .heading(2, "Sparkle"),
            .verbatim("Copyright (c) 2006 Someone\n  indented"),
        ])
    }

    @Test func theBundledNoticesNameSparkle() throws {
        let file = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("THIRD_PARTY_NOTICES.md")
        let blocks = NoticeBlock.parse(try String(contentsOf: file, encoding: .utf8))
        #expect(blocks.contains(.heading(2, "Sparkle")))
        #expect(blocks.contains { if case .verbatim(let t) = $0 { t.contains("Andy Matuschak") } else { false } })
    }
}
