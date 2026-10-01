// Smoke tests of the Swift binding against libs/cua/target/*/cua-test-fixtures
// (a loopback MockServer spacesd with a scripted media socket). Nothing
// touches host apps.
import Foundation
import Testing

@testable import Cua

final class Collect: FrameSink, AudioSink, @unchecked Sendable {
    private let lock = NSLock()
    private(set) var frames: [VideoFrame] = []
    private(set) var events: [MediaEvent] = []
    private(set) var audio: [AudioPacket] = []

    func onFrame(frame: VideoFrame) { lock.lock(); frames.append(frame); lock.unlock() }
    func onEvent(event: MediaEvent) { lock.lock(); events.append(event); lock.unlock() }
    func onAudio(packet: AudioPacket) { lock.lock(); audio.append(packet); lock.unlock() }
    func snapshot() -> ([VideoFrame], [MediaEvent], [AudioPacket]) {
        lock.lock(); defer { lock.unlock() }
        return (frames, events, audio)
    }
}

struct Fixtures {
    let process: Process
    let stdin: Pipe
    let envURL: String
    let envToken: String
    /// Every field the fixture printed (spaces_url, spaces_token, ...).
    let fields: [String: String]

    static func binary() -> URL? {
        let env = ProcessInfo.processInfo.environment
        if let p = env["CUA_TEST_FIXTURES"] { return URL(fileURLWithPath: p) }
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        for profile in ["debug", "release"] {
            let u = root.appendingPathComponent("target/\(profile)/cua-test-fixtures")
            if FileManager.default.isExecutableFile(atPath: u.path) { return u }
        }
        return nil
    }

    static func start() throws -> Fixtures? {
        guard let bin = binary() else { return nil }
        let p = Process()
        p.executableURL = bin
        let input = Pipe(), output = Pipe()
        p.standardInput = input
        p.standardOutput = output
        try p.run()
        // One JSON line; bounded read.
        var data = Data()
        for _ in 0..<1000 {
            let chunk = output.fileHandleForReading.availableData
            if chunk.isEmpty { break }
            data.append(chunk)
            if data.contains(UInt8(ascii: "\n")) { break }
        }
        let line = data.split(separator: UInt8(ascii: "\n")).first ?? Data()
        let obj = try JSONSerialization.jsonObject(with: Data(line)) as! [String: Any]
        return Fixtures(
            process: p, stdin: input,
            envURL: obj["env_url"] as! String, envToken: obj["env_token"] as! String,
            fields: obj.compactMapValues { $0 as? String })
    }

    func stop() {
        try? stdin.fileHandleForWriting.close()
        process.waitUntilExit()
    }
}

@Suite struct SmokeTests {
    @Test func versionAndTypedErrors() throws {
        #expect(!cuaSdkVersion().isEmpty)
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-swift-\(UUID().uuidString)")
        let cua = try Cua.embedded(stateDir: dir.path, fleetFromEnv: false)
        #expect(cua.mode() == .embedded)
        do {
            _ = try cua.fleet()
            Issue.record("expected ProviderNotConfigured")
        } catch CuaError.ProviderNotConfigured {
        }
    }

    @Test func errorsLinkToTheirReferenceEntry() {
        let base = "https://cua.ai/docs/cua-sdk/reference/errors#"
        #expect(CuaError.NotFound(message: "x").docUrl == base + "notfound")
        #expect(CuaError.SpacesdNotAvailable(message: "y").docUrl == base + "spacesdnotavailable")
    }

    @Test(.enabled(if: Fixtures.binary() != nil, "cua-test-fixtures is not built"))
    func embeddedDirectEnvAndMedia() async throws {
        let fx = try #require(try Fixtures.start())
        defer { fx.stop() }
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-swift-\(UUID().uuidString)")
        let cua = try Cua.embedded(stateDir: dir.path, fleetFromEnv: false)
        let sb = try await cua.sandboxes().connectUrl(url: fx.envURL, token: fx.envToken, name: "swift-direct")
        #expect(sb.location() == "direct")
        let env = try await sb.spacesd(probeTimeoutMs: 5000)
        let caps = try await env.capabilities()
        #expect(!caps.version.isEmpty)
        let out = try await env.run(command: SpacesdCommand("echo", ["hi"]))
        #expect(out.exit.success)
        #expect(String(decoding: out.stdout, as: UTF8.self) == "hi\n")

        let blob = Data((0..<100_000).map { UInt8($0 % 251) })
        _ = try await env.upload(path: "/tmp/swift/blob", data: blob, options: nil)
        #expect(try await env.download(path: "/tmp/swift/blob") == blob)

        do {
            _ = try await env.download(path: "/nope")
            Issue.record("expected NotFound")
        } catch CuaError.NotFound {
        }

        let sink = Collect()
        let session = try await env.openMediaWithAudio(
            options: MediaOpenOptions(
                display: nil, windowHandle: nil, maxFps: 0, maxDimension: 0,
                audio: true, disableVideo: false, requestJson: nil),
            frames: sink, audio: sink)
        #expect(session.codec() == "h264")
        for _ in 0..<250 {
            let (f, _, a) = sink.snapshot()
            if f.count >= 2 && !a.isEmpty { break }
            try await Task.sleep(nanoseconds: 20_000_000)
        }
        let (frames, events, audio) = sink.snapshot()
        try #require(frames.count >= 2)
        #expect(frames[0].keyframe)
        #expect(frames[0].sequence == 7)
        #expect(audio.first?.frameSamples == 960)
        #expect(events.prefix(2).map(\.kind) == ["hello", "session_opened"])
        try await session.close()
        try await sb.delete()
    }
}
