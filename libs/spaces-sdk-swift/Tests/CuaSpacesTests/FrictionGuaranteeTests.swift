import CoreGraphics
import Foundation
import Testing
@testable import CuaSpaces

/// One test per guarantee the friction log asked the SDK for.
///
/// Each test names its entry. If a guarantee is ever quietly dropped, the test
/// that fails says which entry was being answered.
@Suite(.serialized) final class FrictionGuaranteeTests {

    private var backend: FakeSpacesBackend!
    private var connection: SpacesConnection!

    init() async throws {
        backend = FakeSpacesBackend()
        connection = SpacesConnection(transport: backend)
    }

    private func localSpace() async throws -> Space {
        try await connection.attach(to: "local:cua-space-test")
    }

    // MARK: §1 — the transport cannot drop bytes
    //
    // There is no stdio stream left to frame: every call is an SDK call
    // (`CuaSpacesTransport`). The line framer and its tests live with the
    // Rust control-plane client (`cua_spaces::client::framing`), and
    // `CuaBackedTests.testManyConsecutiveCallsStayInStep` runs the live
    // equivalent against a real spacesd.

    // MARK: §3 — a failure cannot be returned as a value

    @Test func testToolFailureThrowsRatherThanReturning() async throws {
        await backend.setFailNext("upload", "host path not found")
        let space = try await localSpace()
        let file = try temporaryFile("x")
        do {
            _ = try await space.upload(file, to: .exactPath("/tmp/x"))
            XCTFail("a failing tool must throw")
        } catch let error as SpacesError {
            XCTAssertTrue("\(error)".contains("host path not found"), "\(error)")
        }
    }

    // MARK: §4 — one result type, populated on both branches

    @Test func testDeliveryCarriesAReasonWhicheverBranchIsTaken() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "tidy the inbox")

        let refused = try await run.send("are you done?")
        XCTAssertFalse(refused.accepted)
        XCTAssertFalse(refused.reason.isEmpty, "a refusal must say why")
        XCTAssertEqual(refused.stateAtRefusal, .running)

        await backend.setRun(run.id.rawValue) { $0.acceptsMessage = true }
        let delivered = try await run.send("are you done?")
        XCTAssertTrue(delivered.accepted)
        XCTAssertFalse(delivered.reason.isEmpty, "a delivery must say why too")
    }

    /// §4: "none" answered as an English sentence must become an empty
    /// collection, not something an app has to parse.
    @Test func testProseForEmptyListBecomesAnEmptyCollection() async throws {
        await backend.setListSpacesAnswersProse(true)
        let spaces = try await connection.spaces()
        XCTAssertEqual(spaces, [])
    }

    // MARK: §5, §28, §33 — attach is not create

    @Test func testAttachNeverCreates() async throws {
        let before = try await connection.spaces().count
        _ = try await connection.attach(to: "local:cua-space-test")
        let after = try await connection.spaces().count
        XCTAssertEqual(after, before)
        let calls = await backend.calls
        XCTAssertFalse(calls.contains("create_space"))
    }

    /// §33: a caller handed a Space must be able to name it, and naming a
    /// Space that is not there must be an error rather than a fresh sandbox.
    @Test func testAttachingToAMissingSpaceThrowsRatherThanCreating() async throws {
        do {
            _ = try await connection.attach(to: "local:not-here")
            XCTFail("attaching to a missing Space must throw")
        } catch let error as SpacesError {
            guard case .spaceUnavailable = error else { return XCTFail("\(error)") }
        }
        let calls = await backend.calls
        XCTAssertFalse(calls.contains("create_space"), "a failed attach created a sandbox")
    }

    @Test func testAttachRefusesASpaceThatIsNotReady() async throws {
        do {
            _ = try await connection.attach(to: "cloud:space-dead")
            XCTFail("attaching to a Failed Space must throw")
        } catch let error as SpacesError {
            XCTAssertTrue("\(error)".contains("Failed"), "\(error)")
        }
    }

    /// The only call that can cost money spells its own name, and where it
    /// runs is an argument it always sends: a metered Space is never the
    /// result of a default the caller did not see.
    @Test func testCreatingIsASeparatelyNamedCallThatAlwaysSaysWhere() async throws {
        let space = try await connection.createSpace(on: .cloud)
        XCTAssertEqual(space.provider, .cloud)
        XCTAssertTrue(SpaceLocation.cloud.isMetered)
        XCTAssertFalse(SpaceLocation.local.isMetered)
        let calls = await backend.calls
        XCTAssertTrue(calls.contains("create_space"))
        let sent = await backend.lastCreateArguments
        XCTAssertEqual(sent?["on"]?.stringValue, "cloud")
        XCTAssertEqual(sent?["kind"]?.stringValue, "auto")
        XCTAssertEqual(sent?["runtime"]?.stringValue, "auto")
        XCTAssertEqual(sent?["reuse"]?.boolValue, false, "reuse is asked for, never assumed")
    }

    /// `wait: false` hands back a handle that says it is starting, not one
    /// that fails on first use.
    @Test func testCreatingWithoutWaitingIsStarting() async throws {
        let space = try await connection.createSpace(
            options: SpaceCreateOptions(on: .local, kind: .vm, runtime: .qemu,
                                        name: "later", wait: false))
        XCTAssertEqual(space.id, "local:later")
        XCTAssertEqual(space.info.state, .starting)
        let sent = await backend.lastCreateArguments
        XCTAssertEqual(sent?["kind"]?.stringValue, "vm")
        XCTAssertEqual(sent?["runtime"]?.stringValue, "qemu")
    }

    /// The engines offered follow the location and kind (the placement model).
    @Test func testRuntimesOfferedFollowTheLocation() {
        XCTAssertEqual(SpaceRuntime.offered(on: .local, kind: .container), [.auto, .gvisor, .runc])
        XCTAssertEqual(SpaceRuntime.offered(on: .local, kind: .vm), [.auto, .qemu, .lume])
        XCTAssertEqual(SpaceRuntime.offered(on: .cloud, kind: .container), [.auto, .gvisor])
        XCTAssertEqual(SpaceRuntime.offered(on: .cloud, kind: .vm), [.auto, .kubevirt])
        XCTAssertEqual(SpaceRuntime.kubevirt.kind, .vm)
        XCTAssertEqual(SpaceRuntime.runc.kind, .container)
        XCTAssertNil(SpaceRuntime.auto.kind)
    }

    /// Deleting a created Space deletes it; the result says what happened.
    @Test func testDeleteDeletesACreatedSpace() async throws {
        let space = try await connection.createSpace(on: .local, name: "gone")
        let said = try await space.delete()
        XCTAssertTrue(said.hasPrefix("Deleted"), said)
        let left = try await connection.spaces().map(\.id)
        XCTAssertFalse(left.contains(space.id))
    }

    // MARK: §6 — provider differences are normalised at the boundary

    @Test func testBothProvidersSpellReadyTheirOwnWayAndBothNormaliseToReady() async throws {
        let spaces = try await connection.spaces()
        let local = try XCTUnwrap(spaces.first { $0.provider == .local })
        let cloud = try XCTUnwrap(spaces.first { $0.id == "cloud:space-abc" })
        XCTAssertEqual(local.rawPhase, "running")
        XCTAssertEqual(cloud.rawPhase, "Bound")
        XCTAssertTrue(local.isReady)
        XCTAssertTrue(cloud.isReady)
        // The original string is normalised, never discarded.
        XCTAssertNotEqual(local.rawPhase, cloud.rawPhase)
    }

    @Test func testHomeAndCapabilitiesComeFromTheProviderNotTheCaller() async throws {
        XCTAssertEqual(SpaceProvider.local.home, "/Users/lume")
        XCTAssertEqual(SpaceProvider.cloud.home, "/root")
        // Every provider with a cua-spacesd streams: the media ticket comes
        // from the SDK, not from an ssh read of a per-boot token.
        XCTAssertTrue(SpaceProvider.local.capabilities.rcdpStreaming)
        XCTAssertTrue(SpaceProvider.cloud.capabilities.rcdpStreaming)
        XCTAssertTrue(SpaceProvider.direct.capabilities.rcdpStreaming)
        XCTAssertFalse(SpaceProvider.demo.capabilities.rcdpStreaming)
        XCTAssertTrue(SpaceProvider.cloud.capabilities.teleport)
        XCTAssertFalse(SpaceProvider.direct.capabilities.creation,
                       "a direct Space is not ours to create or delete")
        XCTAssertEqual(SpaceProvider(rawProvider: "cloud"), .cloud)
        XCTAssertEqual(SpaceProvider(rawProvider: "relay"), .relay)
    }

    /// A tool a provider does not have must fail as a capability, not as prose.
    @Test func testUnsupportedToolFailsAsACapability() async throws {
        await backend.addSpace(["id": "demo:none", "provider": "demo", "phase": "running"])
        let demo = try await connection.attach(to: "demo:none")
        do {
            _ = try await demo.streamEndpoint()
            XCTFail("a Space with no spacesd has no stream")
        } catch let error as SpacesError {
            guard case let .unsupportedByProvider(_, provider, _) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertEqual(provider, .demo)
        }
    }

    // MARK: §7 — ids are typed, and there is one spelling per concept

    /// The list tool names the identifier `window`; reading `window_id` alone
    /// yields an empty id for every window, silently.
    @Test func testWindowIdentifierIsReadFromTheSpellingTheListActuallyUses() async throws {
        let space = try await localSpace()
        let windows = try await space.windows()
        XCTAssertEqual(windows.count, 2)
        for w in windows {
            XCTAssertFalse(w.id.isEmpty, "empty window id is the failure §7 describes")
            XCTAssertTrue(w.id.looksLikeRCDPTarget, "\(w.id) is not an rcdp target")
        }
    }

    @Test func testIdentifierTypesAreDistinct() {
        XCTAssertEqual(RunID("run-1").directory, "~/.cua/agents/run-1")
        XCTAssertEqual(SpaceID("local:abc").providerPrefix, "local")
        XCTAssertEqual(SpaceID("space://direct/10.0.0.5:3211").providerPrefix, "direct")
        XCTAssertEqual(SpaceID("cloud:space-abc").providerPrefix, "cloud")
        XCTAssertEqual(SpaceID("space://cloud/ns/space").providerPrefix, "cloud")
        XCTAssertFalse(WindowID("run-1").looksLikeRCDPTarget)
    }

    // MARK: §8 — cleanup is one call

    @Test func testDeleteRemovesTheWholeRunNotJustTheProcess() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "do a thing")
        let existsBefore = try await space.fileExists(run.id.directory)
        XCTAssertTrue(existsBefore)

        let cleanup = try await run.delete()
        XCTAssertTrue(cleanup.processStopped)
        XCTAssertTrue(cleanup.directoryRemoved, "the run directory survived")
        XCTAssertTrue(cleanup.launchAgentRemoved, "the LaunchAgent plist survived")
        XCTAssertTrue(cleanup.terminalWindowClosed)
        XCTAssertTrue(cleanup.isComplete, "residue: \(cleanup.residue)")
        let existsAfter = try await space.fileExists(run.id.directory)
        XCTAssertFalse(existsAfter)
    }

    /// §8: teardown on **every** exit path, including a thrown error.
    /// A Terminal window that survived the close is **residue**, not a
    /// success. The close used to be an `osascript` that matched on the run
    /// id, and a real Space's Terminal windows are titled after the *script*
    /// they ran — so the match always missed, and a `delete()` that reported
    /// `terminalWindowClosed = true` unconditionally is how a demo machine
    /// accumulated a hundred orphaned windows. §8 asks for a call that names
    /// what it could not remove; §9 asks it not to claim a result it never
    /// verified.
    ///
    /// The close is signals now rather than Automation (see `delete()`), which
    /// changes nothing this test asserts: the point is that the *verification*
    /// is the window list, so a window that survives by any route is residue.
    @Test func testASurvivingTerminalWindowIsReportedAsResidueNotSuccess() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "do a thing")
        await backend.leaveWindow(titled: "lume — \(run.id.rawValue).sh — 120×30")

        let cleanup = try await run.delete()
        XCTAssertTrue(cleanup.directoryRemoved, "the run directory still has to go")
        XCTAssertFalse(cleanup.terminalWindowClosed,
                       "a window still in the Space's own window list was reported closed")
        XCTAssertFalse(cleanup.isComplete)
        XCTAssertTrue(cleanup.residue.contains { $0.hasPrefix("window: ") },
                      "the surviving window was not named: \(cleanup.residue)")
    }

    /// §54: the cheapest window to clean up is the one that was never opened.
    ///
    /// A run started through the SDK gets a Terminal window on a Local Space's
    /// desktop by default, because for a person using a Space with a screen
    /// that *is* the product. A test suite wants the opposite, and wanting it
    /// is not enough — the suite that set `OPENKOALABOTS_AGENT_WINDOWS=0` and had
    /// the value silently not arrive is how a demo machine collected windows
    /// twice over. So `showsWindow` is a request field that reaches the wire,
    /// and this asserts it does, in both positions.
    @Test func testAgentStartCarriesShowSoASuiteCanOpenNoWindows() async throws {
        let space = try await localSpace()

        _ = try await space.startAgent(prompt: "visible run")
        let shownByDefault = await backend.lastAgentStartShow
        XCTAssertEqual(shownByDefault, true,
                       "a run started normally must still show its window")

        _ = try await space.startAgent(prompt: "headless run", showsWindow: false)
        let shownWhenRefused = await backend.lastAgentStartShow
        XCTAssertEqual(shownWhenRefused, false,
                       "showsWindow: false did not reach agent_start — a suite that "
                       + "asks for no windows and gets them is what trashes a Space")
    }

    @Test func testWithAgentRunCleansUpWhenTheBodyThrows() async throws {
        let space = try await localSpace()
        struct Boom: Error {}
        var runID: RunID?
        do {
            _ = try await space.withAgentRun(AgentStartRequest(prompt: "x")) { run in
                runID = run.id
                throw Boom()
            }
            XCTFail("the body's error must propagate")
        } catch is Boom {}
        let id = try XCTUnwrap(runID)
        let stillThere = try await space.fileExists(id.directory)
        XCTAssertFalse(stillThere)
    }

    // MARK: §9 — what the MCP got right, kept

    @Test func testStatusOfAnUnknownRunIsUnknownWithAReasonNotADoneGuess() async throws {
        let space = try await localSpace()
        let snapshot = try await space.run("run-deadbeef").status()
        XCTAssertEqual(snapshot.state, .unknown)
        XCTAssertTrue(snapshot.reason.contains("run-deadbeef"), snapshot.reason)
        XCTAssertFalse(snapshot.acceptsMessage)
    }

    @Test func testAcceptsMessageIsPublishedNotInferred() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let before = try await run.status()
        XCTAssertFalse(before.acceptsMessage)
        await backend.setRun(run.id.rawValue) { $0.acceptsMessage = true }
        let after = try await run.status()
        XCTAssertTrue(after.acceptsMessage)
    }

    @Test func testStopReportsAVerifiedProbeAndNotAnAssumption() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let outcome = try await run.stop()
        XCTAssertTrue(outcome.stopped)
        XCTAssertEqual(outcome.alive, false)
        XCTAssertFalse(outcome.reason.isEmpty)
    }

    /// A probe that could not run is not "alive". `nil` is preserved.
    @Test func testAnUnprobableStopKeepsAliveNil() {
        let outcome = StopOutcome(["stopped": false, "reason": "probe failed"])
        XCTAssertNil(outcome.alive)
    }

    @Test func testTheStatusVocabularyIsIntact() {
        XCTAssertEqual(Set(AgentState.allCases.map(\.rawValue)),
                       ["running", "awaiting_input", "idle", "finished", "failed",
                        "crashed", "unknown"])
        XCTAssertEqual(AgentState(wire: "teleporting"), .unknown,
                       "an unrecognised state must degrade to unknown, never to a guess")
    }

    // MARK: §13, §14 — collision-safe uploads and declared limits

    @Test func testTwoUploadsOfTheSameFilenameStayTwoFiles() async throws {
        let space = try await localSpace()
        let a = try temporaryFile("from folder A", named: "notes.txt")
        let b = try temporaryFile("from folder B", named: "notes.txt")
        let first = try await space.upload(a, to: .collisionSafe(in: "/tmp/attachments"))
        let second = try await space.upload(b, to: .collisionSafe(in: "/tmp/attachments"))
        XCTAssertNotEqual(first.path, second.path, "the second upload clobbered the first")
        // §13's other half: the user's filename survives.
        XCTAssertTrue(first.path.hasSuffix("/notes.txt"), first.path)
        XCTAssertTrue(second.path.hasSuffix("/notes.txt"), second.path)
        let stored = await backend.uploadedBytes
        XCTAssertEqual(stored[first.path], "from folder A")
        XCTAssertEqual(stored[second.path], "from folder B")
    }

    @Test func testUploadReportsThePathItActuallyWrote() async throws {
        let space = try await localSpace()
        let file = try temporaryFile("hello", named: "report.csv")
        let written = try await space.upload(file, to: .exactPath("/tmp/exact/report.csv"))
        XCTAssertEqual(written.path, "/tmp/exact/report.csv")
        XCTAssertEqual(written.name, "report.csv")
        XCTAssertEqual(written.byteCount, 5)
    }

    @Test func testDeclaredLimitsAreCheckedBeforeAnyIO() async throws {
        let space = try await localSpace()
        let files = try (0..<7).map { try temporaryFile("x", named: "f\($0).txt") }
        do {
            _ = try await space.upload(files, to: .collisionSafe(in: "/tmp/a"),
                                       limits: .koalaBotsAttachments)
            XCTFail("seven files must exceed a six-file cap")
        } catch let error as SpacesError {
            guard case let .limitExceeded(v) = error else { return XCTFail("\(error)") }
            XCTAssertEqual(v, .tooManyFiles(count: 7, limit: 6))
        }
        let calls = await backend.calls
        XCTAssertFalse(calls.contains("upload"), "a refused batch uploaded something anyway")
    }

    @Test func testPerFileLimitNamesTheOffendingFile() async throws {
        let space = try await localSpace()
        let big = try temporaryFile(String(repeating: "x", count: 4096), named: "big.bin")
        let limits = TransferLimits(maxFileCount: 6, maxBytesPerFile: 1024,
                                    maxBytesPerBatch: .max)
        do {
            _ = try await space.upload(big, limits: limits)
            XCTFail("the file is over the cap")
        } catch let error as SpacesError {
            XCTAssertTrue("\(error)".contains("big.bin"), "\(error)")
        }
    }

    @Test func testLimitsAreDataNotCode() throws {
        let encoded = try JSONEncoder().encode(TransferLimits.koalaBotsAttachments)
        let decoded = try JSONDecoder().decode(TransferLimits.self, from: encoded)
        XCTAssertEqual(decoded, .koalaBotsAttachments)
        XCTAssertEqual(decoded.maxFileCount, 6)
    }

    // MARK: §21, §41 — metadata is not smuggled into the sentence the agent reads

    @Test func testMetadataRoundTripsAndIsStrippedFromEverySummaryTheSDKPublishes() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(
            prompt: "Tidy the inbox", metadata: ["bot": "inbox", "routine": "8am triage"])

        let snapshot = try await run.status()
        XCTAssertEqual(snapshot.summary, "Tidy the inbox",
                       "the marker leaked into a user-visible summary")
        // And the roster feed, which is where §21 caught it being missed.
        let roster = try await space.runs()
        XCTAssertEqual(roster.first { $0.id == run.id }?.summary, "Tidy the inbox")

        let echoed = RunMetadata.encode(prompt: "Tidy the inbox",
                                        metadata: ["bot": "inbox", "routine": "8am triage"])
        XCTAssertEqual(RunMetadata.decode(from: echoed),
                       ["bot": "inbox", "routine": "8am triage"])
        XCTAssertTrue(echoed.hasPrefix("Tidy the inbox"),
                      "the instruction must come first, the bookkeeping after")
    }

    @Test func testMetadataSurvivesSeparatorsInItsOwnValues() {
        let awkward = ["a=b": "x;y", "c": "-->"]
        let encoded = RunMetadata.encode(prompt: "p", metadata: awkward)
        XCTAssertEqual(RunMetadata.decode(from: encoded), awkward)
        XCTAssertEqual(RunMetadata.strip(encoded), "p")
    }

    // MARK: §22 — one state type, and the cheap call never blanks the reason

    @Test func testRosterAndDetailReturnTheSameTypeAndTheRosterKeepsTheReason() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")

        // The expensive call supplies the reason.
        let detail = try await run.status()
        XCTAssertEqual(detail.reason, "the turn is in flight")
        XCTAssertNotNil(detail.outputTail)

        // The cheap call carries no reason on the wire, and must not blank it.
        let roster = try await space.runs()
        let row: RunSnapshot = try XCTUnwrap(roster.first { $0.id == run.id })
        XCTAssertEqual(row.reason, detail.reason, "the roster blanked the explanation")
        XCTAssertTrue(row.reasonIsCarriedForward, "and it must say that it did")
        XCTAssertNil(row.outputTail, "the cheap call may omit output — and says so with nil")
    }

    // MARK: §2, §25 — a subscription, and one poll for the whole roster

    @Test func testRunStateIsAnAsyncSequenceThatFinishesWhenTheRunDoes() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        Task {
            try? await Task.sleep(for: .milliseconds(80))
            await backend.setRun(run.id.rawValue) {
                $0.status = "finished"
                $0.reason = "exit 0"
                $0.exitCode = 0
            }
        }
        var seen: [AgentState] = []
        for await snapshot in run.stateUpdates(every: .milliseconds(20)) {
            seen.append(snapshot.state)
        }
        XCTAssertEqual(seen.first, .running)
        XCTAssertEqual(seen.last, .finished, "the sequence must end on its own, with no invented timeout")
    }

    /// §25's whole point: the base cost of a roster tick does not grow with the
    /// roster.
    @Test func testARosterTickIsOnePollRegardlessOfRosterSize() async throws {
        let space = try await localSpace()
        for _ in 0..<9 { _ = try await space.startAgent(prompt: "x") }
        await backend.resetCalls()

        let roster = space.roster(pollingEvery: .milliseconds(20))
        var update: RosterUpdate?
        for await tick in roster.stream() {
            update = tick
            break
        }
        let tick = try XCTUnwrap(update)
        XCTAssertEqual(tick.runs.count, 9)
        XCTAssertEqual(tick.roundTrips, 1, "nine Bots cost one round trip, not ten")
        let statusCalls = await backend.callCount("agent_status")
        XCTAssertEqual(statusCalls, 0)
        XCTAssertEqual(tick.changed.count, 9, "the first tick reports everything as changed")
    }

    /// Output is only fetched for the runs a caller actually watches.
    @Test func testWatchingOneRunCostsExactlyOneExtraRoundTrip() async throws {
        let space = try await localSpace()
        var ids: [RunID] = []
        for _ in 0..<5 { ids.append(try await space.startAgent(prompt: "x").id) }
        let roster = space.roster(pollingEvery: .milliseconds(20), detailed: [ids[0]])
        for await tick in roster.stream() {
            XCTAssertEqual(tick.roundTrips, 2)
            XCTAssertNotNil(tick[ids[0]]?.outputTail, "the watched run must carry output")
            XCTAssertNil(tick[ids[1]]?.outputTail, "an unwatched run must not")
            break
        }
    }

    // MARK: §23 — truncation is reported

    @Test func testATruncatedTailSaysSo() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        await backend.setRun(run.id.rawValue) {
            $0.outputTail = (0..<50).map { "line \($0)" }.joined(separator: "\n")
        }
        let windowed = try await run.status(tail: 5)
        XCTAssertTrue(windowed.outputTruncated, "a full window must be reported as truncated")
        let whole = try await run.status(tail: 500)
        XCTAssertFalse(whole.outputTruncated)
    }

    // MARK: §24 — an outbox, with refusal still the default

    @Test func testQueueUntilIdleDeliversWithoutKillingTheTurnInFlight() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        Task {
            try? await Task.sleep(for: .milliseconds(60))
            await backend.setRun(run.id.rawValue) { $0.acceptsMessage = true }
        }
        let delivery = try await run.send("later, please",
                                          mode: .queueUntilIdle(timeout: .seconds(5)))
        XCTAssertTrue(delivery.accepted)
        XCTAssertNotNil(delivery.queuedFor, "a queued delivery must say it waited")
        // Refusal is still the default, and force is still deliberate.
        let calls = await backend.calls
        XCTAssertEqual(calls.filter { $0 == "agent_message" }.count, 2)
    }

    @Test func testRefusalIsStillTheDefault() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let refused = try await run.send("hi")
        XCTAssertFalse(refused.accepted)
    }

    @Test func testInterruptingIsADeliberateMode() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let forced = try await run.send("stop", mode: .interruptCurrentTurn)
        XCTAssertTrue(forced.accepted)
    }

    // MARK: §37 — a run can be resolved to its window

    @Test func testARunResolvesToTheWindowItsProcessOwns() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let window = try await run.window()
        XCTAssertEqual(window?.id, "target-aaa", "the join must be by pid, never by title")
    }

    /// And when the join cannot be made, the SDK says `nil` rather than
    /// guessing from a title that sixty other windows share.
    @Test func testAnUnjoinableRunReportsNoWindowRatherThanGuessing() async throws {
        let space = try await localSpace()
        await backend.setWindowsWithoutPIDs()
        let run = try await space.startAgent(prompt: "x")
        let noWindow = try await run.window()
        XCTAssertNil(noWindow)
    }

    // MARK: §10 — frames are not the operator's viewer

    @Test func testStreamEndpointIsSeparateFromOperatorPresentation() async throws {
        let space = try await localSpace()
        let endpoint = try await space.streamEndpoint()
        XCTAssertEqual(endpoint.host, "192.168.64.2")
        XCTAssertEqual(endpoint.token, "tkt-123")
        XCTAssertEqual(endpoint.port, 3211)
        XCTAssertEqual(endpoint.webSocketURL, "ws://192.168.64.2:3211/media?ticket=tkt-123")
        XCTAssertEqual(endpoint.mediaSessionID, "media-1")
        XCTAssertEqual(endpoint.frameSize, CGSize(width: 1200, height: 800))

        await backend.resetCalls()
        try await space.present(.pictureInPicture)
        let presentCalls = await backend.calls
        XCTAssertEqual(presentCalls, ["show_space_pip"],
                       "presenting must not be confused with streaming")
    }

    @Test func testStreamEndpointIsCachedUntilForced() async throws {
        let space = try await localSpace()
        _ = try await space.streamEndpoint()
        await backend.resetCalls()
        _ = try await space.streamEndpoint()
        let cachedCalls = await backend.callCount("stream_endpoint")
        XCTAssertEqual(cachedCalls, 0)
        _ = try await space.streamEndpoint(forceRefresh: true)
        let refreshedCalls = await backend.callCount("stream_endpoint")
        XCTAssertEqual(refreshedCalls, 1,
                       "a forced refresh must mint a fresh ticket")
    }

    // MARK: §30 — safe to call from the main actor

    @MainActor
    @Test func testEverySDKCallIsReachableFromTheMainActor() async throws {
        let space = try await connection.attach(to: "local:cua-space-test")
        let run = try await space.startAgent(prompt: "x")
        _ = try await run.status()
        _ = try await space.runs()
        _ = try await space.windows()
        _ = try await run.delete()
    }

    // MARK: helpers

    private func temporaryFile(_ contents: String, named: String = "f.txt") throws -> URL {
        let dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("cuaspaces-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(named)
        try contents.write(to: url, atomically: true, encoding: .utf8)
        // Temp directories are reclaimed by the OS; nothing here is large.
        return url
    }
}

extension FakeSpacesBackend {
    func addSpace(_ row: [String: JSONValue]) { spaces.append(row) }
    func setFailNext(_ tool: String, _ message: String) { failNext[tool] = message }
    func setListSpacesAnswersProse(_ value: Bool) { listSpacesAnswersProse = value }
    func setWindowsWithoutPIDs() {
        windows = windows.map { row in
            var copy = row
            copy["pid"] = nil
            return copy
        }
    }
}
