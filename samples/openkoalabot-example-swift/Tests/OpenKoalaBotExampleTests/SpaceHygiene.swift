// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// Everything this suite does to make sure it leaves the Space exactly as it
/// found it.
///
/// **Why this file exists.** The live suites used to clean up per test, in
/// `tearDown`, from a list of closures each test appended to. That is the right
/// shape and it still is not enough, for two reasons found by running it
/// against a real demo machine and then looking at the machine:
///
/// 1. **A run whose id a test never learned is a run nothing can clean up.**
///    Several call sites registered their teardown *after* an assertion — so a
///    failing assertion, or a throw between starting the run and registering
///    it, leaked the run permanently. Cleanup that only runs on the happy path
///    is not cleanup.
/// 2. **A crashed or interrupted test process runs no `tearDown` at all.**
///
/// So per-test cleanup is kept (it is still the fastest way to return a Space
/// to rest between tests) and a **sweeper** is added underneath it. The sweeper
/// does not depend on any test having registered anything, or on a naming
/// convention that a future test might forget: it snapshots the set of agent
/// runs in the Space *before* the bundle starts, and removes every run that
/// appeared while the bundle was running. A run cannot hide from that.
///
/// Finally it re-counts the Space's windows and compares against the count it
/// took before the first test. A suite that cannot show the Space is back where
/// it started has not proved it cleaned up.
///
/// See `FRICTION.md` §54 and §55.
enum SpaceHygiene {

    /// Run one async SDK call to completion from synchronous code.
    ///
    /// The SDK is `async` throughout — that is the whole point of it, and it is
    /// what keeps a Spaces round trip off the main actor in the app. But
    /// `XCTestObservation.testBundleDidFinish` is a synchronous delegate
    /// callback, and it is the *only* hook guaranteed to run after every test
    /// however the bundle ended. So the sweeper needs a bridge.
    ///
    /// Blocking on a semaphore is safe **here** specifically: this runs on the
    /// test bundle's own thread, never the main actor, and the work it waits on
    /// is an `actor` hop onto the cooperative pool that can never re-enter this
    /// thread. The same trick in the app would be the defect `FRICTION.md` §47
    /// was about; the asymmetry is deliberate, not an oversight.
    static func sync<T>(_ body: @escaping @Sendable () async throws -> T) -> T? {
        let semaphore = DispatchSemaphore(value: 0)
        nonisolated(unsafe) var out: T?
        Task.detached {
            out = try? await body()
            semaphore.signal()
        }
        // A teardown that hangs is worse than one that fails: cap the wait.
        _ = semaphore.wait(timeout: .now() + 60)
        return out
    }

    /// Remove every trace of one run, with **no Automation and no TCC prompt**.
    ///
    /// `agent_stop` kills the agent process and (since this change) unloads the
    /// LaunchAgent, removes its plist and kills the run's own log watcher. What
    /// it deliberately leaves is the run *directory*, because `agent_status`
    /// has to keep answering after a run ends. A test has no such need, so it
    /// removes that too.
    ///
    /// What this explicitly does **not** do is
    /// `osascript -e 'tell application "Terminal" to close …'`. Driving
    /// Terminal from another process is an Automation request; on a machine
    /// that has not already granted it, it raises a consent dialog on the
    /// user's desktop, and a test suite must never do that. It does not need
    /// to: tests run with `OPENKOALABOTS_AGENT_WINDOWS=0`, so their runs never
    /// open a Terminal window in the first place — see `installTestDefaults()`.
    static func remove(run: String, client: SDKSpacesClient, space: String) {
        // `delete()` is the SDK's whole teardown for a run (FRICTION.md §8):
        // stop the process, remove its run directory, and verify the window
        // is gone against the Space's own window list.
        if sync({ try await client.deleteRun(space: space, runID: run) }) == nil {
            _ = sync { try await client.stopBot(space: space, runID: run) }
        }
        // The macOS extras the SDK's delete already covers, kept as a backstop
        // for a Space whose runs predate it.
        if isMacOS(client: client, space: space) {
            _ = sync { try await client.raw("space_bash", [
                "space": space,
                "command":
                    "pkill -f 'watch-\(run).command' 2>/dev/null; "
                    + "rm -rf ~/.spaces-agents/\(run); "
                    + "rm -f ~/Library/LaunchAgents/com.trycua.agentrun.\(run).plist; "
                    + "true",
            ]) }
        }
    }

    /// Whether a Space is a macOS one (Terminal and LaunchAgent cleanup only
    /// applies there).
    static func isMacOS(client: SDKSpacesClient, space: String) -> Bool {
        (sync { try await client.listSpaces() } ?? [])
            .first { $0.id == space }?.os.lowercased().hasPrefix("mac") ?? false
    }

    static func forgetRestorableTerminalWindows(client: SDKSpacesClient, space: String) {
        guard isMacOS(client: client, space: space) else { return }
        _ = sync { try await client.raw("space_bash", [
            "space": space,
            // **`kill -9`, and the -9 is the entire point.**
            //
            // A plain `pkill` sends SIGTERM, which Terminal handles as a
            // graceful quit — and a graceful quit *writes its Resume state*,
            // listing every window that was open. The next time anything
            // launches Terminal, macOS faithfully reopens all of them. So
            // killing Terminal to clear a hundred leaked windows does not clear
            // them; it stores them, and they come back the next time an agent
            // run opens a terminal. That is what "more and more opening
            // infinitely" was.
            //
            // Deleting the state file afterwards is a race you lose: the dying
            // process writes it after the `rm`. `NSQuitAlwaysKeepsWindows` and
            // `ApplePersistenceIgnoreState` were both measured and neither
            // suppressed it. SIGKILL does, because a process that is killed
            // outright never gets to save anything — verified directly: after
            // SIGTERM, relaunching Terminal produced 100 shells; after SIGKILL
            // with the state removed, exactly 1.
            //
            // All of this is signals and files. No Automation, no TCC prompt.
            "command": "pkill -9 -x Terminal 2>/dev/null; sleep 2; "
                + "rm -rf \"$HOME/Library/Saved Application State/"
                + "com.apple.Terminal.savedState\"; "
                + "defaults write com.apple.Terminal NSQuitAlwaysKeepsWindows "
                + "-bool false 2>/dev/null; true",
        ]) }
    }

    /// The run ids currently in the Space.
    static func runIDs(client: SDKSpacesClient, space: String) -> Set<String> {
        Set((sync { try await client.listBots(space: space) } ?? []).map(\.id))
    }

    static func windowCount(client: SDKSpacesClient, space: String) -> Int {
        sync { try await client.windows(space: space).count } ?? -1
    }

    static func installTestDefaults() {
        // Set directly, not via `setenv`: Foundation caches
        // `ProcessInfo.environment` on first read, so an environment variable
        // set from inside the test process is invisible to the client. The
        // variable is still honoured for callers who set it *before* launch.
        SDKSpacesClient.showsAgentWindows = false
        setenv(SDKSpacesClient.agentWindowsVariable, "0", 1)
    }
}

/// Runs the sweep around each live suite (the `.liveSpace` trait).
///
/// XCTest had `XCTestObservation.testBundleDidFinish`; swift-testing has
/// suite-scoped traits, which run after every test in the suite however each
/// one ended — passed, failed, threw, or skipped. A sweep that finds leftovers
/// records an issue against the suite, so the run cannot report success.
final class SpaceSweeper: @unchecked Sendable {

    private var target: LiveSpace.Target?
    private var runsBefore: Set<String> = []
    private var windowsBefore = -1

    func snapshot() async {
        guard let t = LiveSpace.target else { return }
        target = t
        SpaceHygiene.installTestDefaults()
        // Before the first test, not only after: the suite opens exactly one
        // Terminal window on a macOS Space (the window-list test) and it must
        // not be reopened on Terminal's next launch.
        SpaceHygiene.forgetRestorableTerminalWindows(client: t.client, space: t.space)
        runsBefore = SpaceHygiene.runIDs(client: t.client, space: t.space)
        windowsBefore = SpaceHygiene.windowCount(client: t.client, space: t.space)
        print("SWEEPER: \(t.space) before the suite — "
              + "\(runsBefore.count) runs, \(windowsBefore) windows")
    }

    func sweep() async {
        guard let t = target else { return }
        let (c, s) = (t.client, t.space)
        let ours = SpaceHygiene.runIDs(client: c, space: s).subtracting(runsBefore)
        if !ours.isEmpty {
            print("SWEEPER: removing \(ours.count) run(s) this suite created: "
                  + ours.sorted().joined(separator: ", "))
        }
        for run in ours.sorted() { SpaceHygiene.remove(run: run, client: c, space: s) }
        SpaceHygiene.forgetRestorableTerminalWindows(client: c, space: s)

        let runsLeft = SpaceHygiene.runIDs(client: c, space: s).subtracting(runsBefore)
        let windowsAfter = SpaceHygiene.windowCount(client: c, space: s)
        print("SWEEPER: after the suite — \(windowsAfter) windows "
              + "(was \(windowsBefore)), \(runsLeft.count) of our runs left")
        var problems: [String] = []
        if !runsLeft.isEmpty {
            problems.append("runs this suite created are still in the Space: "
                            + runsLeft.sorted().joined(separator: ", "))
        }
        if windowsBefore >= 0, windowsAfter > windowsBefore {
            problems.append("the Space has \(windowsAfter) windows, "
                            + "\(windowsAfter - windowsBefore) more than the "
                            + "\(windowsBefore) it had before the suite")
        }
        if !problems.isEmpty {
            Issue.record(Comment(rawValue: "SWEEPER FAILED: the suite did not leave \(s) as it found it:\n"
                                 + problems.map { "  - " + $0 }.joined(separator: "\n")))
        } else {
            print("SWEEPER: \(s) is back to its pre-suite state.")
        }
    }
}
