// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import Foundation
import Testing

/// The host service's login item, per channel: the released app keeps the
/// label, plist and spec it always had (a bundle with no stamped keys), and
/// Cua Spaces Local reads its own from the bundle.
@Suite("Helper service names")
struct HelperServiceTests {
    let home = "/Users/ada"

    @Test func aBundleWithoutKeysIsTheReleasedApp() {
        for info in [nil, [:], ["CuaChannel": "stable"], ["CuaHelperLabel": "  ", "CuaHome": ""]] as [[String: Any]?] {
            #expect(HelperService.label(info: info) == "com.trycua.spaces.helper")
            #expect(HelperService.plistName(info: info) == "com.trycua.spaces.helper.plist")
            #expect(HelperService.specPath(info: info, home: home) == "/Users/ada/.cua/helper/service.json")
        }
    }

    @Test func theReleasedAppStampedWithItsOwnNamesKeepsThem() {
        let info: [String: Any] = ["CuaChannel": "stable", "CuaHelperLabel": "com.trycua.spaces.helper", "CuaHome": "~/.cua"]
        #expect(HelperService.label(info: info) == "com.trycua.spaces.helper")
        #expect(HelperService.plistName(info: info) == "com.trycua.spaces.helper.plist")
        #expect(HelperService.specPath(info: info, home: home) == "/Users/ada/.cua/helper/service.json")
    }

    @Test func localReadsItsOwnHelperFromTheBundle() {
        let info: [String: Any] = [
            "CuaChannel": "local", "CuaHelperLabel": "com.trycua.spaces.local.helper", "CuaHome": "~/.cua-local",
        ]
        #expect(HelperService.label(info: info) == "com.trycua.spaces.local.helper")
        #expect(HelperService.plistName(info: info) == "com.trycua.spaces.local.helper.plist")
        #expect(HelperService.specPath(info: info, home: home) == "/Users/ada/.cua-local/helper/service.json")
    }

    @Test func anAbsoluteHomeIsUsedAsIs() {
        #expect(HelperService.specPath(info: ["CuaHome": "/tmp/h"], home: home) == "/tmp/h/helper/service.json")
        #expect(HelperService.specPath(info: ["CuaHome": "~"], home: home) == "/Users/ada/helper/service.json")
    }

    @Test func aBundleOfAnotherChannelNeverReadsTheReleasedAppsSpec() {
        // No home, or one that is not a path: run nothing rather than the
        // released app's helper.
        #expect(HelperService.specPath(info: ["CuaChannel": "local"], home: home) == nil)
        #expect(HelperService.specPath(info: ["CuaChannel": "local", "CuaHome": "cua-local"], home: home) == nil)
        #expect(HelperService.specPath(info: ["CuaHome": "cua-local"], home: home) == nil)
    }

    /// The host service runs from Contents/Helpers, outside Contents/MacOS:
    /// from Contents/MacOS, AppKit (its cursor overlay) registered it with
    /// LaunchServices as the app itself, and opening Cua Spaces activated
    /// the windowless helper instead of showing a window.
    @Test func theHostServiceRunsFromContentsHelpers() {
        let exe = URL(fileURLWithPath: "/Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac")
        let daemon = HelperService.daemonPath(executable: exe)
        #expect(daemon.path == "/Applications/Cua Spaces.app/Contents/Helpers/cua-spacesd")
        #expect(!daemon.path.contains("/Contents/MacOS/"))
        #expect(HelperService.daemonInContents == "Helpers/cua-spacesd")
    }

    /// CUA-2457: the running service follows its files. An update in place
    /// (new files at the same paths) restarts it onto the new build; files
    /// gone twice in a row stop it; a swap's moment without the file does not.
    @Test func theServiceRestartsOntoAnUpdatedApp() {
        typealias ID = HelperService.FileIdentity
        let old: [ID?] = [ID(device: 1, inode: 10), ID(device: 1, inode: 11)]
        let new: [ID?] = [ID(device: 1, inode: 20), ID(device: 1, inode: 21)]
        var misses = 0
        #expect(HelperService.watch(started: old, now: old, specPresent: true, misses: &misses) == .keep)
        #expect(HelperService.watch(started: old, now: new, specPresent: true, misses: &misses) == .restart)
        // Only the cua-spacesd replaced is an update too.
        #expect(HelperService.watch(started: old, now: [old[0], new[1]], specPresent: true, misses: &misses) == .restart)
        // The swap: gone once, then the new build.
        misses = 0
        #expect(HelperService.watch(started: old, now: [nil, old[1]], specPresent: true, misses: &misses) == .keep)
        #expect(HelperService.watch(started: old, now: new, specPresent: true, misses: &misses) == .restart)
        #expect(misses == 0)
        // Files gone twice with the host still set up (a slow swap, a
        // downgrade): not a removal. Exit 75 so launchd tries again; exit 0
        // left the helper stopped after the 0.22.0 update (CUA-3746).
        misses = 0
        #expect(HelperService.watch(started: old, now: [old[0], nil], specPresent: true, misses: &misses) == .keep)
        #expect(HelperService.watch(started: old, now: [old[0], nil], specPresent: true, misses: &misses) == .retryLater)
        // The spec gone twice (the host removed): stop.
        misses = 0
        #expect(HelperService.watch(started: old, now: old, specPresent: false, misses: &misses) == .keep)
        #expect(HelperService.watch(started: old, now: old, specPresent: false, misses: &misses) == .stop)
        #expect(HelperService.restartExitCode != 0)
    }

    /// CUA-3746: what the launcher exits with decides whether launchd leaves
    /// it stopped (0) or starts it again.
    @Test func theLauncherComesBackUnlessItWasStoppedOnPurpose() {
        let exit = HelperService.launcherExitCode
        // An update's restart, whenever it comes.
        #expect(exit(false, true, 0, 100, false) == 75)
        #expect(exit(true, true, 0, 1, false) == 75)
        // Stopped long after it started (launchctl kill, logout): stays stopped.
        #expect(exit(true, false, 0, 3600, false) == 0)
        // SIGTERM 22 ms after the start: a restarter, not a person. Comes back.
        #expect(exit(true, false, 0, 0.022, false) == 75)
        // The host was removed: stays stopped even at once.
        #expect(exit(true, false, 0, 0.022, true) == 0)
        // The child died by itself: always a failure, even exit 0.
        #expect(exit(false, false, 0, 50, false) == 1)
        #expect(exit(false, false, 3, 50, false) == 3)
    }

    /// CUA-3746: the app starts a helper that should run with backoff, and
    /// never gives up.
    @Test func theRepairBacksOffAndStartsOverOnceItRuns() {
        var b = HelperService.RepairBackoff()
        let t0 = Date(timeIntervalSince1970: 1_800_000_000)
        #expect(b.due(now: t0))
        b.attempted(now: t0)
        #expect(!b.due(now: t0.addingTimeInterval(14)))
        #expect(b.due(now: t0.addingTimeInterval(15)))
        b.attempted(now: t0.addingTimeInterval(15))
        #expect(!b.due(now: t0.addingTimeInterval(44)))
        #expect(b.due(now: t0.addingTimeInterval(45)))
        for i in 0..<20 { b.attempted(now: t0.addingTimeInterval(1000 + Double(i))) }
        // Capped at 5 minutes, still going after many tries.
        #expect(!b.due(now: t0.addingTimeInterval(1019 + 299)))
        #expect(b.due(now: t0.addingTimeInterval(1019 + 300)))
        b.healthy()
        #expect(b.due(now: t0) && b.attempts == 0)
    }

    @Test func aBigServiceLogIsMovedAside() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-helper-log-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let log = dir.appendingPathComponent("host.log").path
        try Data(repeating: 65, count: 100).write(to: URL(fileURLWithPath: log))
        HelperService.rotateLog(log, max: 1000)
        #expect(FileManager.default.fileExists(atPath: log))
        HelperService.rotateLog(log, max: 50)
        #expect(!FileManager.default.fileExists(atPath: log))
        #expect(FileManager.default.fileExists(atPath: log + ".1"))
        HelperService.rotateLog(log, max: 0)
    }

    @Test func aFileReplacedAtTheSamePathHasANewIdentity() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-helper-id-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("cua-spacesd")
        try Data("old".utf8).write(to: file)
        let before = HelperService.identity(file.path)
        #expect(before != nil)
        #expect(HelperService.identity(file.path) == before)
        // As an update swaps the bundle: the new file renamed over the old.
        let staged = dir.appendingPathComponent("cua-spacesd.new")
        try Data("new".utf8).write(to: staged)
        #expect(rename(staged.path, file.path) == 0)
        #expect(HelperService.identity(file.path) != before)
        try FileManager.default.removeItem(at: file)
        #expect(HelperService.identity(file.path) == nil)
    }

    /// CUA-2457: at launch, a host service that started before this build's
    /// cua-spacesd was put in place is from the old build, and is restarted.
    @Test func aServiceFromBeforeTheUpdateIsStale() {
        let installed = Date(timeIntervalSince1970: 1_800_000_000)
        #expect(HelperService.isStale(serviceStarted: installed.addingTimeInterval(-3600), daemonInstalled: installed))
        #expect(!HelperService.isStale(serviceStarted: installed.addingTimeInterval(5), daemonInstalled: installed))
        #expect(HelperService.parsePid("gui/501/com.trycua.spaces.helper = {\n\tstate = running\n\tpid = 72613\n}") == 72613)
        #expect(HelperService.parsePid("\tstate = not running\n") == nil)
        // This process started after its own executable was written.
        let me = Bundle.main.executablePath ?? CommandLine.arguments[0]
        let born = HelperService.placedTime(me)
        let started = HelperService.processStart(getpid())
        #expect(born != nil && started != nil)
        if let born, let started { #expect(!HelperService.isStale(serviceStarted: started, daemonInstalled: born)) }
        #expect(HelperService.processStart(-1) == nil)
    }

    /// The host service and its commands never start the Sentry SDK (or
    /// anything else that checks a process in with LaunchServices); the app
    /// does. scripts/tests/helper-not-an-app.sh checks the built bundle.
    @Test func theServiceCommandsAreNotTheApp() {
        for args in [["--service-helper", "run"], ["--service-helper", "status"], ["check-permissions"], ["preflight-permissions"]] {
            #expect(HelperService.isServiceCommand(args), "\(args)")
        }
        for args in [[], ["-NSDocumentRevisionsDebugMode", "YES"], ["--check-launch-only"], ["cua://add/ABCD"]] {
            #expect(!HelperService.isServiceCommand(args), "\(args)")
        }
        // This test process is not an app another service could be mistaken for.
        #expect(!HelperService.serviceHoldsAppEntry(servicePid: getpid(), bundleId: "com.trycua.spaces.macos"))
    }
}
