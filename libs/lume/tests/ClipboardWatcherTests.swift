import Foundation
import Testing

@testable import lume

@Test("Guest copy wait command succeeds only after the pasteboard changes")
func guestClipboardWaitCommandDetectsChange() throws {
    let command = ClipboardWatcher.guestClipboardWaitCommand(
        after: 41,
        attempts: 3,
        changeCountExpression: "42"
    )
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
        after: 41,
        attempts: 1,
        changeCountExpression: "41"
    )
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", command]
    try process.run()
    process.waitUntilExit()

    #expect(process.terminationStatus == 75)
}
