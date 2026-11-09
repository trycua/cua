// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSpacesFFI
import Foundation
import Testing

/// What closing the window and quitting do to the background service:
/// the state machine (`BackgroundPolicy`), the stop (`BackgroundService`)
/// and the settings behind them. No process is touched.
@Suite("Background service", .serialized)
struct BackgroundServiceTests {
    // MARK: Closing the window

    @Test func closingTheWindowKeepsRunningAndShowsTheNoticeByDefault() {
        let defaults = appSettingsDefaults()
        #expect(defaults.closeWindowKeepsRunning)
        #expect(!defaults.closeNoticeOff)
        #expect(BackgroundPolicy.windowClosed(keepsRunning: defaults.closeWindowKeepsRunning,
                                              noticeOff: defaults.closeNoticeOff, terminating: false) == .showNotice)
    }

    @Test func theNoticeIsSkippedOnceTurnedOff() {
        #expect(BackgroundPolicy.windowClosed(keepsRunning: true, noticeOff: true, terminating: false) == .nothing)
    }

    @Test func theQuitSettingQuitsWhateverTheNoticeSays() {
        #expect(BackgroundPolicy.windowClosed(keepsRunning: false, noticeOff: false, terminating: false) == .quitApp)
        #expect(BackgroundPolicy.windowClosed(keepsRunning: false, noticeOff: true, terminating: false) == .quitApp)
    }

    @Test func windowsClosingBecauseTheAppQuitsDoNothing() {
        for keeps in [true, false] {
            for off in [true, false] {
                #expect(BackgroundPolicy.windowClosed(keepsRunning: keeps, noticeOff: off, terminating: true) == .nothing)
            }
        }
    }

    @Test func theNoticeSaysWhatTheOwnerAskedFor() {
        #expect(BackgroundPolicy.noticeText
            == "Cua Spaces is still running in the background so your agents can create and manage their Spaces.")
        for text in [BackgroundPolicy.noticeText, BackgroundPolicy.noticeTitle] {
            #expect(!text.contains("\u{00b7}"))
            #expect(!text.lowercased().contains("pool"))
        }
    }

    @Test func theSettingRowsFollowTheCoreChoice() throws {
        var settings = appSettingsDefaults()
        let rows = { (s: AppSettings) in
            let json = #"{"closeWindowKeepsRunning":\#(s.closeWindowKeepsRunning),"closeNotice":\#(!s.closeNoticeOff)}"#
            return appSettingsPage(input: try! appSettingsInputFromJson(json: json)).sections.flatMap(\.rows)
        }
        let close = try #require(rows(settings).first { $0.id == "close-window" })
        #expect(close.label == "When I close the window")
        #expect(close.options.map(\.label) == ["Keep running in the background", "Quit Cua Spaces"])
        settings = appSettingsChoose(settings: settings, row: "close-window", option: "quit")
        settings = appSettingsChoose(settings: settings, row: "close-notice", option: "off")
        // What the row shows follows the saved choice (saving is the core's serde, tested there;
        // `appSettingsSave` also mirrors experiments into ~/.cua, which a test must never touch).
        #expect(!settings.closeWindowKeepsRunning && settings.closeNoticeOff)
        #expect(rows(settings).first { $0.id == "close-window" }?.options.first { $0.active }?.id == "quit")
    }

    @Test func anOlderSettingsFileKeepsRunningWithTheNoticeOn() throws {
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("old-\(UUID().uuidString).json").path
        try #"{"menuBar":true}"#.write(toFile: path, atomically: true, encoding: .utf8)
        let loaded = appSettingsLoad(path: path)
        #expect(loaded.closeWindowKeepsRunning && !loaded.closeNoticeOff)
    }

    // MARK: Quit

    @Test func quitStopsEverythingExceptForAnUpdateOrAFixturesRun() {
        #expect(BackgroundPolicy.quitPlan(updateRestart: false, managesService: true) == .stopEverything)
        #expect(BackgroundPolicy.quitPlan(updateRestart: true, managesService: true) == .leaveRunning)
        #expect(BackgroundPolicy.quitPlan(updateRestart: false, managesService: false) == .leaveRunning)
    }

    @Test func aQuitForAnUpdateLeavesTheServiceRunning() throws {
        let state = BackgroundService()
        state.manage(try fakeSupervisor())
        state.markUpdateRestart()
        #expect(state.beginQuit() == .leaveRunning)
        #expect(state.isQuitting)
    }

    @Test func aQuitCancelledGoesBackToRunning() throws {
        let state = BackgroundService()
        let supervisor = try fakeSupervisor()
        state.manage(supervisor)
        #expect(state.beginQuit() == .stopEverything)
        #expect(state.isQuitting && supervisor.isStopping)
        state.cancelQuit()
        #expect(!state.isQuitting && !supervisor.isStopping)
    }

    @Test func stopTermsEachLoginItemThenStopsTheDaemon() throws {
        let runner = RecordingRunner()
        let state = BackgroundService()
        let supervisor = try fakeSupervisor()
        state.manage(supervisor)
        let problems = state.stopEverything(runner: runner, labels: ["a.helper", "a.driver"], uid: 501, wait: 0.2,
                                            isRunning: { _ in false })
        #expect(problems.isEmpty)
        #expect(runner.calls.prefix(2).map { $0.joined(separator: " ") }
            == ["/bin/launchctl kill SIGTERM gui/501/a.helper", "/bin/launchctl kill SIGTERM gui/501/a.driver"])
        #expect(runner.calls.dropFirst(2).first?.suffix(2) == ["daemon", "stop"])
        // The supervision loop must not start the daemon again.
        #expect(supervisor.isStopping)
    }

    @Test func stopSaysWhatDidNotStop() throws {
        let state = BackgroundService()
        state.manage(try fakeSupervisor())
        let problems = state.stopEverything(runner: RecordingRunner(), labels: ["a.helper"], uid: 501, wait: 0.3,
                                            isRunning: { _ in true })
        #expect(problems == ["a.helper is still running"])
    }

    @Test func aStoppingAppNeverStartsTheDaemonAgain() throws {
        let supervisor = try fakeSupervisor()
        supervisor.stopSupervising()
        #expect(supervisor.start() == "Cua Spaces is quitting")
    }

    // MARK: Helpers

    private func fakeSupervisor() throws -> DaemonSupervisor {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-background-\(UUID().uuidString)/Cua Spaces.app/Contents/MacOS")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent("cua").path
        try "#!/bin/sh\nexit 0\n".write(toFile: path, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
        let supervisor = try #require(DaemonSupervisor(bundledCua: path))
        supervisor.homeOverride = dir.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        return supervisor
    }
}

private final class RecordingRunner: CommandRunning, @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: [[String]] = []
    var calls: [[String]] { lock.withLock { recorded } }

    func run(_ executable: String, _ arguments: [String], timeout: TimeInterval) -> CommandResult {
        lock.withLock { recorded.append([executable] + arguments) }
        return CommandResult(status: 0, output: "")
    }
}
