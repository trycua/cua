import ArgumentParser
import Foundation

/// `lume sip <on|off> <vm>`
///
/// Enable or disable System Integrity Protection on a macOS Apple Silicon VM
/// without any human interaction, by driving the recoveryOS UI over VNC.
///
/// Design notes
/// ------------
/// SIP on Apple Silicon is governed by a signed LocalPolicy that only the
/// Secure Enclave (or, in a VZ VM, the host-simulated SEP kexts) can mint.
/// The only supported way to ask that signer for a permissive policy is to
/// run `csrutil disable` from the recoveryOS Terminal — recovery has no
/// headless command channel (no sshd, no serial getty, sealed launchd job
/// table). So this command boots the VM into recovery and drives the fixed,
/// deterministic recoveryOS UI over its VNC console:
///
///   1. picker         : Right, Right, Enter                   (focus Options → Continue)
///   2. language        : Enter                                (accept default English)
///   3. Utilities menu : click (x=243, y=14)                    (open menu; Cmd-shortcuts
///                                                              don't route through VZ VNC)
///   4. menu           : Down, Down, Down, Enter                (highlight Terminal, launch)
///   5. Terminal       : type "csrutil disable" / "enable"     (+ y + admin password)
///   6. Terminal       : type "shutdown -h now"                 (clean stop → policy flushed)
///
/// Only step 3 uses a pixel coordinate. It clicks the menu bar at y=14, which
/// is stable across VZ framebuffer sizes — the menu bar is always top-anchored,
/// and "Utilities" is the 4th item at a fixed offset from the Apple logo. All
/// other input is keyboard-only.
///
/// Prerequisite: `vncdotool` on PATH (`pip3 install vncdotool`).
///
/// TODO(native-rfb): swap `VNCDriver` (vncdotool shell-out) for the in-tree
/// `VNCClient` native RFB actor to drop the Python dep. A first attempt in
/// this branch hit an NWConnection stall when awaited from `@MainActor`;
/// needs investigation before adopting here.
struct Sip: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "sip",
        abstract: "Enable or disable System Integrity Protection on a macOS VM",
        discussion: """
            Boots the VM into recovery, drives the recoveryOS Terminal over VNC
            to run `csrutil disable` (or `enable`) with the given admin
            credentials, and shuts the VM down cleanly so the signed policy is
            persisted. Then boots the VM normally once to verify.

            The VM must be stopped. It must have an admin account already
            provisioned (the default `lume`/`lume` unattended-setup account).

            Prerequisite: vncdotool (pip3 install vncdotool).

            Examples:
              lume sip off my-vm --yes
              lume sip on  my-vm --yes --admin-user alice --admin-password s3cret
            """
    )

    enum State: String, ExpressibleByArgument {
        case on, off
    }

    @Argument(help: "Desired SIP state: on or off")
    var state: State

    @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
    var name: String

    @Flag(name: [.short, .long], help: "Skip the interactive confirmation prompt")
    var yes: Bool = false

    @Option(name: .customLong("admin-user"), help: "Admin username in the guest (default: lume)")
    var adminUser: String = "lume"

    @Option(name: .customLong("admin-password"), help: "Admin password in the guest (default: lume)")
    var adminPassword: String = "lume"

    @Option(name: .customLong("screenshot-dir"), help: "Save step-by-step framebuffer PNGs here for debugging")
    var screenshotDir: String?

    @Option(name: .customLong("vnc-port"), help: "TCP port for the transient recovery VNC server (default: 5999)")
    var vncPort: Int = 5999

    @Option(name: .customLong("storage"), help: "VM storage location")
    var storage: String?

    @Option(help: "Overall timeout in seconds (default: 900)")
    var timeout: Int = 900

    @MainActor
    func run() async throws {
        TelemetryClient.shared.record(event: "lume_sip")

        let controller = LumeController()

        // 1. validate VM exists and is stopped
        let details: VMDetails
        do {
            details = try controller.getDetails(name: name, storage: storage)
        } catch {
            throw SipError.vmNotFound(name)
        }
        guard details.status == "stopped" else {
            throw SipError.vmNotStopped(name, actual: details.status)
        }

        // 2. confirmation
        if !yes {
            let verb = state == .off ? "DISABLE" : "ENABLE"
            print("This will \(verb) System Integrity Protection on '\(name)'.")
            print("The VM will be booted into recovery, driven via VNC, and rebooted.")
            print("Continue? [y/N]: ", terminator: "")
            let answer = readLine()?.trimmingCharacters(in: .whitespaces).lowercased() ?? ""
            guard answer == "y" || answer == "yes" else {
                print("Aborted.")
                throw ExitCode(1)
            }
        }

        // 3. vncdotool preflight
        guard let vncdo = VNCDriver.locateVncdo() else {
            throw SipError.vncdoNotFound
        }
        Logger.info("Using vncdotool", metadata: ["path": vncdo])

        // 4. spawn `lume run --recovery-mode true` as a child process
        let vncPassword = "lume-sip-\(UUID().uuidString.prefix(8))"
        let selfPath = ProcessInfo.processInfo.arguments[0]
        let selfExecutable: String = {
            if selfPath.hasPrefix("/") { return selfPath }
            return which(selfPath) ?? selfPath
        }()
        Logger.info(
            "Booting recovery",
            metadata: ["vm": name, "vnc_port": "\(vncPort)"])

        let runProc = Process()
        runProc.executableURL = URL(fileURLWithPath: selfExecutable)
        var runArgs = [
            "run", name,
            "--no-display",
            "--recovery-mode", "true",
            "--vnc-port", "\(vncPort)",
            "--vnc-password", vncPassword,
        ]
        if let storage = storage { runArgs.append(contentsOf: ["--storage", storage]) }
        runProc.arguments = runArgs
        runProc.standardOutput = Pipe()
        runProc.standardError = Pipe()
        try runProc.run()
        defer {
            if runProc.isRunning {
                runProc.terminate()
            }
        }

        // 5. drive the UI
        let driver = VNCDriver(
            vncdo: vncdo,
            host: "127.0.0.1",
            port: vncPort,
            password: vncPassword,
            screenshotDir: screenshotDir
        )
        try await driver.waitForListening(deadlineSeconds: 60)
        try await driver.waitForFramebuffer(minSize: 15_000, deadlineSeconds: 90, label: "picker")

        // picker: Right, Right (focus Options), Enter (Continue)
        try driver.key("right"); try await sleep(seconds: 1)
        try driver.key("right"); try await sleep(seconds: 1)
        try driver.snapshot("01-options-focused")
        try driver.key("enter"); try await sleep(seconds: 3)

        // Language selector renders as ~140 KB PNG (picker was ~30 KB).
        try await driver.waitForFramebuffer(minSize: 60_000, deadlineSeconds: 180, label: "language")
        // Default (English) is highlighted; Enter accepts and transitions to
        // the recoveryOS main menu (~30-45 s).
        try driver.key("enter"); try await sleep(seconds: 45)
        try driver.snapshot("02-recoveryOS-menu")

        // Open Utilities menu → Terminal. Cmd-key chords are not reliably
        // routed through VZ's VNC server via vncdotool, so we click the menu
        // bar (fixed y=14 anchor) then navigate the dropdown with arrows.
        try driver.click(x: 243, y: 14)
        try await sleep(seconds: 1)
        try driver.key("down"); try await sleep(seconds: 0.2)
        try driver.key("down"); try await sleep(seconds: 0.2)
        try driver.key("down"); try await sleep(seconds: 0.2)
        try driver.snapshot("03-terminal-highlighted")
        try driver.key("enter"); try await sleep(seconds: 4)
        try driver.snapshot("04-terminal-opened")

        // Type the csrutil command (unshifted chars only — vncdotool `type`
        // corrupts shifted characters against macOS VNC servers).
        let csrCommand = state == .off ? "csrutil disable" : "csrutil enable"
        try driver.type(csrCommand)
        try driver.key("enter")
        try await sleep(seconds: 3)
        try driver.snapshot("05-csrutil-prompt")

        // y/n prompt → y
        try driver.key("y")
        try driver.key("enter")
        try await sleep(seconds: 3)
        try driver.snapshot("06-password-prompt")

        // "Enter password for user <admin>:" → type password (blind, no echo)
        try driver.type(adminPassword)
        try driver.key("enter")
        try await sleep(seconds: 4)
        try driver.snapshot("07-after-password")

        // Clean shutdown so signed policy is flushed to the aux storage.
        try driver.type("shutdown -h now")
        try driver.key("enter")
        try driver.snapshot("08-shutdown-sent")

        // 6. wait for child (lume run) to exit — VM is halting
        Logger.info("Waiting for recovery to shut down")
        try await waitForProcessExit(runProc, deadlineSeconds: 180)

        // If VZ hasn't marked the VM stopped after the child exited, its
        // config lock will still be held by SOME process; the guest just
        // halted. We deliberately do NOT call `controller.stopVM` here —
        // that helper resolves the lock-holding process via lsof and would
        // find (and kill) this sip process itself, because Foundation may
        // still hold an fd to the config file we read in getDetails().
        // Give the child's cleanup a moment, then move on.
        try await sleep(seconds: 3)

        // 7. verify: boot normally and read csrutil status
        Logger.info("Booting normally to verify SIP state")
        let verifyProc = Process()
        verifyProc.executableURL = URL(fileURLWithPath: selfExecutable)
        var verifyArgs = ["run", name, "--no-display"]
        if let storage = storage { verifyArgs.append(contentsOf: ["--storage", storage]) }
        verifyProc.arguments = verifyArgs
        verifyProc.standardOutput = Pipe()
        verifyProc.standardError = Pipe()
        try verifyProc.run()
        defer {
            if verifyProc.isRunning {
                verifyProc.terminate()
            }
        }

        // Wait for SSH.
        var verifiedDetails: VMDetails?
        let sshDeadline = Date().addingTimeInterval(300)
        while Date() < sshDeadline {
            try await sleep(seconds: 5)
            if let d = try? controller.getDetails(name: name, storage: storage),
                d.status == "running", d.sshAvailable == true,
                let ip = d.ipAddress, !ip.isEmpty
            {
                verifiedDetails = d
                break
            }
        }
        guard let vd = verifiedDetails, let ip = vd.ipAddress else {
            throw SipError.verificationFailed("VM never reached SSH-ready state")
        }

        let ssh = SSHClient(host: ip, port: 22, user: adminUser, password: adminPassword)
        let result = try await ssh.execute(command: "csrutil status", timeout: 30)
        let statusOutput = result.output.trimmingCharacters(in: .whitespacesAndNewlines)
        print(statusOutput)

        // Try a clean shutdown before returning.
        _ = try? await ssh.execute(
            command: "echo \(adminPassword) | sudo -S shutdown -h now",
            timeout: 10
        )

        let expected = state == .off ? "disabled" : "enabled"
        if statusOutput.lowercased().contains(expected) {
            print("SIP is \(expected).")
        } else {
            throw SipError.verificationFailed(
                "Unexpected csrutil status after \(state.rawValue): \(statusOutput)")
        }
    }

    // Helpers

    private func sleep(seconds: Double) async throws {
        try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }

    private func waitForProcessExit(_ p: Process, deadlineSeconds: Double) async throws {
        let deadline = Date().addingTimeInterval(deadlineSeconds)
        while p.isRunning && Date() < deadline {
            try await sleep(seconds: 2)
        }
        if p.isRunning {
            p.terminate()
        }
    }

    private func which(_ name: String) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        p.arguments = [name]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = Pipe()
        try? p.run()
        p.waitUntilExit()
        guard let d = try? out.fileHandleForReading.readToEnd(),
            let s = String(data: d, encoding: .utf8)
        else { return nil }
        let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

enum SipError: Error, CustomStringConvertible {
    case vmNotFound(String)
    case vmNotStopped(String, actual: String)
    case vncdoNotFound
    case verificationFailed(String)

    var description: String {
        switch self {
        case .vmNotFound(let name):
            return "VM '\(name)' not found."
        case .vmNotStopped(let name, let actual):
            return "VM '\(name)' must be stopped (current status: \(actual))."
        case .vncdoNotFound:
            return "vncdotool not found on PATH. Install with: pip3 install vncdotool"
        case .verificationFailed(let msg):
            return "SIP verification failed: \(msg)"
        }
    }
}
