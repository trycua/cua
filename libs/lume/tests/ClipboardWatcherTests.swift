import AppKit
import Foundation
import Testing

@testable import lume

@Test("Guest copy wait command succeeds when the pasteboard already changed")
func guestClipboardWaitCommandDetectsChange() throws {
    let current = NSPasteboard.general.changeCount
    let command = ClipboardWatcher.guestClipboardWaitCommand(after: current - 1, attempts: 3)
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", command]
    process.standardOutput = Pipe()
    process.standardError = Pipe()
    try process.run()
    process.waitUntilExit()

    #expect(process.terminationStatus == 0)
}

@Test("Guest copy wait command reports a distinct timeout")
func guestClipboardWaitCommandTimesOut() throws {
    let command = ClipboardWatcher.guestClipboardWaitCommand(
        after: NSPasteboard.general.changeCount,
        attempts: 1
    )
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", command]
    try process.run()
    process.waitUntilExit()

    #expect(process.terminationStatus == 75)
}
