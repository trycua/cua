// `SpacesdCommand.scrollbackBytes` sizes the guest's output scrollback ring.
//
// The guest keeps a process's output in a bounded ring and never blocks the
// process, so a reader that falls behind by more than the ring loses the
// evicted bytes (an offset gap in ProcessEvent.offset). Against the fixture's
// in-process cua-spacesd core (real child processes), a consumer that starts
// reading only after the process finished still gets all 16 MiB, in order,
// with a 64 MiB ring; with a 1 MiB ring the same reader sees the gap.
import Foundation
import Testing

@testable import Cua

private let mib = 1024 * 1024
private let size = 16 * mib

private func producer(_ scrollbackBytes: UInt64?) -> SpacesdCommand {
    var c = SpacesdCommand(
        "sh", ["-c", "yes 0123456789abcdefghijklmnopqrstuvwxyz | head -c \(size)"])
    c.scrollbackBytes = scrollbackBytes
    return c
}

/// Reads after the process finished, then slowly.
private func slowRead(_ proc: SpacesdProcess) async throws -> (got: Int, gap: Bool, success: Bool) {
    try await Task.sleep(nanoseconds: 2_000_000_000)
    var got = 0
    var gap = false
    while let ev = try await proc.nextEvent() {
        if ev.kind == .exit { return (got, gap, ev.exit?.success ?? false) }
        if Int(ev.offset) != got { gap = true }
        got += ev.data.count
        if got % (4 * mib) < ev.data.count { try await Task.sleep(nanoseconds: 200_000_000) }
    }
    Issue.record("no exit event")
    return (got, gap, false)
}

@Suite(.serialized) struct ProcessScrollbackTests {
    @Test func scrollbackBytesDefaultsToNil() {
        #expect(SpacesdCommand("x").scrollbackBytes == nil)
    }

    @Test func largeOutputReadSlowlyArrivesWhole() async throws {
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

        var r = try await slowRead(try await env.spawn(command: producer(UInt64(64 * mib))))
        #expect(r.success)
        #expect(!r.gap, "offset gap")
        #expect(r.got == size)

        // Control: a 1 MiB ring loses what the slow reader did not take in time.
        r = try await slowRead(try await env.spawn(command: producer(UInt64(mib))))
        #expect(r.success)
        #expect(r.gap && r.got < size)

        // Over the server maximum is clamped, not refused.
        r = try await slowRead(try await env.spawn(command: producer(1 << 40)))
        #expect(!r.gap)
        #expect(r.got == size)
    }
}
