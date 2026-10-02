// Spaces through the Swift binding, embedded and against a `cua daemon`,
// mirroring libs/cua/python/tests/test_spaces.py. The Space is the
// fixture's in-process cua-spacesd core (temp guest HOME/PATH, temp
// Downloads and teleport home, fake driver tools). Teleport ships with Cua
// Spaces: an embedded runtime and the MIT `cua daemon` report it missing, and
// the Cua Spaces daemon (`cua-spaces-cli daemon`) serves it, reading a
// synthetic Firefox profile through a side-effect-free host rooted at a temp
// directory. Nothing touches the real home, apps or keychain.
import Foundation
import Testing

@testable import Cua

final class Approver: TeleportApprover, @unchecked Sendable {
    private let lock = NSLock()
    private let decision: TeleportDecision?
    private(set) var seen: [TeleportManifest] = []
    init(_ decision: TeleportDecision?) { self.decision = decision }
    func approve(manifest: TeleportManifest) -> TeleportDecision? {
        lock.lock(); seen.append(manifest); lock.unlock()
        return decision
    }
}

func cuaCLI(env name: String = "CUA_CLI", binary: String = "cua") -> URL? {
    let env = ProcessInfo.processInfo.environment
    if let p = env[name] { return URL(fileURLWithPath: p) }
    let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
    for profile in ["debug", "release"] {
        let u = root.appendingPathComponent("target/\(profile)/\(binary)")
        if FileManager.default.isExecutableFile(atPath: u.path) { return u }
    }
    return nil
}

func findFile(named name: String, under dir: String) -> URL? {
    let e = FileManager.default.enumerator(atPath: dir)
    var n = 0
    while let rel = e?.nextObject() as? String, n < 10_000 {
        n += 1
        if (rel as NSString).lastPathComponent == name {
            return URL(fileURLWithPath: dir).appendingPathComponent(rel)
        }
    }
    return nil
}

func exerciseSpaces(_ cua: Cua, _ fx: Fixtures, tmp: URL, teleport: Bool = false) async throws {
    let f = fx.fields
    let spaces = cua.spaces()
    let info = try await spaces.add(url: f["spaces_url"]!, token: f["spaces_token"], name: "swift-space")
    #expect(info.provider == "direct" && info.id.hasPrefix("direct:"))
    #expect(info.features.contains("driver"))
    #expect(try await spaces.list().map(\.id) == [info.id])
    #expect(try await spaces.resolve(space: "swift-space").id == info.id)
    do {
        _ = try await spaces.add(url: f["spaces_url"]!, token: "wrong", name: nil)
        Issue.record("a wrong token must be refused")
    } catch CuaError.Unauthenticated {}

    let space = try await spaces.space(space: info.id)
    let out = try await space.bash(command: "echo hi; exit 3", timeoutMs: nil)
    #expect(out.stdout == "hi\n" && out.exitCode == 3 && out.rendered == "hi\n[exit 3]")
    #expect(try await space.home() == f["spaces_guest_home"]!)

    let guest = f["spaces_guest_home"]! + "/swift/note.txt"
    #expect(try await space.write(path: guest, content: Data("from swift".utf8)).bytes == 10)
    let src = tmp.appendingPathComponent("drop.txt")
    try Data("dropped".utf8).write(to: src)
    let sent = try await space.sendFile(
        localPath: src.path, options: SpaceSendFileOptions(targetDirectory: "swift-inbox"))
    #expect(sent.verified && sent.files.count == 1)
    #expect(try String(contentsOfFile: f["spaces_downloads"]! + "/swift-inbox/drop.txt") == "dropped")
    let back = tmp.appendingPathComponent("back")
    try FileManager.default.createDirectory(at: back, withIntermediateDirectories: true)
    let down = try await space.download(remotePath: guest, destDir: back.path)
    #expect(down.verified)
    #expect(try String(contentsOf: back.appendingPathComponent("note.txt")) == "from swift")

    #expect(try await space.listTools(service: nil).contains { $0.name == "get_screen_size" })
    let r = try await space.callTool(tool: "get_screen_size", argumentsJson: "{}", service: nil, timeoutMs: nil)
    #expect(!r.isError && r.text == "1280x800")

    do {
        _ = try await space.openStream(options: SpaceStreamOptions())
        Issue.record("the fixture has no desktop; streams must be refused")
    } catch CuaError.CapabilityMissing {}

    let ok = Approver(TeleportDecision(include: nil, acknowledgeSensitive: true))
    if teleport {
        // The Cua Spaces daemon: the manifest, and a session only through the
        // Keyvault (the daemon never delivers a caller-approved session).
        let manifest = try await space.teleportManifest(app: "firefox", scope: nil)
        #expect(manifest.items.contains { $0.isSensitive })
        do {
            _ = try await space.teleport(app: "firefox", scope: nil, approver: Approver(nil))
            Issue.record("a declined approval must refuse")
        } catch CuaError.TeleportRefused {}
        do {
            _ = try await space.teleport(app: "firefox", scope: nil, approver: ok)
            Issue.record("the daemon must defer session teleport to the Keyvault")
        } catch CuaError.TeleportRefused {}
    } else {
        // Without Cua Spaces: missing, and it says where it ships.
        do {
            _ = try await space.teleportManifest(app: "firefox", scope: nil)
            Issue.record("teleport ships with Cua Spaces")
        } catch CuaError.HostCapabilityMissing(let m) {
            #expect(m.contains("Cua Spaces"))
        }
        do {
            _ = try await space.teleport(app: "firefox", scope: nil, approver: ok)
            Issue.record("teleport ships with Cua Spaces")
        } catch CuaError.HostCapabilityMissing {}
    }

    let listed = try await spaces.callToolJson(tool: "list_spaces", argumentsJson: nil)
    #expect(!listed.isError && listed.text.contains(info.id))
    #expect(try await spaces.delete(space: info.id).contains(info.id))
    #expect(try await spaces.list().isEmpty)
}

@Suite(.serialized) struct SpacesTests {
    @Test func everyContractToolMapsToAGeneratedMethod() {
        let rows = spacesToolMethods()
        #expect(rows.count == 86)
        // Swift spells `Class.method_name` as `methodName` on the generated
        // protocol; check each against the protocol's requirement list.
        let spaceMethods: Set<String> = [
            "add", "list", "resolve", "remove", "create", "delete",
            "stop", "start", "space", "agentCapabilities", "listToolsJson", "callToolJson",
            "cloudStatus", "cloudConnect", "cloudTest", "cloudDisconnect", "cloudSweep",
            "persistentAgentCreate", "persistentAgents", "persistentAgentRemove",
            "persistentAgentSend", "persistentAgentSave", "agentPause", "agentResume",
            "routineAdd", "routines", "routineRemove", "routineSetEnabled", "notifyUser",
            "notifications", "notificationsAck", "computerAccessGrant",
            "computerAccessRevoke", "computerAccess", "relayRegister", "relayUnregister",
            "volumeLs", "volumeRead", "volumeWrite", "volumeDelete", "volumeHistory",
            "volumeRestore", "volumeGrant", "volumeRevoke", "volumeGrants",
            "volumeRequestAccess", "volumeRequests", "volumeApprove", "volumeDeny", "volumeAudit",
            "volumeStorage", "volumeStorageSet", "volumeMountStatus", "volumeMount", "volumeUnmount",
            "volumeSyncStatus", "volumeSyncEvents", "volumeSyncResolve", "volumeCacheStats",
            "volumeCacheSet", "volumeCacheClear",
        ]
        let spaceHandle: Set<String> = [
            "bash", "write", "home", "upload", "download", "sendFile", "listTools", "callTool",
            "windows", "openStream", "closeStream", "streamSession", "joinPresence",
            "teleportManifest", "teleport", "startHotspot", "stopHotspot", "hotspotStatus",
            "agentStart", "agentStatus", "agentMessage", "agentStop", "agentList",
            "agentEvents", "agentInterrupt", "requestSiteLogin", "share", "unshare", "shares",
        ]
        for row in rows {
            let parts = row.method.split(separator: ".").map(String.init)
            let camel = parts[1].split(separator: "_").enumerated()
                .map { $0.offset == 0 ? String($0.element) : $0.element.capitalized }.joined()
            let known = parts[0] == "Spaces" ? spaceMethods : spaceHandle
            #expect(known.contains(camel), "\(row.tool) -> \(row.method) (\(camel))")
        }
        // The sets above are the generated protocols' own requirements: this
        // line fails to compile if any is missing.
        let _: [(any SpacesProtocol) -> Any] = [
            { $0.add }, { $0.list }, { $0.resolve }, { $0.remove }, { $0.create },
            { $0.delete }, { $0.stop }, { $0.start }, { $0.space }, { $0.agentCapabilities },
            { $0.listToolsJson }, { $0.callToolJson },
            { $0.cloudStatus }, { $0.cloudConnect }, { $0.cloudTest }, { $0.cloudDisconnect },
            { $0.cloudSweep },
            { $0.persistentAgentCreate }, { $0.persistentAgents }, { $0.persistentAgentRemove },
            { $0.persistentAgentSend }, { $0.persistentAgentSave }, { $0.agentPause },
            { $0.agentResume }, { $0.routineAdd }, { $0.routines }, { $0.routineRemove },
            { $0.routineSetEnabled }, { $0.notifyUser }, { $0.notifications },
            { $0.notificationsAck }, { $0.computerAccessGrant }, { $0.computerAccessRevoke },
            { $0.computerAccess }, { $0.relayRegister }, { $0.relayUnregister },
            { $0.volumeLs }, { $0.volumeRead }, { $0.volumeWrite }, { $0.volumeDelete },
            { $0.volumeHistory }, { $0.volumeRestore }, { $0.volumeGrant }, { $0.volumeRevoke },
            { $0.volumeGrants }, { $0.volumeRequestAccess }, { $0.volumeRequests },
            { $0.volumeApprove }, { $0.volumeDeny }, { $0.volumeAudit },
            { $0.volumeStorage }, { $0.volumeStorageSet }, { $0.volumeMountStatus },
            { $0.volumeMount }, { $0.volumeUnmount }, { $0.volumeSyncStatus },
            { $0.volumeSyncEvents }, { $0.volumeSyncResolve }, { $0.volumeCacheStats },
            { $0.volumeCacheSet }, { $0.volumeCacheClear },
        ]
        let _: [(any SpaceProtocol) -> Any] = [
            { $0.bash }, { $0.write }, { $0.home }, { $0.upload }, { $0.download }, { $0.sendFile },
            { $0.listTools }, { $0.callTool }, { $0.windows }, { $0.openStream }, { $0.closeStream },
            { $0.streamSession }, { $0.joinPresence }, { $0.teleportManifest }, { $0.teleport },
            { $0.startHotspot }, { $0.stopHotspot }, { $0.hotspotStatus }, { $0.agentStart },
            { $0.agentStatus }, { $0.agentMessage }, { $0.agentStop }, { $0.agentList },
            { $0.requestSiteLogin }, { $0.share }, { $0.unshare }, { $0.shares },
        ]
    }

    @Test func spacesEmbedded() async throws {
        guard let fx = try Fixtures.start() else { return }
        defer { fx.stop() }
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("cua-swift-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let cua = try Cua.embedded(
            stateDir: tmp.appendingPathComponent("sbx").path, fleetFromEnv: false,
            spacesHome: tmp.appendingPathComponent("cua").path,
            teleportHome: fx.fields["teleport_host_home"])
        try await exerciseSpaces(cua, fx, tmp: tmp)
    }

    @Test func spacesThroughTheDaemon() async throws {
        guard let cli = cuaCLI() else { return }
        try await daemonExercise(cli: cli, teleport: false)
    }

    @Test func spacesThroughTheCuaSpacesDaemon() async throws {
        guard let cli = cuaCLI(env: "CUA_SPACES_CLI", binary: "cua-spaces-cli") else { return }
        try await daemonExercise(cli: cli, teleport: true)
    }

    func daemonExercise(cli: URL, teleport: Bool) async throws {
        guard let fx = try Fixtures.start() else { return }
        defer { fx.stop() }
        // Short path: macOS caps Unix socket paths at 104 bytes.
        let home = "/tmp/cua-sw-\(UUID().uuidString.prefix(8))"
        try FileManager.default.createDirectory(atPath: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(atPath: home) }
        let sock = home + "/cua.sock"
        let daemon = Process()
        daemon.executableURL = cli
        daemon.arguments = ["daemon", "start", "--foreground", "--socket", sock, "--state-dir", home + "/sbx"]
        var env = ProcessInfo.processInfo.environment
        env["HOME"] = home
        env["CUA_HOME"] = home
        env["CUA_SPACES_TELEPORT_HOME"] = fx.fields["teleport_host_home"]
        env["CUA_SPACES_AGENT_CREDENTIALS_HOME"] = "none"
        daemon.environment = env
        daemon.standardOutput = FileHandle.nullDevice
        daemon.standardError = FileHandle.nullDevice
        try daemon.run()
        defer { if daemon.isRunning { daemon.terminate(); daemon.waitUntilExit() } }
        for _ in 0..<150 where !FileManager.default.fileExists(atPath: sock) {
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        let cua = try Cua.connect(address: sock, token: nil)
        let tmp = URL(fileURLWithPath: home).appendingPathComponent("work")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        try await exerciseSpaces(cua, fx, tmp: tmp, teleport: teleport)
        #expect(FileManager.default.fileExists(atPath: home + "/spaces.json"))
        try await cua.shutdownDaemon()
    }
}
