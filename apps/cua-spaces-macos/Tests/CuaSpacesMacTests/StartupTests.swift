// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

@testable import CuaSpacesMacKit
import CuaSpacesFFI
import Foundation
import Testing

/// The launch never waits on the keychain or the daemon on the main thread:
/// the window shows `StartupModel` while a scripted keychain (`FixtureKeychain`,
/// or a stand-in `cua` script) and a slow or blocked start run. No real
/// keychain, daemon or SDK is touched.
@MainActor
@Suite struct StartupTests {
    /// Polls `condition` on the main actor (up to `seconds`).
    func eventually(_ seconds: Double = 30, _ condition: @MainActor () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(10))
        }
        return condition()
    }

    /// A start that blocks until `release` resolves (the daemon or the
    /// keychain read taking forever), counting its runs.
    final class SlowStart: @unchecked Sendable {
        let release = LiveGate<Bool>()
        var runs = 0
    }

    func startup(_ keychain: FixtureKeychain?, _ slow: SlowStart) -> StartupModel {
        let s = StartupModel(phase: .starting)
        s.keychain = keychain
        s.start = {
            slow.runs += 1
            _ = await slow.release.wait()
        }
        s.slowAfter = .milliseconds(50)
        s.startingSlowAfter = .milliseconds(50)
        return s
    }

    @Test func aBlockedStartShowsStartingAndNeverBlocksTheMainThread() async {
        let slow = SlowStart()
        let s = startup(FixtureKeychain(quiet: .ready), slow)
        var readied = 0
        s.onReady.append { readied += 1 }
        s.begin()
        // The main actor keeps running while the start is blocked.
        #expect(await eventually { slow.runs == 1 })
        #expect(s.phase == .starting)
        #expect(s.copy.title == "Starting Cua…")
        #expect(await eventually { s.slow })
        #expect(s.copy.title == "Still starting Cua…")
        #expect(s.copy.actions.isEmpty)
        slow.release.resolve(true)
        #expect(await eventually { s.isReady })
        #expect(readied == 1)
        #expect(slow.runs == 1)
        // Buttons do nothing once it started.
        s.act(.signInAgain)
        try? await Task.sleep(for: .milliseconds(30))
        #expect(slow.runs == 1)
    }

    @Test func anUntrustedItemAsksBeforeAnyPrompt() async {
        let slow = SlowStart()
        let keychain = FixtureKeychain(quiet: .needsAccess(locked: false), prompted: .ready)
        let s = startup(keychain, slow)
        s.begin()
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        // No prompt and no start until the user clicks.
        try? await Task.sleep(for: .milliseconds(80))
        #expect(keychain.calls == ["check"])
        #expect(slow.runs == 0)
        #expect(s.copy.title == "Allow Keychain access")
        #expect(s.copy.actions == [.allowAccess, .signInAgain])
        s.act(.allowAccess)
        #expect(await eventually { slow.runs == 1 })
        // The prompt's answer is checked again without one.
        #expect(keychain.calls == ["check", "prompt", "check"])
        slow.release.resolve(true)
        #expect(await eventually { s.isReady })
    }

    @Test func aPromptLeftOpenSaysSoAndCanBeTriedAgain() async {
        let slow = SlowStart()
        let keychain = FixtureKeychain(quiet: .needsAccess(locked: false), prompted: nil)
        let s = startup(keychain, slow)
        s.begin()
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        s.act(.allowAccess)
        #expect(await eventually { s.phase == .waitingForKeychain })
        #expect(s.copy.title == "Waiting for Keychain access…")
        #expect(s.copy.body.contains("behind other windows"))
        #expect(s.copy.actions.isEmpty)
        // After the timeout: still waiting, with a way out.
        #expect(await eventually { s.slow })
        #expect(s.copy.title == "Still waiting for Keychain access")
        #expect(s.copy.actions == [.tryAgain, .signInAgain])
        // Try again stops the open prompt and asks once more.
        keychain.prompted = .ready
        s.act(.tryAgain)
        #expect(await eventually { slow.runs == 1 })
        #expect(keychain.calls == ["check", "prompt", "cancel", "prompt", "check"])
        #expect(s.phase == .starting)
        slow.release.resolve(true)
        #expect(await eventually { s.isReady })
    }

    @Test func aKilledOrFailedPromptIsNeverTakenAsAccess() async {
        let slow = SlowStart()
        // The prompt process was killed (or failed): no access was given.
        let keychain = FixtureKeychain(quiet: .needsAccess(locked: false), prompted: .failed("stopped"))
        let s = startup(keychain, slow)
        s.begin()
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        s.act(.allowAccess)
        #expect(await eventually { keychain.calls == ["check", "prompt", "check"] })
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        #expect(s.note == "macOS didn't give access. Click Allow access to ask again.")
        #expect(s.copy.body.hasPrefix("macOS didn't give access. Click Allow access to ask again. "))
        #expect(slow.runs == 0)
        // A prompt that answered "ready" while access is still missing: the same.
        keychain.prompted = .ready
        keychain.grants = false
        s.act(.allowAccess)
        #expect(await eventually { keychain.calls.count == 5 })
        #expect(await eventually { s.phase == .needsKeychain(locked: false) && s.note != nil })
        #expect(slow.runs == 0)
    }

    @Test func aPromptThatLeftTheScreenSaysSo() async {
        let slow = SlowStart()
        let keychain = FixtureKeychain(quiet: .needsAccess(locked: false), prompted: nil)
        let s = startup(keychain, slow)
        s.hiddenAfter = .milliseconds(50)
        var showing = true
        s.promptShowing = { showing }
        s.begin()
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        s.act(.allowAccess)
        #expect(await eventually { s.phase == .waitingForKeychain })
        try? await Task.sleep(for: .milliseconds(120))
        #expect(!s.promptHidden)
        showing = false
        #expect(await eventually { s.promptHidden })
        #expect(s.copy.title == "The password prompt isn't showing")
        #expect(s.copy.body == "The macOS password prompt was closed or is hidden. Click Try again to show it again.")
        #expect(s.copy.actions == [.tryAgain, .signInAgain])
        // Try again stops the old prompt and shows a new one.
        showing = true
        s.act(.tryAgain)
        #expect(await eventually { keychain.calls == ["check", "prompt", "cancel", "prompt"] })
        #expect(!s.promptHidden)
    }

    @Test func aStartThatHangsIsBoundedAndCanBeTriedAgain() async {
        let slow = SlowStart()
        let s = startup(FixtureKeychain(quiet: .ready), slow)
        s.startTimeout = .milliseconds(100)
        var restarts = 0
        s.restart = { restarts += 1 }
        s.begin()
        #expect(await eventually { s.phase == .startFailed })
        #expect(s.copy.title == "Cua's background service didn't start")
        #expect(s.copy.actions == [.tryAgain])
        s.act(.tryAgain)
        #expect(await eventually { slow.runs == 2 })
        #expect(restarts == 1)
        // Either start finishing hands the services over.
        slow.release.resolve(true)
        #expect(await eventually { s.isReady })
    }

    @Test func aStartWaitingOnTheKeychainAsksForAccess() async {
        let slow = SlowStart()
        let keychain = FixtureKeychain(quiet: .ready, prompted: .ready)
        let s = startup(keychain, slow)
        s.startTimeout = .milliseconds(100)
        s.begin()
        #expect(await eventually { slow.runs == 1 })
        // Access went away meanwhile (an item another build rewrote).
        keychain.quiet = .needsAccess(locked: false)
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        #expect(s.note == "Cua is waiting for Keychain access.")
        s.act(.allowAccess)
        slow.release.resolve(true)
        #expect(await eventually { s.isReady })
    }

    @Test func aDeniedPromptOffersTryAgainAndSignInAgainOnly() async {
        let slow = SlowStart()
        let keychain = FixtureKeychain(quiet: .needsAccess(locked: true), prompted: .denied)
        let s = startup(keychain, slow)
        s.begin()
        #expect(await eventually { s.phase == .needsKeychain(locked: true) })
        #expect(s.copy.title == "Unlock your keychain")
        s.act(.allowAccess)
        #expect(await eventually { s.phase == .keychainDenied })
        #expect(s.copy.actions == [.tryAgain, .signInAgain])
        #expect(slow.runs == 0)
        // Nothing is removed until Sign in again is pressed.
        #expect(!keychain.calls.contains("forget"))
        s.act(.signInAgain)
        #expect(await eventually { slow.runs == 1 })
        #expect(keychain.calls.last == "forget")
        slow.release.resolve(true)
        #expect(await eventually { s.isReady })
    }

    @Test func aCheckThatCannotRunNeverBlocksTheLaunch() async {
        let slow = SlowStart()
        slow.release.resolve(true)
        let s = startup(FixtureKeychain(quiet: .failed("no such subcommand")), slow)
        s.begin()
        #expect(await eventually { s.isReady })
        // No keychain check at all (bare builds): straight to the start.
        let bare = startup(nil, slow)
        bare.begin()
        #expect(await eventually { bare.isReady })
        #expect(slow.runs == 2)
    }

    @Test func fixturesAndTestsStartReady() {
        #expect(StartupModel().isReady)
        #expect(AppEnvironment.fixtureStartup(nil).isReady)
        #expect(AppEnvironment.fixtureStartup("needs-keychain").phase != .ready)
    }

    // MARK: - The stand-ins

    @Test func theGateWakesEveryWaiterOnce() async {
        let gate = LiveGate<Int>()
        #expect(gate.current == nil)
        async let a = gate.wait()
        async let b = gate.wait()
        try? await Task.sleep(for: .milliseconds(20))
        gate.resolve(1)
        gate.resolve(2)
        #expect(await a == 1)
        #expect(await b == 1)
        #expect(await gate.wait() == 1)
        #expect(gate.current == 1)
    }

    @Test func pendingServicesWaitForTheLiveOnes() async throws {
        let gate = LiveGate<LiveServices>()
        let backend = PendingSpacesBackend(gate: gate)
        let account = PendingAccount(gate: gate)
        // Synchronous reads answer nothing yet (never a keychain read).
        #expect(account.identity() == nil)
        #expect(backend.reportedHostname(id: "x") == nil)
        async let rows = backend.rows()
        try? await Task.sleep(for: .milliseconds(20))
        gate.resolve(LiveServices(backend: FixtureSpacesBackend(), account: FixtureAccount(identity: "dana@example.com")))
        #expect(try await !rows.isEmpty)
        #expect(account.identity() == "dana@example.com")
    }

    @Test func aStartThatFailedSaysWhyInsteadOfWaiting() async {
        let gate = LiveGate<LiveServices>()
        gate.resolve(LiveServices(backend: FixtureSpacesBackend(rows: []), startError: "no SDK"))
        await #expect(throws: NotStarted.self) { try await PendingAccount(gate: gate).beginSignIn() }
        await #expect(throws: NotStarted.self) { try await PendingSpacesBackend(gate: gate).cloudTool("cloud_status", [:]) }
        await #expect(throws: NotStarted.self) { try await PendingDevices(gate: gate).snapshot() }
        #expect(await PendingDevices(gate: gate).checkEnrolled() == false)
    }

    @Test func theModelReadsTheAccountOnceTheLiveServicesAreIn() async {
        let gate = LiveGate<LiveServices>()
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-startup-\(UUID().uuidString)")
        let startup = StartupModel(phase: .starting)
        let model = AppModel(backend: PendingSpacesBackend(gate: gate), keyvault: KeyvaultModel(client: nil),
                             onboarding: OnboardingModel(statePath: dir.appendingPathComponent("onboarding.json").path),
                             settingsPath: dir.appendingPathComponent("settings.json").path,
                             account: PendingAccount(gate: gate), telemetry: FixtureTelemetry(), startup: startup)
        #expect(model.identity == nil)
        // The menu says why nothing shows yet.
        #expect(model.menuBar.first?.label == "Starting Cua…")
        var seen: [String] = []
        model.onLive { _ in seen.append("live") }
        gate.resolve(LiveServices(backend: FixtureSpacesBackend(), account: FixtureAccount(identity: "dana@example.com")))
        model.attachLive(nil)
        #expect(model.identity == "dana@example.com")
        #expect(model.onboarding.state.identity == "dana@example.com")
        // No SDK backend (fixtures): what waits for one never runs.
        #expect(seen.isEmpty)
    }

    // MARK: - The bundled cua's check

    func fakeCua(_ body: String) throws -> String {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("cua-keychain-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent("cua").path
        try "#!/bin/sh\n\(body)\n".write(toFile: path, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
        return path
    }

    @Test func theCheckReadsCuaAuthKeychain() {
        typealias K = BundledCuaKeychain
        #expect(K.parse(#"{"state":"ready","items":[]}"#) == .ready)
        #expect(K.parse(#"{"state":"not_used","items":[]}"#) == .ready)
        #expect(K.parse("notice: something\n" + #"{"state":"needs_access","items":[{"account":"cua-cli"}]}"#)
                == .needsAccess(locked: false))
        #expect(K.parse(#"{"state":"locked","items":[]}"#) == .needsAccess(locked: true))
        // Pretty-printed too (one key per line).
        #expect(K.parse("{\n  \"items\": [],\n  \"state\": \"needs_access\"\n}\n") == .needsAccess(locked: false))
        #expect(K.parse(#"{"state":"denied","items":[]}"#) == .denied)
        #expect(K.parse(#"{"state":"failed","items":[{"account":"cua-cli","state":"failed","error":"boom"}]}"#)
                == .failed("boom"))
        #expect(K.parse("error: unrecognized subcommand 'keychain'") == .failed("error: unrecognized subcommand 'keychain'"))
    }

    @Test nonisolated func tryAgainKillsAPromptThatIgnoresSigterm() async throws {
        // A prompt process that ignores SIGTERM (blocked on SecurityAgent),
        // reporting whether it may prompt.
        let cua = try await fakeCua(#"""
            case "$*" in
              "auth keychain --prompt") trap '' TERM; echo "${CUA_KEYCHAIN_NONINTERACTIVE:-interactive}" >> "$0.log"; exec sleep 30 ;;
              "auth keychain") echo "${CUA_KEYCHAIN_NONINTERACTIVE:-interactive}" >> "$0.log"; echo '{"state":"needs_access","items":[]}' ;;
            esac
            """#)
        let keychain = BundledCuaKeychain(cua: cua, quietTimeout: 5, killAfter: 0.5)
        /// Waits until the fake has logged `n` runs (the machine may be slow).
        func logged(_ n: Int) async -> Bool {
            for _ in 0..<300 {
                let text = (try? String(contentsOfFile: cua + ".log", encoding: .utf8)) ?? ""
                if text.split(separator: "\n").count >= n { return true }
                try? await Task.sleep(for: .milliseconds(50))
            }
            return false
        }
        async let first = keychain.check(prompt: true)
        #expect(await logged(1))
        let firstPid = try #require(keychain.runningPid)
        // Try again: the old one is killed before the new one starts.
        async let second = keychain.check(prompt: true)
        #expect(await first == .failed("stopped"))
        #expect(kill(firstPid, 0) != 0)
        #expect(await logged(2))
        let secondPid = try #require(keychain.runningPid)
        #expect(secondPid != firstPid)
        keychain.cancel()
        #expect(await second == .failed("stopped"))
        #expect(await keychain.check(prompt: false) == .needsAccess(locked: false))
        // Only the clicked prompt may prompt; the quiet check never.
        let log = try String(contentsOfFile: cua + ".log", encoding: .utf8)
        #expect(log.split(separator: "\n").map(String.init) == ["interactive", "interactive", "1"], "\(log)")
    }

    @Test nonisolated func theSupervisorFindsTheDaemonsToRestart() throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("cua-pids-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        #expect(DaemonSupervisor.daemonPids(cuaHome: home).isEmpty)
        try "4242\n".write(to: home.appendingPathComponent("daemon.starting"), atomically: true, encoding: .utf8)
        try #"{"pid":4343,"version":"x"}"#.write(to: home.appendingPathComponent("daemon.json"), atomically: true, encoding: .utf8)
        #expect(DaemonSupervisor.daemonPids(cuaHome: home) == [4242, 4343])
    }

    @Test nonisolated func theBundledCuaRunsTheRightCommandAndCanBeStopped() async throws {
        let cua = try await fakeCua(#"""
            case "$*" in
              "auth keychain") echo '{"state":"needs_access","items":[]}' ;;
              "auth keychain --forget") echo '{"state":"ready","items":[]}' ;;
              "auth keychain --prompt") exec sleep 30 ;;
            esac
            """#)
        let keychain = BundledCuaKeychain(cua: cua, quietTimeout: 5)
        #expect(await keychain.check(prompt: false) == .needsAccess(locked: false))
        #expect(await keychain.forget() == .ready)
        // A prompt left open: cancel stops it (and its dialog with it), long
        // before its 30 s are up.
        async let prompted = keychain.check(prompt: true)
        var tries = 0
        while keychain.runningPid == nil, tries < 6000 { tries += 1; try await Task.sleep(for: .milliseconds(10)) }
        let pid = try #require(keychain.runningPid)
        keychain.cancel()
        #expect(await prompted == .failed("stopped"))
        #expect(kill(pid, 0) != 0)
        // A quiet check that hangs is bounded.
        let hung = BundledCuaKeychain(cua: try await fakeCua("exec sleep 30"), quietTimeout: 0.5)
        #expect(await hung.check(prompt: false) == .failed("stopped"))
    }

    // MARK: - Activation and New UI

    @Test func onlyAUsersLaunchComesToTheFront() {
        #expect(AppDelegate.activatesAtLaunch(loginLaunch: false, fixtures: false, asked: false))
        #expect(!AppDelegate.activatesAtLaunch(loginLaunch: true, fixtures: false, asked: false))
        #expect(!AppDelegate.activatesAtLaunch(loginLaunch: false, fixtures: true, asked: false))
        #expect(AppDelegate.activatesAtLaunch(loginLaunch: false, fixtures: true, asked: true))
    }

    @Test func newUIGetsTheSameStateAndWords() async {
        let s = StartupModel(phase: .starting)
        s.keychain = FixtureKeychain(quiet: .needsAccess(locked: false))
        s.begin()
        #expect(await eventually { s.phase == .needsKeychain(locked: false) })
        let state = WebUIBridge.startupState(s)
        #expect(state["phase"] as? String == "needsKeychain")
        #expect(state["slow"] as? Bool == false)
        #expect(state["title"] as? String == "Allow Keychain access")
        #expect(state["actions"] as? [String] == ["allowAccess", "signInAgain"])
        #expect(WebUIBridge.startupState(StartupModel())["phase"] as? String == "ready")
        #expect(WebUIBridge.methods.contains("startup.get") && WebUIBridge.methods.contains("startup.act"))
    }
}
