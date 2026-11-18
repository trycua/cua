// Egress policies through the Swift binding: the types (the policy and where
// it is enforced), the options, the input validation and the refusals, all
// decided before anything runs. Enforcement is the host's (nothing here
// touches a firewall); the rules are covered by the unit tests in
// crates/cua-vmm/src/egress.rs and the opt-in runs in
// crates/cua-daemon/tests/space_egress.rs prove them against a guest root.
import Foundation
import Testing

@testable import Cua

@Suite(.serialized) struct SpaceEgressTests {
    @Test func egressTypesAndValidation() async throws {
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
        let info = try await spaces.add(url: f["spaces_url"]!, token: f["spaces_token"], name: "swift-egress")

        #expect(info.egress == EgressPolicy())
        #expect(info.egressEnforced == nil)
        let policy = EgressPolicy(allow: ["1.1.1.1", "2606:4700::1111"], deny: ["all"])
        #expect(policy.deny == ["all"] && policy.allow.count == 2)
        #expect(SpaceCreateOptions().egress == nil)

        for bad in [["example.com"], ["*.example.com"], ["10.0.0.0/33"], ["1.2.3"], [""]] {
            do {
                try await spaces.setEgress(space: info.id, policy: EgressPolicy(deny: bad))
                Issue.record("\(bad) must be refused")
            } catch CuaError.InvalidArgument {}
            do {
                _ = try await spaces.create(
                    options: SpaceCreateOptions(on: "local", egress: EgressPolicy(deny: bad)))
                Issue.record("\(bad) must be refused at create")
            } catch CuaError.InvalidArgument {}
        }
        do {
            try await spaces.setEgress(space: info.id, policy: EgressPolicy(deny: ["example.com"]))
        } catch CuaError.InvalidArgument(let m) {
            #expect(m.contains("by IP/CIDR"))
        }
        do {
            _ = try await spaces.create(
                options: SpaceCreateOptions(on: "local", reuse: true, egress: policy))
            Issue.record("reuse with egress must be refused")
        } catch CuaError.InvalidArgument(let m) {
            #expect(m.contains("egress"))
        }
        do {
            _ = try await spaces.create(options: SpaceCreateOptions(on: "cloud", egress: policy))
            Issue.record("Cua Cloud has ended")
        } catch {
            // `on: "cloud"` is refused before anything is made.
            #expect("\(error)".contains("Cua Cloud has ended"))
        }
        do {
            try await spaces.setEgress(space: info.id, policy: policy)
            Issue.record("a Space added by address cannot take a policy")
        } catch CuaError.Unsupported(let m) {
            #expect(m.contains("not run by cua"))
        }
        #expect(EgressEnforcement.host == EgressEnforcement.host)
        _ = try await spaces.delete(space: info.id)
    }
}
