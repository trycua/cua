// `ExitInfo.signalNumber` / `signalName` report any terminating signal.
//
// `ExitInfo.signal` only names the signals of the contract enum (kill, term,
// ...). A process that dies of SIGSEGV, SIGABRT or SIGPIPE leaves it nil; the
// raw number and the conventional name say what happened. Against the
// fixture's in-process cua-spacesd core (real child processes).
import Foundation
import Testing

@testable import Cua

private func runSh(_ env: SpacesdClient, _ script: String) async throws -> ExitInfo {
    let proc = try await env.spawn(command: SpacesdCommand("sh", ["-c", script]))
    while let ev = try await proc.nextEvent() {
        if ev.kind == .exit, let exit = ev.exit { return exit }
    }
    throw CocoaError(.fileReadUnknown)
}

@Suite(.serialized) struct ProcessSignalTests {
    @Test func signalFieldsDefaultToNil() {
        let info = ExitInfo(code: 0, signal: nil, timedOut: false, error: nil, success: true)
        #expect(info.signalNumber == nil && info.signalName == nil)
    }

    @Test func signalDeathsCarryNumberAndName() async throws {
        guard let fx = try Fixtures.start() else { return }
        defer { fx.stop() }
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("cua-swift-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let cua = try Cua.embedded(
            stateDir: tmp.appendingPathComponent("sbx").path,
            spacesHome: tmp.appendingPathComponent("cua").path)
        let f = fx.fields
        let env = try await cua.spacesd(url: f["spaces_url"]!, token: f["spaces_token"])

        // Outside the contract enum: only the raw fields say which signal.
        for (name, number) in [("SEGV", SIGSEGV), ("ABRT", SIGABRT), ("PIPE", SIGPIPE)] {
            let exit = try await runSh(env, "kill -\(name) $$; sleep 5")
            #expect(exit.code == nil && !exit.success)
            #expect(exit.signal == nil)
            #expect(exit.signalName == name)
            #expect(exit.signalNumber == number)
        }

        // Inside the enum: both are set.
        let killed = try await runSh(env, "kill -KILL $$; sleep 5")
        #expect(killed.signal == "kill")
        #expect(killed.signalNumber == 9 && killed.signalName == "KILL")

        // A normal exit has no signal at all.
        let ok = try await runSh(env, "exit 3")
        #expect(ok.code == 3)
        #expect(ok.signalNumber == nil && ok.signalName == nil)
    }
}
