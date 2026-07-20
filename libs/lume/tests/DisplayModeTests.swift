import ArgumentParser
import Foundation
import Testing

@testable import lume

@Test("DisplayMode parses all supported values")
func displayModeParsing() throws {
  #expect(try Run.parse(["test-vm"]).display == .none)
  #expect(try Run.parse(["test-vm", "--display", "vnc"]).display == .vnc)
  #expect(try Run.parse(["test-vm", "--display", "native"]).display == .native)
  #expect(try Run.parse(["test-vm", "--display", "none"]).display == .none)
  #expect(throws: Error.self) {
    _ = try Run.parse(["test-vm", "--display", "invalid"])
  }
}

@MainActor
@Test("Native macOS display enables the SSH clipboard fallback")
func nativeClipboardFallbackSelection() {
    #expect(
        VM.shouldStartClipboardWatcher(
            displayMode: .native,
            osType: "macOS",
            explicitlyRequested: false
        )
    )
    #expect(
        !VM.shouldStartClipboardWatcher(
            displayMode: .native,
            osType: "linux",
            explicitlyRequested: false
        )
    )
    #expect(
        VM.shouldStartClipboardWatcher(
            displayMode: .vnc,
            osType: "linux",
            explicitlyRequested: true
        )
    )
}

@Test("--no-display remains an alias for display none")
func noDisplayCompatibility() throws {
  let command = try Run.parse(["test-vm", "--display", "native", "--no-display"])
  let shortCommand = try Run.parse(["test-vm", "-d"])
  #expect(command.noDisplay)
  #expect(shortCommand.noDisplay)
  #expect(DisplayMode.resolve(requested: command.display, noDisplay: command.noDisplay) == .none)
}
