import Foundation
import Testing

@testable import lume

@MainActor private final class MockRecoveryVNCDriver: RecoveryVNCDriving {
    var events: [String] = []
    var receivedSensitiveInput = false
    var usesDirectAccountPrompt = false

    func waitForListening(deadlineSeconds: Double, heartbeat: () throws -> Void) async throws {
        try heartbeat()
        events.append("listening")
    }

    func waitForFramebuffer(minSize: Int, deadlineSeconds: Double, label: String, heartbeat: () throws -> Void)
        async throws
    {
        try heartbeat()
        events.append("framebuffer:\(label)")
    }

    func waitForText(
        containing expectedPhrases: [String], rejecting failurePhrases: [String], deadlineSeconds: Double,
        label: String, heartbeat: () throws -> Void
    ) async throws -> String {
        try heartbeat()
        let text: String
        if label == "csrutil prompt" {
            text = usesDirectAccountPrompt ? expectedPhrases.last ?? "" : "[y/n]"
        } else {
            text = expectedPhrases.first ?? ""
        }
        events.append("text:\(text)")
        return text
    }

    func type(_ text: String) throws { events.append("type:\(text)") }

    func typeSensitive(_ text: String) throws {
        receivedSensitiveInput = !text.isEmpty
        events.append("sensitive-input")
    }

    func key(_ name: String) throws { events.append("key:\(name)") }

    func click(x: Int, y: Int) throws { events.append("click:\(x),\(y)") }

    func snapshot(_ tag: String) throws { events.append("snapshot:\(tag)") }
}

@MainActor @Test("sip parses stdin credentials and the short confirmation flag") func sipCommandParsing() throws {
    let root = try Lume.parseAsRoot(["sip", "off", "test-vm", "-y", "--admin-password-stdin"])
    let command = try #require(root as? Sip)

    #expect(command.state == .off)
    #expect(command.name == "test-vm")
    #expect(command.yes)
    #expect(command.adminPasswordStdin)
}

@Test("sip validates VM state and target status") func sipValidation() throws {
    let stoppedMac = vmDetails(os: "macOS", status: "stopped")
    try Sip.validateInitialState(stoppedMac, expectedName: "test-vm")
    #expect(Sip.statusMatches("System Integrity Protection status: disabled.", expected: .off))
    #expect(Sip.statusMatches("System Integrity Protection status: enabled.", expected: .on))
    #expect(!Sip.statusMatches("System Integrity Protection status: enabled.", expected: .off))
    let customized = """
        System Integrity Protection status: enabled (Custom Configuration).
        Filesystem Protections: disabled
        Debugging Restrictions: enabled
        """
    #expect(!Sip.statusMatches(customized, expected: .off))
    #expect(!Sip.statusMatches(customized, expected: .on))

    #expect(throws: SipError.self) {
        try Sip.validateInitialState(vmDetails(os: "linux", status: "stopped"), expectedName: "test-vm")
    }
    #expect(throws: SipError.self) {
        try Sip.validateInitialState(vmDetails(os: "macOS", status: "running"), expectedName: "test-vm")
    }
    #expect(throws: SipError.self) { try Sip.requireVNCExecutable(nil) }
    #expect(throws: SipError.self) { try Sip.validateVNCPasswordCharacters("Uppercase") }

    let vncPasswords = Swift.Set<String>((0..<8).map { _ in Sip.makeVNCPassword() })
    #expect(vncPasswords.allSatisfy { $0.utf8.count == 8 })
    #expect(vncPasswords.count > 1)
}

@MainActor @Test("recovery automation enters the requested csrutil command and verifies its result")
func sipRecoveryAutomation() async throws {
    for state in [Sip.State.off, .on] {
        let driver = MockRecoveryVNCDriver()
        driver.usesDirectAccountPrompt = state == .on
        var heartbeats = 0
        let automator = SipRecoveryAutomator(sleeper: { _ in })

        try await automator.perform(
            driver: driver, state: state, adminUser: "lume", adminPassword: "secret", heartbeat: { heartbeats += 1 },
            timeout: { limit, _ in limit })

        let verb = state == .off ? "disable" : "enable"
        let result = state == .off ? "off" : "on"
        #expect(driver.events.contains("type:csrutil \(verb)"))
        let expectedPrompt = state == .off ? "text:enter password for user lume:" : "text:password"
        #expect(driver.events.contains(expectedPrompt))
        #expect(driver.events.contains("key:y") == (state == .off))
        #expect(driver.events.contains("text:system integrity protection is \(result)."))
        #expect(driver.events.contains("sensitive-input"))
        #expect(driver.receivedSensitiveInput)
        #expect(driver.events.suffix(3).contains("type:sync; shutdown -h now"))
        #expect(heartbeats > 0)
    }
}

@MainActor @Test("sensitive VNC input is sent on stdin and never in argv") func vncSensitiveInputUsesStdin() throws {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent(
        "lume-sip-vnc-test-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: directory) }

    let executable = directory.appendingPathComponent("vncdo")
    let argumentsLog = directory.appendingPathComponent("arguments")
    let stdinLog = directory.appendingPathComponent("stdin")
    let script = """
        #!/bin/sh
        printf '%s\\n' "$@" > '\(argumentsLog.path)'
        head -c 262144 /dev/zero
        cat > '\(stdinLog.path)'
        """
    try script.write(to: executable, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: executable.path)

    let secret = "sentinel-secret"
    let driver = VNCDriver(
        vncdo: executable.path, host: "127.0.0.1", port: 5999, password: "transient-vnc-password", screenshotDir: nil)
    try driver.typeSensitive(secret)

    let arguments = try String(contentsOf: argumentsLog, encoding: .utf8)
    let standardInput = try String(contentsOf: stdinLog, encoding: .utf8)
    #expect(!arguments.contains(secret))
    #expect(arguments.contains("typefile\n-"))
    #expect(standardInput == secret)
}

@MainActor @Test("lume child output cannot fill an undrained pipe") func sipChildProcessHandlesLargeOutput()
    async throws
{
    let child = try SipChildProcess(
        executable: "/bin/sh", arguments: ["-c", "yes output | head -c 262144"], label: "large-output-test")
    defer { child.cleanup() }

    try child.start()
    for _ in 0..<100 where child.isRunning { try await Task.sleep(for: .milliseconds(20)) }
    if child.isRunning { await child.terminateAndWait() }

    #expect(!child.isRunning)
    #expect(child.terminationStatus == 0)
    #expect(!child.outputTail(redacting: []).isEmpty)
}

private func vmDetails(os: String, status: String) -> VMDetails {
    VMDetails(
        name: "test-vm", os: os, cpuCount: 4, memorySize: 8 * 1024 * 1024 * 1024,
        diskSize: DiskSize(allocated: 1, total: 2), display: "1024x768", status: status, vncUrl: nil, ipAddress: nil,
        locationName: "home")
}
