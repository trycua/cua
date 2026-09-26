import Foundation
import Testing

@testable import lume

@MainActor
@Test("run parses repeated stacked disk options")
func runCommandParsesStackedDisks() throws {
    let root = try Lume.parseAsRoot([
        "run",
        "test-vm",
        "--disk-stack",
        "base.asif+session.asif",
        "--disk-stack=other.asif+scratch.asif",
    ])
    let command = try #require(root as? Run)

    #expect(command.name == "test-vm")
    #expect(command.stackedDisks == ["base.asif+session.asif", "other.asif+scratch.asif"])
}

@Test("a bare layer defaults to overlay")
func stackedDiskDefaultsToOverlay() throws {
    let spec = try StackedDiskSpec(argument: "/tmp/base.asif+/tmp/session.asif")

    #expect(spec.base.path == "/tmp/base.asif")
    #expect(spec.layers.count == 1)
    #expect(spec.layers[0].kind == .overlay)
    #expect(spec.writableLayer?.path == "/tmp/session.asif")
}

@Test("layer types are parsed and ordering is preserved")
func stackedDiskParsesLayerKinds() throws {
    let spec = try StackedDiskSpec(
        argument: "/tmp/base.asif+/tmp/warm.asif@cache+/tmp/session.asif@overlay")

    #expect(spec.layers.map(\.kind) == [.cache, .overlay])
    #expect(spec.layers.map(\.path.path) == ["/tmp/warm.asif", "/tmp/session.asif"])
    #expect(spec.writableLayer?.path == "/tmp/session.asif")
    #expect(spec.paths.count == 3)
}

@Test("tilde in a stacked disk path is expanded")
func stackedDiskExpandsTilde() throws {
    let spec = try StackedDiskSpec(argument: "~/base.asif+~/session.asif")

    #expect(!spec.base.path.contains("~"))
    #expect(spec.base.path.hasPrefix(NSHomeDirectory()))
}

@Test("a base with no layers is rejected")
func stackedDiskRejectsBaseOnly() throws {
    #expect(throws: StackedDiskError.self) {
        _ = try StackedDiskSpec(argument: "/tmp/base.asif")
    }
}

@Test("an empty base is rejected")
func stackedDiskRejectsEmptyBase() throws {
    #expect(throws: StackedDiskError.self) {
        _ = try StackedDiskSpec(argument: "+/tmp/session.asif")
    }
}

@Test("an empty layer is rejected")
func stackedDiskRejectsEmptyLayer() throws {
    #expect(throws: StackedDiskError.self) {
        _ = try StackedDiskSpec(argument: "/tmp/base.asif+")
    }
}

@Test("an unknown layer type is rejected")
func stackedDiskRejectsUnknownLayerKind() throws {
    #expect(throws: StackedDiskError.self) {
        _ = try StackedDiskSpec(argument: "/tmp/base.asif+/tmp/session.asif@bogus")
    }
}
