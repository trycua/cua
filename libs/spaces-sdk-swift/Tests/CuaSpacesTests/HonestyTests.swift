import Foundation
import Testing
@testable import CuaSpaces
@testable import CuaSpacesTranscript

/// The guarantees the honesty apparatus exists to make.
///
/// Each of these asserts that the SDK **declines to claim** something the
/// backend does not do. They are as much about what is absent as what is
/// present, which is why they are worth writing down.
@Suite(.serialized) final class HonestyTests {

    private var backend: FakeSpacesBackend!
    private var connection: SpacesConnection!

    init() async throws {
        backend = FakeSpacesBackend()
        connection = SpacesConnection(transport: backend)
    }

    private func localSpace() async throws -> Space {
        try await connection.attach(to: "local:cua-space-test")
    }

    // MARK: - Inferred events

    /// A scrollback-only harness can produce `.text`, `.raw`, `.stateChanged`
    /// and `.finished`. Nothing else, ever, without a classifier.
    @Test func testPlainEventsAreNeverInferredAndNeverRicherThanScrollback() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "say something")
        await backend.setOutput(of: run.id, to: "⏺ Bash(ls -la)\nhello there")
        await backend.finish(run.id)

        var kinds: [String] = []
        for await event in run.events(pollingEvery: .milliseconds(10)) {
            XCTAssertFalse(event.isInferred,
                           "CuaSpaces alone must not infer: \(event.kind)")
            switch event.kind {
            case .text: kinds.append("text")
            case .raw: kinds.append("raw")
            case .stateChanged: kinds.append("state")
            case .finished: kinds.append("finished")
            case .toolUse, .artifact, .question, .turnEnded:
                XCTFail("the SDK minted a \(event.kind) the agent never offered")
            }
        }
        XCTAssertTrue(kinds.contains("text"))
        XCTAssertTrue(kinds.contains("finished"))
    }

    /// The same line, through the opt-in module, becomes a tool card — and
    /// says that it is a guess.
    @Test func testTheOptInClassifierProducesRicherEventsAndLabelsThemInferred() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "say something")
        await backend.setOutput(of: run.id, to: "⏺ Bash(ls -la)\nhello there")
        await backend.finish(run.id)

        var sawInferredTool = false
        for await event in run.inferredEvents(pollingEvery: .milliseconds(10)) {
            if case let .toolUse(name, summary) = event.kind {
                XCTAssertTrue(event.isInferred, "an inferred event claimed to be published")
                XCTAssertEqual(name, "Bash")
                XCTAssertEqual(summary, "ls -la")
                sawInferredTool = true
            }
        }
        XCTAssertTrue(sawInferredTool, "the classifier saw nothing in a line it should read")
    }

    /// The conservative tier refuses to guess a question. A wrong `.question`
    /// puts a prompt in front of a person that the agent never asked.
    @Test func testTheConservativeClassifierRefusesToGuessAQuestion() {
        let classifier = ScrollbackClassifier(confidence: .conservative)
        XCTAssertNil(classifier.classify(line: "Should I delete the branch?", in: "run-1"))
        let eager = ScrollbackClassifier(confidence: .eager)
        guard case .question = eager.classify(line: "Should I delete the branch?", in: "run-1")
        else { return XCTFail("the eager tier should have guessed a question") }
    }

    // MARK: - Nothing pretends to be server-backed

    @Test func testTheSchedulerSaysItIsNotServerBacked() {
        XCTAssertFalse(Scheduler.isServerBacked,
                       "spaces_mcp.py has zero occurrences of schedule, cron or recurr")
        XCTAssertEqual(Scheduler.missedSlotPolicy, .collapseToOneFiring)
    }

    @Test func testTransferLimitsSayWhetherTheServerPublishedThem() async throws {
        let space = try await localSpace()
        let limits = try await space.files.limits()
        XCTAssertFalse(limits.isServerPublished,
                       "no tool publishes limits; MAX_TRANSFER_BYTES is a constant")
        XCTAssertEqual(limits.maxBytesPerFile, 25 * 1024 * 1024)
    }

    @Test func testApprovalsAreDeclaredUnenforcedAndApproveThrows() async throws {
        XCTAssertFalse(AgentRun.approvalsAreEnforced)
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        do {
            try await run.approve(.allowOnce)
            XCTFail("approve() cannot succeed while nothing enforces approvals")
        } catch let error as SpacesError {
            guard case .notImplementedYet = error else { return XCTFail("\(error)") }
        }
    }

    @Test func testStubbedAgentBackendsAreReportedNotDiscoveredByFailing() {
        XCTAssertTrue(AgentKind.claudeCode.isProductionReady)
        XCTAssertTrue(AgentKind.codex.isProductionReady)
        XCTAssertFalse(AgentKind("gemini-cli").isProductionReady)
    }

    @Test func testTheServerBackstopIsReservedAndRefusesUntilItExists() async throws {
        let space = try await localSpace()
        XCTAssertFalse(space.capabilities.serverBackstop)
        do {
            try await space.setIdleTimeout(.seconds(600))
            XCTFail("no backend ends an idle Space")
        } catch let error as SpacesError {
            guard case .notImplementedYet = error else { return XCTFail("\(error)") }
        }
        // The request field exists now so gaining it later is not a break.
        var request = AgentStartRequest(prompt: "x")
        request.timeout = .seconds(60)
        XCTAssertEqual(request.timeout, .seconds(60))
    }

    // MARK: - Truncation stays expressible

    /// A window that slid past unseen output is loss, and the SDK says so
    /// rather than stitching the two halves into a transcript that reads as
    /// though nothing was missing.
    @Test func testASlidWindowIsReportedAsLossRatherThanStitchedSilently() {
        var window = TranscriptWindow()
        window.ingest("one\ntwo\nthree")
        XCTAssertFalse(window.lostBefore)
        // An overlapping window: nothing lost.
        window.ingest("two\nthree\nfour")
        XCTAssertFalse(window.lostBefore)
        XCTAssertEqual(window.lines, ["one", "two", "three", "four"])
        // A window with no overlap at all: output existed that nobody saw.
        window.ingest("ninety\nninety-one")
        XCTAssertTrue(window.lostBefore, "a non-overlapping window is loss")
    }

    @Test func testANewlineSurvivesANSIStrippingOnScalarsNotCharacters() {
        let stripped = TranscriptWindow.stripANSI("\u{1B}[32mgreen\u{1B}[0m\r\nnext")
        XCTAssertEqual(stripped, "green\nnext")
        XCTAssertEqual(TranscriptWindow.split(stripped), ["green", "next"])
    }

    @Test func testOutputPagesCarryACursorAndSayWhenTheyTruncated() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        await backend.setOutput(of: run.id, to: "a\nb\nc")
        let first = try await run.output(limit: 2)
        XCTAssertEqual(first.events.map(\.text), ["a", "b"])
        XCTAssertFalse(first.reachedEnd)
        let second = try await run.output(from: first.next)
        XCTAssertEqual(second.events.map(\.text), ["c"])
        XCTAssertTrue(second.reachedEnd)
    }

    // MARK: - Queueing declares its lifetime

    @Test func testAQueuedMessageDeclaresThatItDiesWithTheProcess() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        await backend.setAcceptsMessage(run.id, true)
        let delivered = try await run.send("hello",
                                           mode: .queue(timeout: .seconds(2),
                                                        queuedUntil: .processExit))
        XCTAssertTrue(delivered.accepted)
        let held = QueuedMessage(run: run.id, text: "later")
        XCTAssertEqual(held.queuedUntil, .processExit)
        XCTAssertEqual(held.queuedUntil.description, "until this process exits")
    }

    @Test func testRefusalIsStillTheDefaultMode() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let refused = try await run.send("mid-turn")
        XCTAssertFalse(refused.accepted)
        XCTAssertFalse(refused.reason.isEmpty)
    }

    // MARK: - The teleport gate

    @Test func testOnlyAManifestCanMintAnApprovalAndSensitiveEntriesThrow() async throws {
        let space = try await localSpace()
        let chrome = TeleportableApp(id: "chrome", displayName: "Chrome",
                                     isInstalledOnHost: true)
        let manifest = TeleportManifest(app: chrome, scope: .full, entries: [
            .init(relativePath: "Default/Cookies", byteCount: 400,
                  isSensitive: true, isDefault: true),
            .init(relativePath: "Default/Preferences", byteCount: 90,
                  isSensitive: false, isDefault: true),
        ])
        // Credentials cannot travel on a defaulted parameter.
        XCTAssertThrowsError(try manifest.approving(manifest.entries, into: space)) { error in
            guard case SpacesError.teleportRefused = error else {
                return XCTFail("\(error)")
            }
        }
        // Non-sensitive entries need no acknowledgement.
        let safe = try manifest.approving(
            manifest.entries.filter { !$0.isSensitive }, into: space)
        XCTAssertEqual(safe.approvedPaths, ["Default/Preferences"])
        // And an entry that is not in the manifest cannot be smuggled in.
        XCTAssertThrowsError(try manifest.approving(
            [.init(relativePath: "../../.ssh/id_rsa", byteCount: 1,
                   isSensitive: false, isDefault: false)],
            into: space))
    }

    /// The Local branch is the **first line** of the backend's `teleport_app`,
    /// and the SDK used to declare the capability `false` and refuse.
    @Test func testTeleportIsAvailableOnLocalSpaces() async throws {
        XCTAssertTrue(SpaceProvider.local.capabilities.teleport,
                      "teleport_app's first line is the Local branch")
        XCTAssertTrue(SpaceProvider.local.capabilities.features.contains(.sessions))
    }

    // MARK: - A budget that is honoured

    @Test func testWaitHonoursItsBudgetRatherThanTheBackoff() async throws {
        let space = try await localSpace()
        let run = try await space.startAgent(prompt: "x")
        let started = ContinuousClock.now
        do {
            _ = try await run.wait(upTo: .milliseconds(300), tail: 0) { _ in false }
            XCTFail("a condition that is never met must time out")
        } catch let error as SpacesError {
            guard case let .timedOut(waitingFor, after) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertEqual(after, .milliseconds(300))
            XCTAssertTrue("\(waitingFor)".contains(run.id.rawValue),
                          "the type should say what was awaited")
        }
        let elapsed = ContinuousClock.now - started
        // The old implementation could return at 8x the poll interval.
        XCTAssertLessThan(elapsed, .seconds(2), "the budget was not honoured: \(elapsed)")
    }

    // MARK: - fileExists

    @Test func testFileExistsExpandsTildeAndDoesNotSubstringMatch() async throws {
        let space = try await localSpace()
        // A path whose *name* contains the old sentinel. A substring test for
        // "yes" cannot tell this apart from an existing file.
        let absent = try await space.fileExists("~/yes-please.txt")
        XCTAssertFalse(absent)
        _ = try await space.bash("mkdir -p '/Users/lume/expanded'")
        let present = try await space.fileExists("~/expanded")
        XCTAssertTrue(present,
                      "a quoted ~ does not expand; the SDK must expand it itself")
    }

    // MARK: - Roster policy

    @Test func testOneLiveAndTheRestRotatingIsTheSameExpressionAsAllLive() async throws {
        let space = try await localSpace()
        let a = try await space.startAgent(prompt: "a")
        let b = try await space.startAgent(prompt: "b")
        let c = try await space.startAgent(prompt: "c")

        await backend.resetCalls()
        var policy = RosterPolicy.focused(a.id)
        policy.interval = .milliseconds(20)
        var ticks = 0
        var rotatedSeen: Set<RunID> = []
        for await update in space.agents.live(policy) {
            rotatedSeen.formUnion(update.rotated)
            XCTAssertFalse(update.rotated.contains(a.id),
                           "a watched run is not a rotated one")
            ticks += 1
            if ticks == 3 { break }
        }
        XCTAssertTrue(rotatedSeen.isSubset(of: [b.id, c.id]))
        XCTAssertFalse(rotatedSeen.isEmpty, "nothing rotated; 'everyone, eventually' is unmet")

        // The other half of the same expression.
        let all = RosterPolicy.allLive([a.id, b.id, c.id])
        XCTAssertEqual(all.rotateUnwatched, 0)
        XCTAssertEqual(all.watched.count, 3)
    }

    /// A roster is "conversations I have had", not "work happening in the
    /// Space", unless a caller says otherwise.
    @Test func testForeignRunsAreNotAdoptedByDefault() async throws {
        let space = try await localSpace()
        await backend.addForeignRun("run-someone-else")
        let mine = try await space.startAgent(prompt: "mine")

        var policy = RosterPolicy.default
        policy.interval = .milliseconds(20)
        policy.rotateUnwatched = 0
        for await update in space.agents.live(policy) {
            XCTAssertEqual(update.runs.map(\.id), [mine.id])
            break
        }
        policy.includesForeignRuns = true
        for await update in space.agents.live(policy) {
            XCTAssertTrue(update.runs.contains { $0.id == "run-someone-else" })
            break
        }
    }

    // MARK: - A reconnect handle that is hydrated

    @Test func testARecoveredHandleCarriesItsAgentAndMetadata() async throws {
        let space = try await localSpace()
        let started = try await space.startAgent(
            prompt: "triage", metadata: ["koalabots.bot": "bot-7"])
        // The cheap, synchronous handle is honestly empty.
        XCTAssertEqual(space.run(started.id).agent, "")
        // The hydrated one is contractually not.
        let recovered = try await space.agents.agent(started.id)
        XCTAssertEqual(recovered.agent, "claude-code")
        XCTAssertEqual(recovered.metadata["koalabots.bot"], "bot-7")
    }
}
