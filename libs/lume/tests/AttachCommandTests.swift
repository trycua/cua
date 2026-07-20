import ArgumentParser
import Foundation
import Testing

@testable import lume

@Test("Attach parses native and VNC display choices")
func attachCommandParsing() throws {
  let automatic = try Attach.parse(["test-vm"])
  let native = try Attach.parse(["test-vm", "--display", "native"])
  let vnc = try Attach.parse(["test-vm", "--display=vnc", "--storage", "external"])

  #expect(automatic.display == nil)
  #expect(native.display == .native)
  #expect(vnc.display == .vnc)
  #expect(vnc.storage == "external")
  #expect(throws: Error.self) {
    _ = try Attach.parse(["test-vm", "--display", "none"])
  }
}

@Test("A VM without a registered owner cannot accept native attachment")
func nativeAttachRequiresRegisteredOwner() throws {
  let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
  try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
  defer { try? FileManager.default.removeItem(at: root) }

  let directory = VMDirectory(Path(root.path))
  #expect(!NativeDisplayAttachService.isAvailable(vmDirectory: directory))
  #expect(throws: NativeDisplayAttachError.self) {
    try NativeDisplayAttachService.requestNativeDisplay(vmDirectory: directory)
  }
}
