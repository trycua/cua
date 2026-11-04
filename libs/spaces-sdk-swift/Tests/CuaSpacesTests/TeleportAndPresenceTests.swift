import CoreGraphics
import Foundation
import Testing
@testable import CuaSpaces

/// The three things this pass moved: teleport into the SDK, presence into the
/// streaming layer, and transfer policy plus agent-output parsing out of the
/// sample app.
@Suite(.serialized) final class TeleportTests {

    private func localSpace() async throws -> (Space, FakeSpacesBackend) {
        let backend = FakeSpacesBackend()
        let connection = SpacesConnection(transport: backend)
        return (try await connection.attach(to: "local:cua-space-test"), backend)
    }

    private func cloudSpace() async throws -> Space {
        let connection = SpacesConnection(transport: FakeSpacesBackend())
        return try await connection.attach(to: "cloud:space-abc")
    }

    /// The bug this pass exists to fix. `ProviderCapabilities.teleport` said
    /// `false` for Local on the strength of a stale error message attached to a
    /// helper `teleport_app` returns *before* reaching — while the MCP's
    /// `teleport_app` opens by routing Local Spaces to `_local_teleport_app`.
    @Test func testLocalSpacesDeclareTeleport() async throws {
        let (local, _) = try await localSpace()
        XCTAssertTrue(local.capabilities.teleport,
                      "Local teleport is implemented; declaring it unavailable makes the SDK "
                      + "refuse a call the server would have served")
        let cloud = try await cloudSpace()
        XCTAssertTrue(cloud.capabilities.teleport)
        XCTAssertFalse(local.capabilities.notes.contains { $0.contains("cloud-only") },
                       "the stale cloud-only note must not survive")
    }

    @Test func testManifestDecodesTheConsentFields() async throws {
        let (space, _) = try await localSpace()
        let manifest = try await space.sessions.manifest(for: .claudeCode)

        XCTAssertEqual(manifest.items.count, 3)
        let login = try XCTUnwrap(manifest.item("claude/.credentials.json"))
        XCTAssertTrue(login.isSensitive)
        XCTAssertTrue(login.isCheckedByDefault)

        // The expensive one: sensitive, nearly a gigabyte, and deliberately
        // unchecked. A caller that treats the manifest as a file list sends it.
        let conversations = try XCTUnwrap(manifest.item("claude/projects/"))
        XCTAssertTrue(conversations.isSensitive)
        XCTAssertFalse(conversations.isCheckedByDefault)
        XCTAssertEqual(conversations.count, 14)
        XCTAssertEqual(conversations.countNoun, "projects")

        XCTAssertEqual(manifest.defaultSelection.map(\.relativePath),
                       ["claude/.credentials.json", "claude/.claude.json"])
        XCTAssertEqual(manifest.loginOnlySelection.map(\.relativePath),
                       ["claude/.credentials.json"])
        XCTAssertFalse(manifest.notes.isEmpty, "the server's own warnings must survive")
    }

    /// No selection means the *server's* default set, which is the rule the MCP
    /// enforces — not "everything", and not an empty transfer.
    @Test func testTeleportWithoutASelectionSendsNoIncludeAndLetsTheServerDecide() async throws {
        let (space, backend) = try await localSpace()
        let result = try await space.sessions.sendServerDefault(
            for: .claudeCode, acknowledgingSensitiveItems: true)
        XCTAssertEqual(result.method, "import_session")
        XCTAssertEqual(result.space, space.id)
        XCTAssertTrue(result.included.isEmpty)
        let sent = await backend.lastTeleportInclude
        XCTAssertTrue(sent.isEmpty)
    }

    @Test func testTeleportLoginSendsOnlyTheCredentials() async throws {
        let (space, backend) = try await localSpace()
        let result = try await space.sessions.sendLoginOnly(
            for: .claudeCode, acknowledgingSensitiveItems: true)
        XCTAssertEqual(result.included, ["claude/.credentials.json"])
        let sent = await backend.lastTeleportInclude
        XCTAssertEqual(sent, ["claude/.credentials.json"],
                       "teleportLogin must not carry the 900 MB transcript item")
        let app = await backend.lastTeleportApp
        XCTAssertEqual(app, "claude-code")
    }

    /// A provider that cannot teleport refuses on the language's error channel
    /// rather than returning prose — `FRICTION.md` §3 and §6.
    @Test func testAProviderWithoutTeleportThrows() async throws {
        let connection = SpacesConnection(transport: FakeSpacesBackend())
        let space = try await connection.attach(to: "cloud:space-abc")
        let stripped = SpaceInfo(id: space.id, provider: .unknown,
                                 operatingSystem: space.info.operatingSystem,
                                 state: space.info.state, rawPhase: space.info.rawPhase,
                                 ipAddress: space.info.ipAddress)
        let unsupported = Space(info: stripped, connection: connection)
        do {
            _ = try await unsupported.sessions.sendServerDefault(
                for: .chrome, acknowledgingSensitiveItems: true)
            XCTFail("teleport on a provider without it must throw")
        } catch let error as SpacesError {
            guard case .unsupportedByProvider(let tool, _, _) = error else {
                return XCTFail("\(error)")
            }
            XCTAssertEqual(tool, "teleport_app")
        }
    }
}

/// Transfer policy and agent-output parsing, now that both live in the SDK.
@Suite(.serialized) final class MovedFromTheAppTests {

    /// Dropping seven files on a six-attachment surface attaches six and says
    /// why the seventh did not go. `check` is the all-or-nothing batch verdict;
    /// `admit` is the interactive one, and they are different questions.
    @Test func testAdmitAttachesWhatFits() {
        let limits = TransferLimits(maxFileCount: 6, maxBytesPerFile: 1_000,
                                    maxBytesPerBatch: 10_000)
        let candidates = (1...7).map { TransferLimits.Candidate(name: "f\($0)", byteCount: 100) }
        let admission = limits.admit(candidates)
        XCTAssertEqual(admission.accepted.count, 6)
        XCTAssertEqual(admission.rejected.count, 1)
        guard case .tooManyFiles = admission.rejected[0].violation else {
            return XCTFail("\(admission.rejected)")
        }
    }

    /// Order is load-bearing: one oversized file must be named as oversized,
    /// not as "the batch is too big", which would point the user at the wrong
    /// file to remove.
    @Test func testPerFileSizeIsTestedBeforeTheRunningTotal() {
        let limits = TransferLimits(maxFileCount: 10, maxBytesPerFile: 100,
                                    maxBytesPerBatch: 150)
        let admission = limits.admit([.init(name: "huge", byteCount: 500),
                                      .init(name: "fine", byteCount: 50)])
        XCTAssertEqual(admission.accepted.map(\.name), ["fine"])
        guard case let .fileTooLarge(name, _, _) = admission.rejected[0].violation else {
            return XCTFail("\(admission.rejected)")
        }
        XCTAssertEqual(name, "huge")
    }

    @Test func testAdmitCarriesCountAndTotalAcrossDrops() {
        let limits = TransferLimits(maxFileCount: 2, maxBytesPerFile: 1_000,
                                    maxBytesPerBatch: 1_000)
        let admission = limits.admit([.init(name: "c", byteCount: 10)],
                                     existing: [.init(name: "a", byteCount: 10),
                                                .init(name: "b", byteCount: 10)])
        XCTAssertTrue(admission.accepted.isEmpty)
    }

    /// §19/§20: the harness publishes one undifferentiated text stream, so the
    /// parser is the SDK's. ANSI over unicode *scalars*, because Swift treats
    /// CR-LF as a single grapheme cluster and a Character-level filter silently
    /// leaves every `\r\n` intact while looking like it worked.
    @Test func testANSIStrippingHandlesCRLF() {
        let stripped = AgentOutputParser.strippingANSI("a\u{1B}[31mred\u{1B}[0m\r\nb")
        XCTAssertEqual(stripped, "ared\nb")
    }

    @Test func testSegmentsClassifyTheShapes() {
        let output = """
        Opening Safari

        https://example.com/report.pdf

        /Users/lume/out/deck.pptx

        Drafted email:
          Hi there
          Thanks

        just some prose
        """
        let segments = AgentOutputParser.segments(from: output)
        guard case .activity = segments[0] else { return XCTFail("\(segments[0])") }
        guard case let .link(_, host, kind) = segments[1] else { return XCTFail("\(segments[1])") }
        XCTAssertEqual(host, "example.com")
        XCTAssertEqual(kind, .pdf)
        guard case let .file(_, name, _, fileKind) = segments[2] else {
            return XCTFail("\(segments[2])")
        }
        XCTAssertEqual(name, "deck.pptx")
        XCTAssertEqual(fileKind, .slides)
        guard case let .titled(title, lines) = segments[3] else { return XCTFail("\(segments[3])") }
        XCTAssertEqual(title, "Drafted email")
        XCTAssertEqual(lines, ["Hi there", "Thanks"])
        guard case .prose = segments[4] else { return XCTFail("\(segments[4])") }
    }

    /// The letters are ignored; position decides them. A Bot that writes
    /// `A) … C)` still yields two options.
    @Test func testChoiceBlockIgnoresTheEmittedLetters() {
        let block = AgentOutputParser.choiceBlock(in: """
        [[choices: Which one?]]
        A) first
        C) second
        [[/choices]]
        """)
        XCTAssertEqual(block?.heading, "Which one?")
        XCTAssertEqual(block?.options, ["first", "second"])
    }

    @Test func testAOneOptionBlockIsNotACard() {
        XCTAssertNil(AgentOutputParser.choiceBlock(in: "[[choices: x]]\nA) only\n[[/choices]]"),
                     "a card with one button is a wrong guess, and a wrong guess is worse "
                     + "than a plain bubble")
    }

    @Test func testAcknowledgementsAreSingleLine() {
        XCTAssertTrue(AgentOutputParser.isAcknowledgement("Done"))
        XCTAssertFalse(AgentOutputParser.isAcknowledgement("Done\nand more"))
    }
}
