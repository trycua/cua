import Foundation
import Testing

@testable import lume

@Test("Guest copy wait command succeeds only after the pasteboard changes")
func guestClipboardWaitCommandDetectsChange() throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    let osascript = directory.appendingPathComponent("osascript")
    let invocationCount = directory.appendingPathComponent("count")
    let script = """
        #!/bin/sh
        count=$(cat '\(invocationCount.path)' 2>/dev/null || printf 0)
        count=$((count + 1))
        printf '%s' "$count" > '\(invocationCount.path)'
        if [ "$count" -lt 2 ]; then printf '41\\n'; else printf '42\\n'; fi
        """
    try script.write(to: osascript, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: osascript.path)

    let command = ClipboardWatcher.guestClipboardWaitCommand(after: 41, attempts: 3)
        .replacingOccurrences(of: "osascript ", with: "'\(osascript.path)' ")
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", command]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = Pipe()
    try process.run()
    process.waitUntilExit()

    #expect(process.terminationStatus == 0)
    #expect(String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) == "42\n")
}

@Test("Guest copy wait command reports a distinct timeout")
func guestClipboardWaitCommandTimesOut() throws {
    let command = ClipboardWatcher.guestClipboardWaitCommand(after: 41, attempts: 1)
        .replacingOccurrences(
            of: "osascript -l JavaScript -e 'ObjC.import(\"AppKit\"); $.NSPasteboard.generalPasteboard.changeCount'",
            with: "printf '41\\n'"
        )
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/sh")
    process.arguments = ["-c", command]
    try process.run()
    process.waitUntilExit()

    #expect(process.terminationStatus == 75)
}
