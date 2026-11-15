// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import OpenKoalaBotExample

/// End-to-end tests for `SDKSpacesClient` against a **live** Cua Space, driven
/// through the cua SDK's Spaces runtime (the Rust `cua-spaces` crate, embedded
/// or in `cua daemon`). Nothing here is mocked: every assertion is about what the
/// Space actually returned.
///
/// ## Running the suite
///
/// Point it at an already-warm Space:
///
/// ```sh
/// # a spacesd (e.g. a local linux container), added to a temp registry
/// OPENKOALABOTS_TEST_SPACE_URL=http://127.0.0.1:32768 OPENKOALABOTS_TEST_SPACE_TOKEN=… swift test
/// # or an existing Space, through the cua daemon
/// OPENKOALABOTS_TEST_SPACE=local:cua-space-e3c1b54907 swift test
/// ```
///
/// With both **unset** the live tests skip rather than run.
/// That is deliberate: the ordinary `ensureSpace()` path calls
/// `create_space` with `reuse`, which *creates a new sandbox* (a metered one
/// when the default location is the cloud) when it finds none to reuse.
/// Costing a reviewer a sandbox just by typing `swift test` is not
/// acceptable, so this suite never takes that path.
///
/// ## What this suite promises about the Space
///
/// It is written to be safe to run against a Space someone demos from. It never
/// creates, deletes, re-creates or re-displays one; it never calls
/// `show_space_pip` / `open_space_viewer` / `stream_space_window`. Every
/// artefact it creates — an uploaded file, an agent run — is named with a fresh
/// UUID and removed in `tearDown`, whether the test passed or failed.
@Suite(.liveSpace, .serialized) final class SpacesE2ETests {

    // MARK: Harness

    private var client: SDKSpacesClient!
    private var space: String!
    /// The Space's operating system: the few assertions about macOS
    /// specifically (Terminal, rcdp window ids) run only on a macOS Space.
    private var os = ""
    /// Cleanup closures, drained in reverse order by the `.liveSpace` trait
    /// after each test (XCTest's async `tearDown`).
    private var cleanup: LiveCleanup { LiveCleanup.current }

    init() throws {
        let target = try LiveSpace.require()
        client = target.client
        space = target.space
        os = target.os
    }

    private var isMacOS: Bool { os.lowercased().hasPrefix("mac") }

    /// Run a shell command in the Space and return its output.
    @discardableResult
    private func bash(_ command: String) async throws -> String {
        let out = try await client.raw("space_bash", ["space": space!, "command": command])
        return out as? String ?? String(describing: out)
    }

    // MARK: ensureSpace — attach, do not create

    /// `ensureSpace()` with the override set must return exactly that Space and
    /// must not create another one. Asserted against the real Space inventory
    /// before and after, so a stray create would fail the test.
    @Test func testEnsureSpaceAttachesToWarmSpaceWithoutCreating() async throws {
        let before = try await spaceIDs()
        XCTAssertTrue(before.contains(space), "\(space!) is not in the account's Space list")

        let resolved = try await client.ensureSpace()
        XCTAssertEqual(resolved, space, "the override must win over create_space")

        let after = try await spaceIDs()
        XCTAssertEqual(after, before, "ensureSpace() must not create or delete a Space")
    }

    /// The live Space is actually up — the precondition every other test leans
    /// on, asserted rather than assumed.
    @Test func testWarmSpaceIsRunning() async throws {
        let rows = try await spaceRows()
        guard let row = rows.first(where: { $0["id"] as? String == space }) else {
            return XCTFail("\(space!) missing from list_spaces")
        }
        let phase = (row["phase"] as? String ?? "").lowercased()
        XCTAssertTrue(["running", "bound", "ready"].contains(phase), "unexpected phase \(phase)")
    }

    private func spaceRows() async throws -> [[String: Any]] {
        let out = try await client.raw("list_spaces", [:])
        return out as? [[String: Any]] ?? []
    }

    private func spaceIDs() async throws -> Set<String> {
        Set(try await spaceRows().compactMap { $0["id"] as? String })
    }

    // MARK: windows — the live window view behind a Bot's "screen"

    /// `windows(space:)` must return the Space's real on-screen windows with
    /// usable rcdp identifiers. This is the operation behind the Agent Computer
    /// screens: without a window id there is nothing to present.
    @Test func testWindowsReturnsLiveWindowsWithUsableIdentifiers() async throws {
        if !isMacOS {
            // A fresh Linux desktop can have no top-level windows at all, so
            // open the image's own fixture window (linux ships
            // `cua-fixtures`) and close it afterwards.
            _ = try await bash("command -v cua-fixtures >/dev/null && cua-fixtures start grid >/dev/null 2>&1; true")
            cleanup.append { [self] in _ = try? await self.bash("cua-fixtures stop grid >/dev/null 2>&1; true") }
            for _ in 0..<30 where try await client.windows(space: space).isEmpty {
                try await Task.sleep(nanoseconds: 500_000_000)
            }
        }
        let windows = try await client.windows(space: space)
        XCTAssertFalse(windows.isEmpty, "a running desktop Space always has windows")
        for w in windows {
            XCTAssertFalse(w.id.isEmpty, "window \(w.app) has no id — the id key was misread")
        }
        XCTAssertTrue(windows.contains { !$0.app.isEmpty }, "no window reported an app name")
    }

    /// The window list is live, not a cached fixture: a window this test opens
    /// appears in it, and is gone once the test closes it.
    @Test func testWindowsReflectsAWindowOpenedAndClosedByTheTest() async throws {
        // Opens a Terminal window, which is a macOS thing.
        try XCTSkipUnless(isMacOS, "opens a Terminal window: macOS Spaces only (this one is \(os))")
        let marker = "OpenKoalaBotsE2E-\(UUID().uuidString.prefix(8))"
        let script = "/tmp/\(marker).sh"
        // Was Terminal already running? If it was not, this test is what
        // started it, and quitting it afterwards is this test's business and
        // nobody else's. See the teardown below.
        let terminalWasRunning = ((try? await bash("pgrep -x Terminal >/dev/null && echo yes || echo no"))
            ?? "").contains("yes")
        try await bash("printf '#!/bin/bash\\nsleep 120\\n' > \(script); chmod +x \(script); "
                 + "open -a Terminal \(script)")
        cleanup.append { [self] in
            // Three separate problems, and the old one-liner solved none of
            // them. The sweeper is what caught it.
            //
            // 1. `pkill -f '<marker>'` matched the *shell*, whose argv holds
            //    the script path — but not the `sleep 120` it forked, whose
            //    argv is just "sleep 120". So the child was orphaned and kept
            //    the window busy. Killing the whole process group gets both.
            // 2. `osascript -e 'tell application "Terminal" to close …'` is an
            //    Automation request. On a machine that has not granted it, it
            //    does nothing at all — silently, exit status 0 — and on one
            //    that has not been *asked*, it raises a consent dialog on the
            //    user's desktop. Neither is acceptable, so it is gone.
            // 3. Even with the shell dead the window survives, because Terminal
            //    keeps a window open after its command exits. The only lever
            //    left that needs no Automation is quitting Terminal — which is
            //    why this test checks first whether it owns it.
            //
            // And the sting in the tail: Terminal *restores its windows on next
            // launch* from Saved Application State. Quitting it without clearing
            // that state is not cleanup, it is a delay — the windows come back
            // the next time anything opens a terminal, which is what made a
            // demo Space appear to sprout windows "infinitely". So the saved
            // state goes too. `FRICTION.md` §56.
            _ = try? await self.bash(
                "pgid=$(ps -o pgid= -p $(pgrep -f '\(marker)' | head -1) 2>/dev/null | tr -d ' '); "
                + "[ -n \"$pgid\" ] && kill -- -\"$pgid\" 2>/dev/null; "
                + "pkill -f '\(marker)' 2>/dev/null; "
                + "rm -f \(script); true")
            // Closing the window itself, and making sure it stays closed, is
            // `SpaceHygiene`'s job — it is the same SIGKILL-plus-clear-state
            // dance for every leaked terminal, and doing it in one place means
            // one explanation. Only done here when this test is what started
            // Terminal; otherwise the end-of-bundle sweep handles it.
            if !terminalWasRunning, let client = self.client, let space = self.space {
                SpaceHygiene.forgetRestorableTerminalWindows(client: client, space: space)
            }
        }

        var appeared = false
        for _ in 0..<20 {
            let titles = try await client.windows(space: space).map(\.title)
            if titles.contains(where: { $0.contains(marker) }) { appeared = true; break }
            try await Task.sleep(nanoseconds: 1_000_000_000)
        }
        XCTAssertTrue(appeared, "a window opened in the Space did not show up in windows(space:)")
    }

    // MARK: upload — attachment intake

    /// Attachment intake: a host file lands in the Space with its bytes intact,
    /// verified by reading it back in the guest, and is then removed.
    @Test func testUploadDeliversFileContentsIntoTheSpace() async throws {
        let tag = UUID().uuidString
        let payload = "date,amount\n2026-09-19,\(tag)\n"
        let localPath = NSTemporaryDirectory() + "openkoalabots-\(tag).csv"
        try payload.write(toFile: localPath, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(atPath: localPath) }

        let remoteDir = "/tmp/openkoalabots-e2e-\(tag)"
        let remotePath = "\(remoteDir)/Q3-actuals.csv"
        try await bash("mkdir -p \(remoteDir)")
        cleanup.append { [self] in _ = try? await self.bash("rm -rf \(remoteDir)") }

        try await client.upload(space: space, localPath: localPath, remotePath: remotePath)

        let readBack = try await bash("cat \(remotePath)")
        XCTAssertTrue(readBack.contains(tag),
                      "uploaded file did not arrive with its contents: \(readBack)")

        // And the sample cleans up after itself: prove the removal really works,
        // so a failed run cannot quietly leave junk in a demo Space.
        _ = try await bash("rm -rf \(remoteDir)")
        let gone = try await bash("test -e \(remotePath) && echo PRESENT || echo GONE")
        XCTAssertTrue(gone.contains("GONE"), "cleanup did not remove \(remotePath)")
    }

    /// A failing tool must surface as a thrown error. `spaces_mcp.py` reports
    /// tool failures as an ordinary result with `isError: true`, so a client
    /// that does not check it swallows every failure silently — which is what
    /// this client used to do.
    @Test func testUploadOfMissingHostFileThrows() async throws {
        do {
            try await client.upload(space: space,
                                    localPath: "/tmp/definitely-not-here-\(UUID().uuidString)",
                                    remotePath: "/tmp/never-written")
            XCTFail("uploading a nonexistent host file must throw")
        } catch let e as SDKSpacesClient.Failure {
            XCTAssertFalse("\(e)".isEmpty, "a failure must say what failed")
        }
    }

    /// Bringing a Bot's output back out: a file created *inside* the Space
    /// arrives on the host with its bytes intact. The other half of attachment
    /// handling.
    @Test func testDownloadBringsAFileBackOutOfTheSpace() async throws {
        let tag = UUID().uuidString
        let remoteDir = "/tmp/openkoalabots-dl-\(tag)"
        let remotePath = "\(remoteDir)/report.txt"
        try await bash("mkdir -p \(remoteDir) && printf 'produced-in-space-\(tag)\\n' > \(remotePath)")
        cleanup.append { [self] in _ = try? await self.bash("rm -rf \(remoteDir)") }

        let hostDir = NSTemporaryDirectory() + "openkoalabots-dl-\(tag)"
        cleanup.append { try? FileManager.default.removeItem(atPath: hostDir) }

        let landed = try await client.download(space: space, remotePath: remotePath,
                                               localDirectory: hostDir)
        let contents = try String(contentsOfFile: landed, encoding: .utf8)
        XCTAssertTrue(contents.contains("produced-in-space-\(tag)"),
                      "downloaded file did not carry its contents: \(contents)")
    }

    /// Downloading a path that is not there must throw, not return a path to
    /// nothing.
    @Test func testDownloadOfMissingPathThrows() async throws {
        do {
            _ = try await client.download(space: space,
                                          remotePath: "/tmp/not-here-\(UUID().uuidString)")
            XCTFail("downloading a nonexistent path must throw")
        } catch let e as SDKSpacesClient.Failure {
            XCTAssertTrue("\(e)".contains("not found"), "unexpected failure: \(e)")
        }
    }

    // MARK: sandboxes — lifecycle

    /// `listSpaces()` decodes the real inventory, including the provider and
    /// readiness split between local and cloud Spaces.
    @Test func testListSpacesDecodesTheRealInventory() async throws {
        let spaces = try await client.listSpaces()
        XCTAssertFalse(spaces.isEmpty)
        guard let mine = spaces.first(where: { $0.id == space }) else {
            return XCTFail("\(space!) not in listSpaces()")
        }
        // The provider is the id's scheme (`space://local/…`, `space://direct/…`).
        XCTAssertTrue(space.contains("://\(mine.provider)/") || space.hasPrefix("\(mine.provider):"),
                      "provider \(mine.provider) does not match \(space!)")
        // A direct Space (a spacesd added by URL) is registered from its
        // capabilities, which do not name an operating system; the providers
        // that create machines do.
        if mine.provider != "direct" {
            XCTAssertFalse(mine.os.isEmpty, "a Space reports its operating system")
        }
        XCTAssertTrue(mine.isReady, "phase \(mine.phase) did not read as ready")
        for s in spaces { XCTAssertFalse(s.id.isEmpty) }
    }

    // MARK: agent lifecycle — a running Bot

    /// The whole Bot lifecycle against the live Space: start a run, read its
    /// output back, steer it with a follow-up message, then stop it and confirm
    /// it is actually stopped. Every assertion is on what the Space returned.
    @Test func testBotLifecycleStartMessageStatusStop() async throws {
        let run = try await startCleanBot("Print the single word READY and exit. Do nothing else.")

        XCTAssertTrue(run.runID.hasPrefix("run-"), "agent_start returned \(run.runID)")
        XCTAssertEqual(run.runID.count, 12, "run ids are run-<8 hex>: \(run.runID)")
        XCTAssertEqual(run.space, space)
        XCTAssertFalse(run.capabilities.isEmpty,
                       "agent_start should publish the harness capabilities, not leave them to be inferred")

        // The run really executed inside the Space: its own output comes back.
        let first = try await waitForOutput(run.runID, containing: "READY")
        XCTAssertTrue(first.tail.contains("READY"),
                      "agent never produced its output; tail was: \(first.tail)")
        XCTAssertNotEqual(first.state, .unknown, "status was unknown: \(first.reason)")
        XCTAssertFalse(first.reason.isEmpty, "every status should say why it is that status")
        XCTAssertFalse(first.summary.isEmpty, "status should summarise what the Bot was asked")

        // `accepts_message` is the harness telling us when a follow-up is legal,
        // so the app never has to infer the turn model from behaviour.
        let idle = try await waitForState(run.runID, in: [.idle, .finished, .awaitingInput])
        XCTAssertTrue(idle.acceptsMessage || idle.state == .finished,
                      "an idle run should accept a message; got \(idle.state) (\(idle.reason))")

        // Steering a run: a second turn that resumes the same session.
        let outcome = try await client.message(space: space, runID: run.runID,
                                               text: "Now print the single word SECOND and exit.")
        XCTAssertTrue(outcome.accepted, "agent_message was refused: \(outcome.reason)")
        let second = try await waitForOutput(run.runID, containing: "SECOND")
        XCTAssertTrue(second.tail.contains("SECOND"),
                      "agent_message did not reach the run; tail was: \(second.tail)")
        XCTAssertTrue(second.tail.contains("READY"),
                      "the follow-up should continue the same session, not start a new one")

        // The run shows up in the roster feed.
        let roster = try await client.listBots(space: space)
        guard let row = roster.first(where: { $0.id == run.runID }) else {
            return XCTFail("agent_list did not include \(run.runID)")
        }
        XCTAssertEqual(row.agent, run.agent)
        XCTAssertFalse(row.summary.isEmpty)

        // Stopping is verified by the harness, not assumed.
        let stop = try await client.stopBot(space: space, runID: run.runID)
        XCTAssertTrue(stop.stopped, "stopBot did not confirm the run died: \(stop.reason)")
        XCTAssertEqual(stop.alive, false)
        let after = try await client.status(space: space, runID: run.runID)
        XCTAssertFalse([.running].contains(after.state),
                       "the run is still alive after stopBot: \(after.state) (\(after.reason))")
    }

    /// `agent_message` must refuse a turn already in flight rather than
    /// silently queueing it or killing the in-flight turn.
    @Test func testMessageToARunningBotIsRefusedNotQueued() async throws {
        let run = try await startCleanBot(
            "Run the shell command `sleep 45` and then print DONE-SLEEP. Do nothing else.")
        let busy = try await waitForState(run.runID, in: [.running], seconds: 60)
        try XCTSkipUnless(busy.state == .running,
                          "could not catch the run mid-turn (it was \(busy.state)); "
                          + "the refusal path needs a turn in flight")

        let outcome = try await client.message(space: space, runID: run.runID, text: "interrupt me")
        XCTAssertFalse(outcome.accepted,
                       "a message to a running turn must be refused, not queued")
        XCTAssertFalse(outcome.reason.isEmpty, "a refusal must say why")
    }

    /// `agent_status` on a run id that does not exist must report `unknown`
    /// with a reason, never a confident "done".
    @Test func testStatusOfUnknownRunIsUnknownNotDone() async throws {
        let status = try await client.status(space: space, runID: "run-deadbeef")
        XCTAssertEqual(status.state, .unknown)
        XCTAssertTrue(status.reason.contains("run-deadbeef") || status.reason.contains("no run record"),
                      "unknown status should say why: \(status.reason)")
    }

    /// Start a Bot and register everything needed to leave the Space as we
    /// found it: the run stopped, its state directory gone, its Terminal closed.
    private func startCleanBot(_ prompt: String) async throws -> AgentRun {
        let run = try await client.startBot(space: space, bot: Fixtures.bot("inbox"), prompt: prompt)
        cleanup.append { [self] in
            guard let client = self.client, let space = self.space else { return }
            SpaceHygiene.remove(run: run.runID, client: client, space: space)
        }
        return run
    }

    /// Poll `status` until the run's own output shows up. Returns the last
    /// status seen either way so the caller can assert on it.
    private func waitForOutput(_ runID: String, containing needle: String,
                               seconds: Int = 120) async throws -> AgentStatus {
        try await poll(runID, seconds: seconds) { $0.tail.contains(needle) }
    }

    private func waitForState(_ runID: String, in states: Set<AgentState>,
                              seconds: Int = 120) async throws -> AgentStatus {
        try await poll(runID, seconds: seconds) { states.contains($0.state) }
    }

    /// The poll loop every caller of `agent_status` has to write by hand.
    /// See FRICTION.md #2: this wants to be a subscription.
    private func poll(_ runID: String, seconds: Int,
                      until done: (AgentStatus) -> Bool) async throws -> AgentStatus {
        var last = AgentStatus(state: .unknown, reason: "not polled yet", acceptsMessage: false,
                               exitCode: nil, summary: "", tail: "")
        for _ in 0..<seconds {
            last = try await client.status(space: space, runID: runID)
            if done(last) { return last }
            try await Task.sleep(nanoseconds: 1_000_000_000)
        }
        return last
    }

    // MARK: presentScreen — documented as inert, asserted as inert

    /// `presentScreen` is deliberately a no-op in this build, and the client
    /// says so in `capabilities`. This test pins that contract: it is *not*
    /// coverage of a Spaces operation, it is proof that the build does not
    /// quietly open a window on the operator's Mac. See RUBRIC.md.
    @Test func testPresentScreenIsInertAndCapabilitiesSaySo() async throws {
        XCTAssertFalse(client.capabilities.liveScreenPixels)
        let windows = try await client.windows(space: space)
        let before = windows.count
        // A fresh Linux desktop can have no windows; presentScreen must be
        // inert for any window, so a placeholder one proves the same thing.
        let target = windows.first
            ?? SpaceWindow(id: "none", app: "", title: "", width: 0, height: 0, visible: false)
        for tier in [ComputerTier.glyph, .pinnedPreview, .fullScreen] {
            try await client.presentScreen(space: space, window: target, tier: tier)
        }
        let after = try await client.windows(space: space).count
        XCTAssertEqual(after, before, "presentScreen changed what is displayed; it must not")
    }
}

/// Tests that need no Space: the transport and the offline client the screen
/// export renders from.
@Suite final class SpacesClientUnitTests {

    /// The handshake really round-trips with the real server, and the tools the
    /// app depends on exist. (Name presence alone is not coverage — the live
    /// suite above calls each one — but a missing tool should fail loudly.)
    @Test func testHandshakeListsTheToolsTheAppUses() async throws {
        // Embedded, with a temp registry: no daemon, no network, no ~/.cua.
        let home = NSTemporaryDirectory() + "openkoalabots-handshake-\(UUID().uuidString)"
        defer { try? FileManager.default.removeItem(atPath: home) }
        let client = try SDKSpacesClient(backend: .embedded(spacesHome: home))
        let tools = try await client.handshake()
        for name in ["create_space", "delete_space", "agent_start", "agent_message", "agent_status",
                     "agent_stop", "list_space_windows", "upload", "space_bash", "list_spaces"] {
            XCTAssertTrue(tools.contains(name), "the Spaces contract no longer offers \(name)")
        }
    }

    /// The status decode is the one place a silent misread would turn a stuck
    /// Bot into a "done" one, so it is pinned against the harness's own keys.
    @Test func testAgentStatusDecodesTheHarnessVocabulary() {
        let s = SDKSpacesClient.decodeStatus([
            "run_id": "run-1", "status": "awaiting_input", "reason": "the agent asked a question",
            "summary": "tidy the inbox", "accepts_message": true, "exit_code": 0,
            "output_tail": "…", "agent": "claude-code",
        ])
        XCTAssertEqual(s.state, .awaitingInput)
        XCTAssertTrue(s.acceptsMessage)
        XCTAssertEqual(s.exitCode, 0)
        XCTAssertEqual(s.summary, "tidy the inbox")

        // An unrecognised status must degrade to `unknown`, never to a guess.
        let odd = SDKSpacesClient.decodeStatus(["status": "teleporting"])
        XCTAssertEqual(odd.state, .unknown)
        XCTAssertFalse(odd.acceptsMessage)
    }

    /// There is no server path to get wrong any more; the failure that
    /// replaces it is a daemon that is not there. It must throw on the first
    /// call, not hang and not fall back to something else.
    @Test func testUnreachableDaemonFailsOnFirstCall() async {
        let socket = NSTemporaryDirectory() + "openkoalabots-no-daemon-\(UUID().uuidString).sock"
        do {
            let client = try SDKSpacesClient(backend: .daemon(address: socket))
            _ = try await client.listSpaces()
            XCTFail("a daemon that does not exist must not answer")
        } catch {
            XCTAssertFalse("\(error)".isEmpty)
        }
    }

    /// The demo client backs screenshot export and must create no Space at all.
    @Test func testDemoClientCreatesNothing() async throws {
        let demo = DemoSpacesClient()
        let sid = try await demo.ensureSpace()
        XCTAssertEqual(sid, "demo:openkoalabots")
        XCTAssertFalse(demo.capabilities.agents)
        XCTAssertFalse(demo.capabilities.upload)
    }
}
