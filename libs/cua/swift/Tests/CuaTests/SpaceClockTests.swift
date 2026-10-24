// When a Space was created and when it ends (SpaceInfo.createdAtUnix /
// expiresAtUnix / onExpire), read back through the Swift binding against the
// fixture's in-process cua-spacesd core. No VMs; nothing touches the real home.
import Foundation
import Testing

@testable import Cua

@Suite(.serialized) struct SpaceClockTests {
    @Test func createdAndExpiresAreListed() async throws {
        guard let fx = try Fixtures.start() else { return }
        defer { fx.stop() }
        let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("cua-swift-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let cua = try Cua.embedded(
            stateDir: tmp.appendingPathComponent("sbx").path,
            spacesHome: tmp.appendingPathComponent("cua").path)
        let spaces = cua.spaces()
        let f = fx.fields
        let t0 = Int64(Date().timeIntervalSince1970)
        let info = try await spaces.add(url: f["spaces_url"]!, token: f["spaces_token"], name: "swift-clock")
        #expect(info.createdAtUnix >= t0 - 2 && info.createdAtUnix <= t0 + 10)
        #expect(info.expiresAtUnix == 0 && info.onExpire.isEmpty)

        let at = try await spaces.setLifetime(space: info.id, seconds: 60, onExpire: nil)
        var row = try await spaces.list()[0]
        #expect(row.expiresAtUnix == at && row.onExpire == "delete")
        #expect(row.createdAtUnix == info.createdAtUnix)

        let later = try await spaces.setLifetime(space: info.id, seconds: 600, onExpire: "suspend")
        row = try await spaces.list()[0]
        #expect(row.expiresAtUnix == later && row.onExpire == "suspend")
        do {
            _ = try await spaces.setLifetime(space: info.id, seconds: 1, onExpire: "explode")
            Issue.record("an unknown action must be refused")
        } catch CuaError.InvalidArgument {}
        _ = try await spaces.delete(space: info.id)
    }
}
