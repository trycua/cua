// Space labels through the Swift binding: the options and SpaceInfo types,
// and the limits checked before anything is created. Creating a labelled
// Space needs a VM (crates/cua-spaces/tests/generic.rs and the live runs
// cover that). Nothing touches the real home.
import Foundation
import Testing

@testable import Cua

@Suite(.serialized) struct SpaceLabelsTests {
    @Test func labelsTypesAndLimits() async throws {
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
        let info = try await spaces.add(url: f["spaces_url"]!, token: f["spaces_token"], name: "swift-labels")
        #expect(info.labels.isEmpty)
        #expect(try await spaces.list().allSatisfy { $0.labels.isEmpty })

        let opts = SpaceCreateOptions(labels: ["team": "research", "run": "7"])
        #expect(opts.labels == ["team": "research", "run": "7"])
        #expect(SpaceCreateOptions().labels.isEmpty)

        var tooMany: [String: String] = [:]
        for i in 0..<65 { tooMany["k\(i)"] = "v" }
        let bad: [[String: String]] = [
            ["": "v"], ["k": ""], [String(repeating: "k", count: 64): "v"],
            ["k": String(repeating: "v", count: 257)], tooMany,
        ]
        for labels in bad {
            do {
                _ = try await spaces.create(options: SpaceCreateOptions(on: "local", labels: labels))
                Issue.record("labels \(labels.count) must be refused")
            } catch CuaError.InvalidArgument {}
        }
        // A durable or reused Space cannot take labels.
        do {
            _ = try await spaces.create(
                options: SpaceCreateOptions(on: "local", reuse: true, labels: ["a": "b"]))
            Issue.record("reuse with labels must be refused")
        } catch CuaError.InvalidArgument(let m) {
            #expect(m.contains("labels"))
        }
        _ = try await spaces.delete(space: info.id)
    }
}
