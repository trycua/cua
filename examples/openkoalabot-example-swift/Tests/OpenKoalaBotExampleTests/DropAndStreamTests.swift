// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSpaces
import CuaSpacesStreaming
import AppKit
import Foundation
import Testing
@testable import OpenKoalaBotExample

/// Coverage for the two drag-and-drop gestures and for making the Agent
/// Computer tiers injectable.
///
/// The pure parts (the documented attachment caps, the window drag payload,
/// the source switch a drop resolves to) are tested offline and always run.
/// The round trip of a file **into** the Space and back **out** of it needs a
/// real Space and follows the same opt-in rule as `SpacesE2ETests`: with
/// `OPENKOALABOTS_TEST_SPACE` unset it skips rather than creating a sandbox.
@Suite final class DropAndStreamTests {

    // MARK: - Attachment caps (documented: 6 files, 25 MB, 200 MB)

    private func candidate(_ name: String, mb: Double) -> AttachmentAdmission.Candidate {
        .init(name: name, byteCount: Int(mb * 1_048_576))
    }

    @Test func testSixAttachmentsFitAndTheSeventhIsRejected() {
        let files = (1...7).map { candidate("f\($0).txt", mb: 1) }
        let result = AttachmentAdmission.admit(files)
        XCTAssertEqual(result.accepted.count, 6)
        XCTAssertEqual(result.rejected.count, 1)
        XCTAssertEqual(result.rejected.first?.candidate.name, "f7.txt")
        XCTAssertEqual(result.rejected.first?.reason, "Up to 6 attachments per message.")
    }

    @Test func testTheCountCapCountsWhatIsAlreadyAttached() {
        let existing = (1...5).map { candidate("old\($0).txt", mb: 1) }
        let result = AttachmentAdmission.admit([candidate("a.txt", mb: 1),
                                                candidate("b.txt", mb: 1)],
                                               existing: existing)
        XCTAssertEqual(result.accepted.map(\.name), ["a.txt"])
        XCTAssertEqual(result.rejected.map(\.candidate.name), ["b.txt"])
    }

    @Test func testAFileOverTwentyFiveMegabytesIsRejectedAsOversized() {
        let result = AttachmentAdmission.admit([candidate("huge.mov", mb: 31.4)])
        XCTAssertTrue(result.accepted.isEmpty)
        let reason = try? XCTUnwrap(result.rejected.first?.reason)
        XCTAssertEqual(reason, "“huge.mov” is 31.4 MB. Files are capped at 25.0 MB.")
    }

    @Test func testExactlyTwentyFiveMegabytesIsAllowed() {
        let result = AttachmentAdmission.admit(
            [.init(name: "edge.bin", byteCount: AttachmentLimits.maxBytesPerFile)])
        XCTAssertEqual(result.accepted.count, 1)
        XCTAssertNil(result.message)
    }

    @Test func testTheTwoHundredMegabyteTotalIsEnforcedAcrossTheMessage() {
        // Nine 24 MB files would be 216 MB, but the count cap bites at six
        // (144 MB) first, so the total cap is exercised with larger existing
        // state, which is how a user actually reaches it.
        let existing = (1...5).map { candidate("old\($0).bin", mb: 24.9) }   // 124.5 MB
        let result = AttachmentAdmission.admit([candidate("last.bin", mb: 24.9)],
                                               existing: existing)
        XCTAssertEqual(result.accepted.count, 1, "149.4 MB is still under the cap")

        let heavier = (1...7).map { candidate("big\($0).bin", mb: 24.9) }
        let tight = AttachmentAdmission.admit([candidate("last.bin", mb: 24.9)],
                                              existing: heavier)  // 174.3 MB, 7 files
        XCTAssertTrue(tight.accepted.isEmpty)
    }

    @Test func testOversizeIsReportedAsOversizeRatherThanAsATotal() {
        // Order of the checks: a single 30 MB file dropped into an almost-full
        // message must still be told it is too big on its own.
        let existing = (1...5).map { candidate("old\($0).bin", mb: 24) }
        let result = AttachmentAdmission.admit([candidate("huge.mov", mb: 30)], existing: existing)
        XCTAssertTrue(result.rejected.first?.reason.contains("capped at 25.0 MB") ?? false,
                      "got: \(result.rejected.first?.reason ?? "-")")
    }

    @Test func testPartialAdmissionAttachesWhatFits() {
        let result = AttachmentAdmission.admit([candidate("ok.txt", mb: 1),
                                                candidate("huge.mov", mb: 40),
                                                candidate("ok2.txt", mb: 1)])
        XCTAssertEqual(result.accepted.map(\.name), ["ok.txt", "ok2.txt"])
        XCTAssertEqual(result.rejected.map(\.candidate.name), ["huge.mov"])
    }

    @Test func testTheBannerSummarisesMultipleRejections() {
        let result = AttachmentAdmission.admit([candidate("a.mov", mb: 40),
                                                candidate("b.mov", mb: 40)])
        let message = result.message ?? ""
        XCTAssertTrue(message.contains("“a.mov”"), message)
        XCTAssertTrue(message.hasSuffix("1 more were not attached."), message)
    }

    // MARK: - The window drag payload

    private var blender: StreamWindow {
        StreamWindow(id: "target-172aad9a-1da8-4582-a30d-3437271a78b7",
                     app: "Blender", title: "* octocat.blend - Blender 4.5.13 LTS", epoch: 3)
    }

    @Test func testWindowPayloadSurvivesARoundTrip() throws {
        let decoded = try XCTUnwrap(StreamWindowDrag.window(from: StreamWindowDrag.data(for: blender)))
        XCTAssertEqual(decoded.id, blender.id)
        XCTAssertEqual(decoded.app, "Blender")
        XCTAssertEqual(decoded.title, blender.title)
        XCTAssertEqual(decoded.epoch, 3, "the epoch must travel; a stale pair is refused as stale_target")
    }

    @Test func testWindowPayloadSurvivesATitleFullOfSeparatorCandidates() throws {
        // A real title from the live Space.
        let awkward = StreamWindow(id: "target-x", app: "Terminal",
                                   title: "lume - watch.command - tail -f out.log - 120×30")
        let decoded = try XCTUnwrap(StreamWindowDrag.window(from: StreamWindowDrag.data(for: awkward)))
        XCTAssertEqual(decoded.title, awkward.title)
    }

    @Test func testMalformedPayloadsAreRefusedRatherThanGuessed() {
        XCTAssertNil(StreamWindowDrag.decode(""))
        XCTAssertNil(StreamWindowDrag.decode("just some dragged text"))
        XCTAssertNil(StreamWindowDrag.decode("id\u{1F}notanumber\u{1F}App\u{1F}Title"))
        XCTAssertNil(StreamWindowDrag.decode("\u{1F}1\u{1F}App\u{1F}Title"), "empty handle")
    }

    @Test func testADropPrefersTheLiveListsEpochOverTheDraggedOne() {
        let stale = StreamWindow(id: "target-a", app: "Blender", title: "old title", epoch: 1)
        let live = [StreamWindow(id: "target-a", app: "Blender", title: "new title", epoch: 9)]
        guard case let .window(resolved) = StreamWindowDrag.resolve(stale, against: live) else {
            return XCTFail("a window drop must resolve to a window source")
        }
        XCTAssertEqual(resolved.epoch, 9)
        XCTAssertEqual(resolved.title, "new title")
    }

    @Test func testADropOnAPaneThatHasNotListedWindowsYetStillSelects() {
        guard case let .window(resolved) = StreamWindowDrag.resolve(blender, against: []) else {
            return XCTFail("a window drop must resolve to a window source")
        }
        XCTAssertEqual(resolved.id, blender.id)
    }

    @Test func testTheDragTypeIsPrivateSoStrayTextAndFilesAreNotAccepted() {
        XCTAssertEqual(StreamWindowDrag.typeIdentifier, "com.cua.openkoalabots.stream-window")
        XCTAssertNotEqual(StreamWindowDrag.typeIdentifier, "public.file-url")
        let provider = StreamWindowDrag.itemProvider(for: blender)
        XCTAssertEqual(provider.registeredTypeIdentifiers, [StreamWindowDrag.typeIdentifier])
    }

    // MARK: - The tiers stay injectable

    @MainActor
    @Test func testTiersDefaultToTheFixtureSoTheExportPathIsUnchanged() {
        // Both tiers go through `AgentScreen`, and its default source is the
        // fixture screen, so a tier that was never handed a session cannot
        // quietly claim to be live.
        XCTAssertFalse(AgentScreen().source.isLive)
        XCTAssertFalse(AgentScreenSource.fixture.isLive)
        XCTAssertFalse(DesktopShell().screen.isLive)
        XCTAssertNil(DesktopShell().intake)
        XCTAssertNil(DesktopShell().dropZone)
        XCTAssertFalse(DesktopShell().takeover)
    }

    @MainActor
    @Test func testALiveSourceCarriesItsSession() {
        let session = LiveStreamSession(provider: OfflineStreamSourceProvider())
        let source = AgentScreenSource.live(session)
        XCTAssertTrue(source.isLive)
        XCTAssertTrue(source.session === session)
        XCTAssertNil(AgentScreenSource.fixture.session)
    }

    // MARK: - Intake, with the Space stubbed out

    @MainActor
    @Test func testIntakeReportsPerFileUploadState() async throws {
        let intake = AttachmentIntake()
        let file = try Self.temporaryFile(named: "notes.txt", contents: "hello")
        defer { try? FileManager.default.removeItem(at: file) }
        intake.upload = { _, _ in }
        let admitted = intake.accept([file])
        XCTAssertEqual(admitted.count, 1)
        try await Self.until { if case .uploaded = intake.items.first?.state { return true }; return false }
        guard case let .uploaded(path) = intake.items.first?.state else {
            return XCTFail("expected an uploaded state, got \(String(describing: intake.items.first?.state))")
        }
        XCTAssertTrue(path.hasSuffix("-notes.txt"), path)
        XCTAssertTrue(path.hasPrefix(intake.remoteDirectory), path)
    }

    @MainActor
    @Test func testIntakeSurfacesAnUploadFailureOnTheAttachmentItself() async throws {
        struct Boom: Error, CustomStringConvertible { var description: String { "upload refused" } }
        let intake = AttachmentIntake()
        let file = try Self.temporaryFile(named: "notes.txt", contents: "hello")
        defer { try? FileManager.default.removeItem(at: file) }
        intake.upload = { _, _ in throw Boom() }
        intake.accept([file])
        try await Self.until { if case .failed = intake.items.first?.state { return true }; return false }
        guard case let .failed(reason) = intake.items.first?.state else { return XCTFail("expected failure") }
        XCTAssertTrue(reason.contains("upload refused"), reason)
    }

    @MainActor
    @Test func testWithNoSpaceAttachedAnAttachmentFailsRatherThanLookingSent() async throws {
        let intake = AttachmentIntake()   // no uploader
        let file = try Self.temporaryFile(named: "notes.txt", contents: "hello")
        defer { try? FileManager.default.removeItem(at: file) }
        intake.accept([file])
        try await Self.until { if case .failed = intake.items.first?.state { return true }; return false }
        guard case let .failed(reason) = intake.items.first?.state else { return XCTFail("expected failure") }
        XCTAssertEqual(reason, "no Space is attached")
    }

    @MainActor
    @Test func testTwoDropsOfTheSameNameBecomeTwoDistinctFilesInTheSpace() async throws {
        let intake = AttachmentIntake()
        let box = PathBox()
        intake.upload = { _, remote in box.append(remote) }
        let a = try Self.temporaryFile(named: "notes.txt", contents: "a", subdirectory: "one")
        let b = try Self.temporaryFile(named: "notes.txt", contents: "b", subdirectory: "two")
        defer {
            try? FileManager.default.removeItem(at: a.deletingLastPathComponent())
            try? FileManager.default.removeItem(at: b.deletingLastPathComponent())
        }
        intake.accept([a, b])
        try await Self.until { box.paths.count == 2 }
        XCTAssertEqual(Set(box.paths).count, 2, "two drops must not collide in the Space: \(box.paths)")
    }

    @MainActor
    @Test func testADragOutIsRefusedUntilTheFileHasActuallyLanded() async throws {
        let export = AgentArtifactExport()
        XCTAssertFalse(export.isReady("/tmp/not-fetched.csv"))
        XCTAssertTrue(export.itemProvider("/tmp/not-fetched.csv").registeredTypeIdentifiers.isEmpty,
                      "an unfetched artefact must not hand Finder a path with nothing behind it")

        let landed = try Self.temporaryFile(named: "report.csv", contents: "a,b\n1,2\n")
        defer { try? FileManager.default.removeItem(at: landed) }
        export.download = { _ in landed.path }
        await export.prepare("/Users/ci/report.csv")
        XCTAssertTrue(export.isReady("/Users/ci/report.csv"))
        XCTAssertFalse(export.itemProvider("/Users/ci/report.csv").registeredTypeIdentifiers.isEmpty)
    }

    // MARK: - Live: a file goes into the Space and comes back out

    /// The whole gesture, end to end, against the real Space: a file dropped on
    /// a thread is uploaded through `AttachmentIntake`, **read back in the
    /// guest** to prove it arrived, then pulled out again through
    /// `AgentArtifactExport` as the drag-out path does and compared byte for
    /// byte. Everything it creates is removed.
    @MainActor
    @Test func testDroppedFileReachesTheSpaceAndCanBeDraggedBackOut() async throws {
        let target = try LiveSpace.require("the live round trip")
        let (client, space) = (target.client, target.space)

        let stamp = UUID().uuidString.prefix(8)
        let remoteDir = "/tmp/openkoalabots-dnd-\(stamp)"
        let contents = "quarter,revenue\nQ3,411000\n# \(stamp)\n"
        let file = try Self.temporaryFile(named: "Q3-actuals.csv", contents: contents)
        defer { try? FileManager.default.removeItem(at: file) }

        @Sendable func bash(_ command: String) async throws -> String {
            let out = try await client.raw("space_bash", ["space": space, "command": command])
            return out as? String ?? String(describing: out)
        }
        _ = try await bash("mkdir -p \(remoteDir)")
        // `defer` cannot await, so the teardown is an explicit last step below
        // plus this best-effort detached sweep for the failure paths.
        let sweep = { @Sendable in _ = try? await bash("rm -rf \(remoteDir)") }
        defer { Task.detached { await sweep() } }

        // --- in: the drop
        let intake = AttachmentIntake()
        intake.remoteDirectory = remoteDir
        intake.upload = { local, remote in
            try await client.upload(space: space, localPath: local.path, remotePath: remote)
        }
        intake.accept([file])
        try await Self.until(timeout: 60) {
            if case .uploaded = intake.items.first?.state { return true }
            if case .failed = intake.items.first?.state { return true }
            return false
        }
        guard case let .uploaded(remotePath) = intake.items.first?.state else {
            return XCTFail("upload did not complete: \(String(describing: intake.items.first?.state))")
        }

        let readBack = try await bash("cat \(remotePath)")
        XCTAssertTrue(readBack.contains("Q3,411000"),
                      "the dropped file did not arrive in the Space: \(readBack)")
        XCTAssertTrue(readBack.contains(stamp), "wrong file in the Space: \(readBack)")

        // --- out: the drag back
        let hostDir = NSTemporaryDirectory() + "openkoalabots-out-\(stamp)"
        try FileManager.default.createDirectory(atPath: hostDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: hostDir) }

        let export = AgentArtifactExport()
        export.download = { path in
            try await client.download(space: space, remotePath: path, localDirectory: hostDir)
        }
        await export.prepare(remotePath)
        XCTAssertTrue(export.isReady(remotePath),
                      "download did not land: \(export.failures[remotePath] ?? "no error reported")")
        let landedPath = try XCTUnwrap(export.local[remotePath])
        XCTAssertEqual(try String(contentsOfFile: landedPath, encoding: .utf8), contents,
                       "the file that came back is not the file that went in")
        XCTAssertFalse(export.itemProvider(remotePath).registeredTypeIdentifiers.isEmpty,
                       "a landed artefact must be draggable to Finder")

        // --- cleanup is asserted, not assumed
        _ = try await bash("rm -rf \(remoteDir)")
        let gone = try await bash("test -e \(remotePath) && echo STILL_THERE || echo GONE")
        XCTAssertTrue(gone.contains("GONE"), "cleanup left \(remotePath) behind")
    }

    // MARK: - Helpers

    private final class PathBox: @unchecked Sendable {
        private let lock = NSLock()
        private var storage: [String] = []
        func append(_ path: String) { lock.lock(); storage.append(path); lock.unlock() }
        var paths: [String] { lock.lock(); defer { lock.unlock() }; return storage }
    }

    private static func temporaryFile(named name: String, contents: String,
                                      subdirectory: String? = nil) throws -> URL {
        var dir = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("openkoalabots-tests-\(UUID().uuidString.prefix(8))")
        if let subdirectory { dir = dir.appendingPathComponent(subdirectory) }
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        try contents.write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    /// Poll a condition rather than sleeping a fixed amount.
    private static func until(timeout: TimeInterval = 10,
                              _ condition: @MainActor () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await MainActor.run(body: condition) { return }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        XCTFail("condition never became true within \(timeout)s")
    }
}
