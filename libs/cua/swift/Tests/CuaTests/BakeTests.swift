// Sandboxes.bake: the options record and the checks that run before any VM
// starts (the bake itself is the docker tier's integration test,
// libs/images/linux/tests/bake-world.sh).
import Foundation
import Testing

@testable import Cua

@Suite struct BakeTests {
    @Test func optionsHaveDefaults() {
        let o = BakeOptions(projectDir: "/worlds/w", checkpoint: "w1")
        #expect(o.image == nil && o.name == nil && o.dest == nil)
        #expect(o.composeFiles.isEmpty && o.imageTarballs.isEmpty)
        #expect(o.health.isEmpty && o.run.isEmpty)
        #expect(o.healthTimeoutS == 600)
        #expect(!o.verifyFork && !o.keep)
    }

    @Test func checksInputsBeforeStartingAVM() async throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("cua-swift-bake-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let cua = try Cua.embedded(stateDir: dir.appendingPathComponent("sbx").path)
        let sbx = cua.sandboxes()
        do {
            _ = try await sbx.bake(
                options: BakeOptions(projectDir: dir.appendingPathComponent("missing").path, checkpoint: "w1"))
            Issue.record("expected NotFound")
        } catch CuaError.NotFound {
        }
        do {
            _ = try await sbx.bake(options: BakeOptions(projectDir: dir.path, checkpoint: " "))
            Issue.record("expected InvalidArgument")
        } catch CuaError.InvalidArgument {
        }
    }
}
