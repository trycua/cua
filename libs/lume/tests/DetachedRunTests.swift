import ArgumentParser
import Foundation
import Testing

@testable import lume

@Test("Run parses detached mode and a custom log path")
func detachedRunParsing() throws {
  let command = try Run.parse([
    "test-vm", "--detach", "--log-file", "~/custom-lume.log",
  ])

  #expect(command.detach)
  #expect(command.logFile == "~/custom-lume.log")
}

@Test("Detached child arguments remove the detach flag without changing VM options")
func detachedChildArguments() {
  let arguments = DetachedVMRunner.childArguments(from: [
    "/path/to/lume", "run", "test-vm", "--detach", "--display", "native",
    "--log-file", "/tmp/vm.log", "--shared-dir", "/tmp/shared",
  ])

  #expect(
    arguments == [
      "run", "test-vm", "--display", "native", "--shared-dir", "/tmp/shared",
    ])
}

@Test("Detached runs use a per-VM log in the user Library")
func detachedDefaultLogPath() {
  let path = DetachedVMRunner.defaultLogURL(vmName: "example/vm:latest").path
  #expect(path.hasSuffix("/Library/Logs/lume/example_vm_latest.log"))
}
