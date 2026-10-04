import Foundation
import Testing
@testable import CuaSpaces

/// The overlay against the **real** Spaces runtime: the cua SDK's `cua-spaces`
/// (embedded, and through a cua daemon), talking to the in-process
/// cua-spacesd core that `cua-test-fixtures` serves (temp guest HOME/PATH,
/// temp Downloads and teleport home, fake driver tools; teleport reads a
/// synthetic Firefox profile under a temp home through a side-effect-free
/// host). The daemon is the fixture's cua daemon server core, whose Keyvault
/// is test-only (a passphrase vault in a temp home, a fake presence gate,
/// the fixture profile imported). This is what keeps `FakeSpacesBackend`
/// honest: every shape the fake answers with is parsed here from the real
/// server too.
///
/// Skips (passes vacuously) when `libs/cua/target/*/cua-test-fixtures` is not
/// built, and fails when it is stale (built from another commit). Nothing
/// touches the real home, apps or keychain.
struct Fixtures {
    let process: Process
    let stdin: Pipe
    let fields: [String: String]

    static var cuaRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("cua")
    }

    static func binary(_ env: String, _ name: String) -> URL? {
        if let p = ProcessInfo.processInfo.environment[env] { return URL(fileURLWithPath: p) }
        for profile in ["debug", "release"] {
            let u = cuaRoot.appendingPathComponent("target/\(profile)/\(name)")
            if FileManager.default.isExecutableFile(atPath: u.path) { return u }
        }
        return nil
    }

    /// `CUA_REQUIRE_FIXTURES=1` (CI) turns a missing binary into a failure
    /// instead of a vacuous pass.
    static func required(_ what: String) {
        if ProcessInfo.processInfo.environment["CUA_REQUIRE_FIXTURES"] == "1" {
            Issue.record("\(what) is not built")
        }
    }

    static func start() throws -> Fixtures? {
        guard let bin = binary("CUA_TEST_FIXTURES", "cua-test-fixtures") else {
            required("cua-test-fixtures")
            return nil
        }
        let p = Process()
        p.executableURL = bin
        let input = Pipe(), output = Pipe()
        p.standardInput = input
        p.standardOutput = output
        try p.run()
        var data = Data()
        for _ in 0..<1000 {  // bounded: one JSON line
            let chunk = output.fileHandleForReading.availableData
            if chunk.isEmpty { break }
            data.append(chunk)
            if data.contains(UInt8(ascii: "\n")) { break }
        }
        let line = data.split(separator: UInt8(ascii: "\n")).first ?? Data()
        guard let obj = (try? JSONSerialization.jsonObject(with: Data(line))) as? [String: Any] else {
            // It refuses to serve when stale; its stderr (above) says why.
            p.waitUntilExit()
            throw FixturesUnavailable(status: p.terminationStatus)
        }
        return Fixtures(process: p, stdin: input, fields: obj.compactMapValues { $0 as? String })
    }

    struct FixturesUnavailable: Error, CustomStringConvertible {
        let status: Int32
        var description: String {
            "cua-test-fixtures printed no endpoints (exit \(status)); if it is stale, rebuild it "
                + "with libs/cua/scripts/build-test-fixtures.sh"
        }
    }

    /// The user approves the waiting Keyvault request in Cua.
    func approveTeleport() {
        stdin.fileHandleForWriting.write(Data("approve\n".utf8))
    }

    func stop() {
        try? stdin.fileHandleForWriting.close()
        process.waitUntilExit()
    }
}

@Suite(.serialized) final class CuaBackedTests {

    private func exercise(_ connection: SpacesConnection, _ fx: Fixtures, work: URL,
                          keyvault: Bool) async throws {
        let f = fx.fields
        // Registry: add by URL, attach, list — never creates.
        let space = try await connection.add(url: f["spaces_url"]!, token: f["spaces_token"],
                                             name: "swift-overlay")
        XCTAssertEqual(space.provider, .direct)
        XCTAssertTrue(space.id.rawValue.hasPrefix("direct:"), space.id.rawValue)
        XCTAssertEqual(space.home, f["spaces_guest_home"], "home comes from the guest, not a table")
        let listed = try await connection.spaces()
        XCTAssertEqual(listed.map(\.id), [space.id])
        XCTAssertTrue(listed[0].isReady)

        // §1, live: many calls in a row, each answered with its own shape.
        for i in 0..<12 {
            let out = try await space.bash("echo step-\(i)")
            XCTAssertEqual(out, "step-\(i)\n")
            let count = try await connection.spaces().count
            XCTAssertEqual(count, 1)
        }

        // Files: literal text, existence, drop into Downloads, upload, download.
        let file = try await space.write("from the overlay", to: "~/overlay/note.txt")
        XCTAssertEqual(file.path, f["spaces_guest_home"]! + "/overlay/note.txt")
        let exists = try await space.fileExists("~/overlay/note.txt")
        XCTAssertTrue(exists)
        let missing = try await space.fileExists("~/overlay/nope.txt")
        XCTAssertFalse(missing)
        let local = work.appendingPathComponent("drop.txt")
        try "dropped".write(to: local, atomically: true, encoding: .utf8)
        let sent = try await space.sendFile(local, intoDownloads: "overlay-in")
        XCTAssertEqual(sent.count, 1)
        XCTAssertEqual(try String(contentsOfFile: f["spaces_downloads"]! + "/overlay-in/drop.txt",
                                  encoding: .utf8), "dropped")
        let uploaded = try await space.upload(local, to: .exactPath(f["spaces_guest_home"]! + "/up.txt"))
        XCTAssertEqual(uploaded.path, f["spaces_guest_home"]! + "/up.txt")
        let back = work.appendingPathComponent("back")
        let landed = try await space.download("~/up.txt", into: back)
        XCTAssertEqual(try String(contentsOf: landed, encoding: .utf8), "dropped")

        // In-Space MCP services (the driver's fake registry).
        let catalog = try await space.services.tools()
        XCTAssertTrue(catalog.tools.contains { $0.name == "get_screen_size" })
        let parts = try await space.services.call("get_screen_size")
        XCTAssertEqual(ToolContent.text(of: parts), "1280x800")

        // No desktop in this driver: a stream is refused, naming the feature,
        // as a throw and never as a value (§3).
        do {
            _ = try await space.streamEndpoint()
            XCTFail("the fixture has no desktop; the stream must be refused")
        } catch let SpacesError.toolFailed(tool, message) {
            XCTAssertEqual(tool, "stream_endpoint")
            XCTAssertTrue(message.contains("desktop_stream"), message)
        }

        // Teleport ships with Cua Spaces (source-available): the daemon Cua
        // Spaces runs serves it; an embedded runtime of this SDK says where
        // it ships and moves nothing.
        if keyvault {
            // Consent is a type, and the server checks it again.
            let firefox = TeleportableApp("firefox")
            let manifest = try await space.sessions.manifest(for: firefox)
            XCTAssertFalse(manifest.items.isEmpty)
            XCTAssertFalse(manifest.sensitiveEntries.isEmpty)
            // Firefox's credentials are opt-ins: the server's default
            // selection carries none, so it needs no acknowledgement.
            XCTAssertTrue(manifest.defaultSelection.allSatisfy { !$0.isSensitive })
            let approval = try manifest.approvingServerDefault(into: space)
            let arrived = { (name: String) -> Bool in
                FileManager.default.enumerator(atPath: f["spaces_teleport_home"]!)?
                    .allObjects.compactMap { $0 as? String }
                    .contains { ($0 as NSString).lastPathComponent == name } ?? false
            }
            // The Keyvault moves it: the request waits for the user, who
            // approves it in Cua; then it delivers the item it holds.
            async let sending = space.sessions.send(approval)
            fx.approveTeleport()
            let receipt = try await sending
            XCTAssertEqual(receipt.method, "import_session")
            XCTAssertFalse(receipt.transferredPaths.isEmpty)
            // The Keyvault delivers the signed-in state of the item the user
            // imported (the fixture's synthetic profile): its session
            // cookies, and no history.
            XCTAssertTrue(arrived("cookies.sqlite"), "the vault's session landed in the receiver's temp home")
            XCTAssertFalse(arrived("places.sqlite"), "history is not part of the signed-in state")
        } else {
            do {
                _ = try await space.sessions.manifest(for: TeleportableApp("firefox"))
                XCTFail("an embedded runtime has no teleport")
            } catch let SpacesError.toolFailed(tool, message) {
                XCTAssertEqual(tool, "teleport_manifest")
                XCTAssertTrue(message.contains("ships with Cua Spaces"), message)
            }
        }

        // Agent harness capabilities come from the server.
        let caps = try await space.agents.harnessCapabilities()
        XCTAssertFalse(caps.harnesses.isEmpty)

        // Delete: a Space added by address is only forgotten.
        try await space.delete()
        let left = try await connection.spaces()
        XCTAssertTrue(left.isEmpty)
    }

    private func scratch() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cuaspaces-live-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    @Test func testTheOverlayAgainstTheEmbeddedRuntime() async throws {
        guard let fx = try Fixtures.start() else { return }
        defer { fx.stop() }
        let work = try scratch()
        defer { try? FileManager.default.removeItem(at: work) }
        let connection = try SpacesConnection.embedded(
            spacesHome: work.appendingPathComponent("cua").path,
            teleportHome: fx.fields["teleport_host_home"])
        let tools = try await connection.availableTools()
        XCTAssertEqual(Set(tools), Set(SpacesConnection.contractTools))
        try await exercise(connection, fx, work: work, keyvault: false)
    }

    @Test func testTheOverlayThroughACuaDaemon() async throws {
        guard let fx = try Fixtures.start() else { return }
        defer { fx.stop() }
        guard let sock = fx.fields["daemon_socket"] else {
            return XCTFail("the fixture serves a cua daemon on unix")
        }
        let connection = try SpacesConnection.daemon(address: sock)
        let work = try scratch()
        defer { try? FileManager.default.removeItem(at: work) }
        try await exercise(connection, fx, work: work, keyvault: true)
        // The registry is the daemon's.
        let home = (sock as NSString).deletingLastPathComponent
        XCTAssertTrue(FileManager.default.fileExists(atPath: home + "/spaces.json"))
    }
}
