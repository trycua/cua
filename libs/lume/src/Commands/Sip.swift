import ArgumentParser
import Darwin
import Foundation

/// `lume sip <on|off> <vm>`
///
/// Changes SIP from paired recoveryOS, where Apple permits LocalPolicy
/// downgrades. Recovery has no supported command channel, so the workflow
/// drives its Terminal over the VM's transient VNC server and then verifies
/// the result over SSH after a normal boot.
struct Sip: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "sip", abstract: "Enable or disable System Integrity Protection on a macOS VM",
        discussion: """
            Boots a stopped VM normally to validate the admin credentials, then
            boots paired recoveryOS and runs `csrutil disable` (or `enable`) in
            Terminal over VNC. After confirming the signed policy result, Lume
            stops Recovery and boots normally once more to verify the result.

            The VM must already have an admin account and Remote Login enabled.
            VMs created with unattended setup use lume/lume by default.

            Prefer --admin-password-stdin so the guest password does not appear
            in the Lume process arguments. --admin-password remains available
            for interactive convenience but can be observed by other processes.

            Prerequisite: vncdotool (pip3 install vncdotool).

            Examples:
              lume sip off my-vm --yes
              printf 'secret\\n' | lume sip on my-vm --yes \\
                --admin-user alice --admin-password-stdin
            """)

    enum State: String, ExpressibleByArgument, Sendable { case on, off }

    @Argument(help: "Desired SIP state: on or off") var state: State

    @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName)) var name: String

    @Flag(name: [.short, .long], help: "Skip the interactive confirmation prompt") var yes: Bool = false

    @Option(name: .customLong("admin-user"), help: "Admin username in the guest") var adminUser: String = "lume"

    @Option(
        name: .customLong("admin-password"),
        help: "Admin password in the guest. Prefer --admin-password-stdin; defaults to lume") var adminPassword: String?

    @Flag(
        name: .customLong("admin-password-stdin"), help: "Read one admin-password line from standard input without echo"
    ) var adminPasswordStdin: Bool = false

    @Option(name: .customLong("screenshot-dir"), help: "Save step-by-step framebuffer PNGs here for debugging")
    var screenshotDir: String?

    @Option(name: .customLong("vnc-port"), help: "TCP port for the transient recovery VNC server")
    var vncPort: Int = 5999

    @Option(name: .customLong("storage"), help: "VM storage location") var storage: String?

    @Option(help: "Overall timeout in seconds") var timeout: Int = 900

    @MainActor func run() async throws {
        TelemetryClient.shared.record(event: "lume_sip")
        guard timeout > 0 else { throw SipError.invalidTimeout(timeout) }

        let controller = LumeController()
        let details = try loadInitialDetails(controller: controller)
        try Self.validateInitialState(details, expectedName: name)

        if !yes { try confirmChange() }

        let password = try resolvedAdminPassword()
        try Self.validateVNCPasswordCharacters(password)
        let vncdo = try Self.requireVNCExecutable(VNCDriver.locateVncdo())
        Logger.info("Using vncdotool", metadata: ["path": vncdo])

        let selfExecutable = resolvedSelfExecutable()
        let deadline = SipDeadline(seconds: timeout)

        try await preflightCredentials(
            controller: controller, executable: selfExecutable, password: password, deadline: deadline)
        try await changePolicyInRecovery(
            controller: controller, executable: selfExecutable, vncdo: vncdo, password: password, deadline: deadline)
        try await verifyPolicy(
            controller: controller, executable: selfExecutable, password: password, deadline: deadline)
    }

    @MainActor private func preflightCredentials(
        controller: LumeController, executable: String, password: String, deadline: SipDeadline
    ) async throws {
        Logger.info("Booting normally to validate admin credentials", metadata: ["vm": name])
        let child = try SipChildProcess(
            executable: executable, arguments: runArguments(recovery: false), label: "credential-preflight")

        do {
            try child.start()
            let details = try await waitForSSHReady(
                controller: controller, child: child, context: "credential preflight", deadline: deadline,
                maxSeconds: 300)
            guard let ip = details.ipAddress else {
                throw SipError.credentialPreflightFailed("VM did not report an IP address")
            }

            let ssh = SSHClient(host: ip, port: 22, user: adminUser, password: password)
            do {
                let result = try await ssh.execute(
                    command: "true", timeout: try deadline.remaining(upTo: 30, operation: "credential preflight"))
                guard result.exitCode == 0 else {
                    throw SipError.credentialPreflightFailed("SSH command exited with status \(result.exitCode)")
                }
            } catch let error as SipError { throw error } catch {
                throw SipError.credentialPreflightFailed(
                    "SSH authentication failed for '\(adminUser)': \(error.localizedDescription)")
            }

            Logger.info("Admin credential preflight passed", metadata: ["vm": name])
            try await stopNormalBoot(
                child: child, controller: controller, deadline: deadline, operation: "credential preflight")
            child.cleanup()
        } catch {
            await child.terminateAndWait()
            child.cleanup()
            throw error
        }
    }

    @MainActor private func changePolicyInRecovery(
        controller: LumeController, executable: String, vncdo: String, password: String, deadline: SipDeadline
    ) async throws {
        let vncPassword = Self.makeVNCPassword()
        Logger.info("Booting recovery", metadata: ["vm": name, "vnc_port": "\(vncPort)"])
        let child = try SipChildProcess(
            executable: executable, arguments: runArguments(recovery: true, vncPassword: vncPassword), label: "recovery"
        )

        do {
            try child.start()
            let driver = VNCDriver(
                vncdo: vncdo, host: "127.0.0.1", port: vncPort, password: vncPassword, screenshotDir: screenshotDir)
            let heartbeat = { try child.ensureRunning(context: "recovery boot", redacting: [vncPassword]) }
            let automator = SipRecoveryAutomator()
            try await automator.perform(
                driver: driver, state: state, adminUser: adminUser, adminPassword: password, heartbeat: heartbeat,
                timeout: { limit, operation in try deadline.remaining(upTo: limit, operation: operation) })

            Logger.info("Waiting for recovery to shut down")
            do {
                try await waitForProcessExit(
                    child, deadline: deadline, maxSeconds: 60,
                    context: "recovery shutdown; SIP state may not have been committed", redacting: [vncPassword])
            } catch SipError.childProcessTimedOut {
                Logger.info(
                    "Recovery did not halt after committing the policy; stopping its run process",
                    metadata: ["vm": name])
                await child.terminateAndWait()
                guard !child.isRunning else {
                    throw SipError.childProcessTimedOut(
                        context: "recovery shutdown fallback", output: child.outputTail(redacting: [vncPassword]))
                }
            }
            try await waitForStoppedState(controller: controller, deadline: deadline, operation: "recovery shutdown")
            child.cleanup()
        } catch {
            await child.terminateAndWait()
            child.cleanup()
            throw error
        }
    }

    @MainActor private func verifyPolicy(
        controller: LumeController, executable: String, password: String, deadline: SipDeadline
    ) async throws {
        Logger.info("Booting normally to verify SIP state")
        let child = try SipChildProcess(
            executable: executable, arguments: runArguments(recovery: false), label: "verification")

        do {
            try child.start()
            let details = try await waitForSSHReady(
                controller: controller, child: child, context: "verification boot", deadline: deadline, maxSeconds: 300)
            guard let ip = details.ipAddress else {
                throw SipError.verificationFailed("VM did not report an IP address")
            }

            let ssh = SSHClient(host: ip, port: 22, user: adminUser, password: password)
            let result = try await ssh.execute(
                command: "csrutil status", timeout: try deadline.remaining(upTo: 30, operation: "SIP verification"))
            let statusOutput = result.output.trimmingCharacters(in: .whitespacesAndNewlines)
            let matches = result.exitCode == 0 && Self.statusMatches(statusOutput, expected: state)
            print(statusOutput)

            try await stopNormalBoot(
                child: child, controller: controller, deadline: deadline, operation: "verification")

            guard result.exitCode == 0 else {
                throw SipError.verificationFailed("csrutil status exited with status \(result.exitCode): \(statusOutput)")
            }
            guard matches else {
                throw SipError.verificationFailed("Unexpected csrutil status after \(state.rawValue): \(statusOutput)")
            }
            print("SIP is \(state == .off ? "disabled" : "enabled").")
            child.cleanup()
        } catch {
            await child.terminateAndWait()
            child.cleanup()
            throw error
        }
    }

    @MainActor private func waitForSSHReady(
        controller: LumeController, child: SipChildProcess, context: String, deadline: SipDeadline, maxSeconds: Double
    ) async throws -> VMDetails {
        let allowed = try deadline.remaining(upTo: maxSeconds, operation: context)
        let localDeadline = ContinuousClock.now.advanced(by: .seconds(allowed))
        while ContinuousClock.now < localDeadline {
            try child.ensureRunning(context: context, redacting: [])
            if let details = try? controller.getDetails(name: name, storage: storage), details.status == "running",
                details.sshAvailable == true, let ip = details.ipAddress, !ip.isEmpty
            {
                return details
            }
            try await sleep(seconds: min(5, try deadline.remaining(upTo: 5, operation: context)))
        }
        throw SipError.verificationFailed(
            "VM never reached SSH-ready state during \(context). " + "Ensure Remote Login is enabled.")
    }

    @MainActor private func stopNormalBoot(
        child: SipChildProcess, controller: LumeController, deadline: SipDeadline, operation: String
    ) async throws {
        Logger.info("Stopping normal boot", metadata: ["operation": operation, "vm": name])
        await child.terminateAndWait()
        guard !child.isRunning else {
            throw SipError.childProcessTimedOut(
                context: "\(operation) stop", output: child.outputTail(redacting: []))
        }
        try await waitForStoppedState(controller: controller, deadline: deadline, operation: "\(operation) stop")
    }

    @MainActor private func waitForProcessExit(
        _ child: SipChildProcess, deadline: SipDeadline, maxSeconds: Double, context: String,
        redacting secrets: [String]
    ) async throws {
        let allowed = try deadline.remaining(upTo: maxSeconds, operation: context)
        let localDeadline = ContinuousClock.now.advanced(by: .seconds(allowed))
        while child.isRunning && ContinuousClock.now < localDeadline { try await sleep(seconds: 1) }
        guard !child.isRunning else {
            throw SipError.childProcessTimedOut(context: context, output: child.outputTail(redacting: secrets))
        }
        guard child.terminationStatus == 0 else {
            throw SipError.childProcessExited(
                context: context, status: child.terminationStatus, output: child.outputTail(redacting: secrets))
        }
    }

    @MainActor private func waitForStoppedState(controller: LumeController, deadline: SipDeadline, operation: String)
        async throws
    {
        let allowed = try deadline.remaining(upTo: 30, operation: operation)
        let localDeadline = ContinuousClock.now.advanced(by: .seconds(allowed))
        while ContinuousClock.now < localDeadline {
            if let details = try? controller.getDetails(name: name, storage: storage), details.status == "stopped" {
                return
            }
            try await sleep(seconds: 1)
        }
        throw SipError.vmDidNotStop(name, operation: operation)
    }

    @MainActor private func loadInitialDetails(controller: LumeController) throws -> VMDetails {
        do { return try controller.getDetails(name: name, storage: storage) } catch { throw SipError.vmNotFound(name) }
    }

    private func confirmChange() throws {
        let verb = state == .off ? "DISABLE" : "ENABLE"
        print("This will \(verb) System Integrity Protection on '\(name)'.")
        print("The VM will boot normally for credential validation, then into recovery.")
        print("Continue? [y/N]: ", terminator: "")
        let answer = readLine()?.trimmingCharacters(in: .whitespaces).lowercased() ?? ""
        guard answer == "y" || answer == "yes" else {
            print("Aborted.")
            throw ExitCode(1)
        }
    }

    private func resolvedAdminPassword() throws -> String {
        if adminPassword != nil && adminPasswordStdin { throw SipError.conflictingPasswordSources }
        let password: String
        if adminPasswordStdin {
            password = try readSecretLineFromStandardInput()
        } else {
            password = adminPassword ?? "lume"
        }
        guard !password.isEmpty else { throw SipError.emptyPassword }
        return password
    }

    private func readSecretLineFromStandardInput() throws -> String {
        let fd = STDIN_FILENO
        var original = termios()
        let isTerminal = isatty(fd) == 1
        var echoDisabled = false

        if isTerminal {
            guard tcgetattr(fd, &original) == 0 else {
                throw SipError.passwordTerminalConfigurationFailed
            }
            var noEcho = original
            noEcho.c_lflag &= ~tcflag_t(ECHO)
            guard tcsetattr(fd, TCSAFLUSH, &noEcho) == 0 else {
                throw SipError.passwordTerminalConfigurationFailed
            }
            echoDisabled = true
            FileHandle.standardError.write(Data("Admin password: ".utf8))
        }
        defer {
            if echoDisabled {
                var restored = original
                tcsetattr(fd, TCSAFLUSH, &restored)
                FileHandle.standardError.write(Data("\n".utf8))
            }
        }

        guard let password = readLine(strippingNewline: true) else { throw SipError.passwordStdinUnavailable }
        return password
    }

    private func resolvedSelfExecutable() -> String {
        let selfPath = ProcessInfo.processInfo.arguments[0]
        if selfPath.hasPrefix("/") { return selfPath }
        return which(selfPath) ?? selfPath
    }

    private func runArguments(recovery: Bool, vncPassword: String? = nil) -> [String] {
        var args = ["run", name, "--no-display"]
        if recovery { args.append(contentsOf: ["--recovery-mode", "true"]) }
        if let vncPassword { args.append(contentsOf: ["--vnc-port", "\(vncPort)", "--vnc-password", vncPassword]) }
        if let storage { args.append(contentsOf: ["--storage", storage]) }
        return args
    }

    private func sleep(seconds: Double) async throws { try await Task.sleep(for: .seconds(seconds)) }

    private func which(_ name: String) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        process.arguments = [name]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try? process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0, let data = try? output.fileHandleForReading.readToEnd(),
            let path = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
            !path.isEmpty
        else { return nil }
        return path
    }

    static func validateInitialState(_ details: VMDetails, expectedName: String) throws {
        guard details.name == expectedName else { throw SipError.vmNotFound(expectedName) }
        guard details.os.lowercased() == "macos" else { throw SipError.unsupportedOS(details.os) }
        guard details.status == "stopped" else { throw SipError.vmNotStopped(expectedName, actual: details.status) }
    }

    static func requireVNCExecutable(_ path: String?) throws -> String {
        guard let path else { throw SipError.vncdoNotFound }
        return path
    }

    static func makeVNCPassword() -> String {
        // Classic VNC authentication uses only the first eight password bytes.
        let characters = Array("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
        var generator = SystemRandomNumberGenerator()
        return String((0..<8).map { _ in
            characters[Int.random(in: characters.indices, using: &generator)]
        })
    }

    static func statusMatches(_ output: String, expected state: State) -> Bool {
        let prefix = "system integrity protection status:"
        guard let statusLine = output.split(whereSeparator: \.isNewline).map({
            $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        }).first(where: { $0.hasPrefix(prefix) }) else {
            return false
        }
        let value = statusLine.dropFirst(prefix.count).trimmingCharacters(in: .whitespaces)
        switch state {
        case .off: return value == "disabled" || value == "disabled."
        case .on: return value == "enabled" || value == "enabled."
        }
    }

    static func validateVNCPasswordCharacters(_ password: String) throws {
        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789-")
        guard password.unicodeScalars.allSatisfy(allowed.contains) else { throw SipError.unsupportedPasswordCharacters }
    }
}

@MainActor struct SipRecoveryAutomator {
    typealias Sleeper = (Double) async throws -> Void
    typealias Timeout = (Double, String) throws -> Double

    private let sleeper: Sleeper

    init(sleeper: @escaping Sleeper = { seconds in try await Task.sleep(for: .seconds(seconds)) }) {
        self.sleeper = sleeper
    }

    func perform(
        driver: any RecoveryVNCDriving, state: Sip.State, adminUser: String, adminPassword: String,
        heartbeat: () throws -> Void,
        timeout: Timeout
    ) async throws {
        try await driver.waitForListening(deadlineSeconds: try timeout(60, "VNC startup"), heartbeat: heartbeat)
        try await driver.waitForFramebuffer(
            minSize: 15_000, deadlineSeconds: try timeout(90, "recovery picker"), label: "picker", heartbeat: heartbeat)

        try driver.key("right")
        try await pause(seconds: 1, operation: "recovery picker", heartbeat: heartbeat, timeout: timeout)
        try driver.key("right")
        try await pause(seconds: 1, operation: "recovery picker", heartbeat: heartbeat, timeout: timeout)
        try driver.snapshot("01-options-focused")
        try driver.key("enter")
        try await pause(seconds: 3, operation: "recovery picker", heartbeat: heartbeat, timeout: timeout)

        try await driver.waitForFramebuffer(
            minSize: 60_000, deadlineSeconds: try timeout(180, "recovery language selector"), label: "language",
            heartbeat: heartbeat)
        try driver.key("enter")
        try await pause(
            seconds: 45, operation: "recoveryOS main-menu transition", heartbeat: heartbeat, timeout: timeout)
        try driver.snapshot("02-recoveryOS-menu")

        try driver.click(x: 243, y: 14)
        try await pause(seconds: 1, operation: "Utilities menu", heartbeat: heartbeat, timeout: timeout)
        for _ in 0..<3 {
            try driver.key("down")
            try await pause(seconds: 0.2, operation: "Utilities menu", heartbeat: heartbeat, timeout: timeout)
        }
        try driver.snapshot("03-terminal-highlighted")
        try driver.key("enter")
        try await pause(seconds: 4, operation: "Terminal launch", heartbeat: heartbeat, timeout: timeout)
        try driver.snapshot("04-terminal-opened")

        try driver.type(state == .off ? "csrutil disable" : "csrutil enable")
        try driver.key("enter")
        try await pause(seconds: 3, operation: "csrutil prompt", heartbeat: heartbeat, timeout: timeout)
        try driver.snapshot("05-csrutil-prompt")

        let accountPrompt = "enter password for user \(adminUser):"
        let promptText = try await driver.waitForText(
            containing: ["[y/n]", accountPrompt],
            rejecting: ["no administrator was found", "failed to authenticate", "authentication failure"],
            deadlineSeconds: try timeout(15, "csrutil prompt"), label: "csrutil prompt", heartbeat: heartbeat)

        if !promptText.lowercased().contains(accountPrompt.lowercased()) {
            try driver.key("y")
            try driver.key("enter")
            try await pause(seconds: 3, operation: "csrutil password prompt", heartbeat: heartbeat, timeout: timeout)
            _ = try await driver.waitForText(
                containing: [accountPrompt],
                rejecting: ["no administrator was found", "failed to authenticate", "authentication failure"],
                deadlineSeconds: try timeout(15, "csrutil account prompt"), label: "csrutil account prompt",
                heartbeat: heartbeat)
        }
        try driver.snapshot("06-password-prompt")

        try driver.typeSensitive(adminPassword)
        try driver.key("enter")
        let expected = state == .off ? "system integrity protection is off." : "system integrity protection is on."
        _ = try await driver.waitForText(
            containing: [expected],
            rejecting: ["incorrect password", "authentication failed", "failed to modify", "try again"],
            deadlineSeconds: try timeout(20, "csrutil result"), label: "csrutil result", heartbeat: heartbeat)
        try driver.snapshot("07-after-password")

        try driver.type("sync; shutdown -h now")
        try driver.key("enter")
        try driver.snapshot("08-shutdown-sent")
    }

    private func pause(seconds: Double, operation: String, heartbeat: () throws -> Void, timeout: Timeout) async throws
    {
        var remaining = seconds
        while remaining > 0 {
            try heartbeat()
            let interval = min(1, remaining)
            let allowed = try timeout(interval, operation)
            try await sleeper(min(interval, allowed))
            remaining -= interval
        }
        try heartbeat()
    }
}

struct SipDeadline: Sendable {
    private let clock = ContinuousClock()
    private let end: ContinuousClock.Instant

    init(seconds: Int) { end = clock.now.advanced(by: .seconds(seconds)) }

    func remaining(upTo limit: Double, operation: String) throws -> Double {
        let duration = clock.now.duration(to: end).components
        let seconds = Double(duration.seconds) + Double(duration.attoseconds) / 1e18
        guard seconds > 0 else { throw SipError.timedOut(operation) }
        return min(limit, seconds)
    }
}

@MainActor final class SipChildProcess {
    private static let maximumTailBytes = 16 * 1024

    let process = Process()
    private let logURL: URL
    private let logHandle: FileHandle

    init(executable: String, arguments: [String], label: String) throws {
        let safeLabel = label.replacingOccurrences(of: " ", with: "-")
        logURL = FileManager.default.temporaryDirectory.appendingPathComponent(
            "lume-sip-\(safeLabel)-\(UUID().uuidString).log")
        guard FileManager.default.createFile(atPath: logURL.path, contents: nil, attributes: [.posixPermissions: 0o600])
        else { throw SipError.processSetupFailed("Could not create child-process log") }
        logHandle = try FileHandle(forWritingTo: logURL)
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardOutput = logHandle
        process.standardError = logHandle
    }

    var isRunning: Bool { process.isRunning }

    var terminationStatus: Int32 { process.isRunning ? -1 : process.terminationStatus }

    func start() throws { try process.run() }

    func ensureRunning(context: String, redacting secrets: [String]) throws {
        guard process.isRunning else {
            throw SipError.childProcessExited(
                context: context, status: terminationStatus, output: outputTail(redacting: secrets))
        }
    }

    func outputTail(redacting secrets: [String]) -> String {
        try? logHandle.synchronize()
        guard let data = try? Data(contentsOf: logURL) else { return "" }
        let tail = data.suffix(Self.maximumTailBytes)
        var output = String(decoding: tail, as: UTF8.self)
        for secret in secrets where !secret.isEmpty {
            output = output.replacingOccurrences(of: secret, with: "<redacted>")
        }
        return output.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    func terminateAndWait() async {
        guard process.isRunning else { return }
        process.interrupt()
        await waitForTermination(seconds: 5)
        if process.isRunning {
            process.terminate()
            await waitForTermination(seconds: 5)
        }
        if process.isRunning {
            Darwin.kill(process.processIdentifier, SIGKILL)
            await waitForTermination(seconds: 2)
        }
    }

    func cleanup() {
        try? logHandle.close()
        try? FileManager.default.removeItem(at: logURL)
    }

    private func waitForTermination(seconds: Double) async {
        let deadline = ContinuousClock.now.advanced(by: .seconds(seconds))
        while process.isRunning && ContinuousClock.now < deadline { try? await Task.sleep(for: .milliseconds(100)) }
    }
}

enum SipError: Error, CustomStringConvertible {
    case vmNotFound(String)
    case vmNotStopped(String, actual: String)
    case vmDidNotStop(String, operation: String)
    case unsupportedOS(String)
    case vncdoNotFound
    case conflictingPasswordSources
    case passwordStdinUnavailable
    case passwordTerminalConfigurationFailed
    case emptyPassword
    case unsupportedPasswordCharacters
    case invalidTimeout(Int)
    case credentialPreflightFailed(String)
    case childProcessExited(context: String, status: Int32, output: String)
    case childProcessTimedOut(context: String, output: String)
    case processSetupFailed(String)
    case timedOut(String)
    case verificationFailed(String)

    var description: String {
        switch self {
        case .vmNotFound(let name): return "VM '\(name)' not found."
        case .vmNotStopped(let name, let actual): return "VM '\(name)' must be stopped (current status: \(actual))."
        case .vmDidNotStop(let name, let operation):
            return "VM '\(name)' did not reach stopped state after \(operation)."
        case .unsupportedOS(let os): return "The sip command supports macOS VMs only (found: \(os))."
        case .vncdoNotFound: return "vncdotool not found on PATH. Install with: pip3 install vncdotool"
        case .conflictingPasswordSources: return "Use either --admin-password or --admin-password-stdin, not both."
        case .passwordStdinUnavailable: return "Could not read the admin password from standard input."
        case .passwordTerminalConfigurationFailed:
            return "Could not disable terminal echo while reading the admin password."
        case .emptyPassword: return "The admin password cannot be empty."
        case .unsupportedPasswordCharacters:
            return "Recovery VNC currently supports admin passwords containing only "
                + "lowercase ASCII letters, digits, and hyphens."
        case .invalidTimeout(let timeout): return "Timeout must be greater than zero (received: \(timeout))."
        case .credentialPreflightFailed(let message): return "Admin credential preflight failed: \(message)"
        case .childProcessExited(let context, let status, let output):
            let suffix = output.isEmpty ? "" : "\n\(output)"
            return "The lume run child exited during \(context) with status \(status).\(suffix)"
        case .childProcessTimedOut(let context, let output):
            let suffix = output.isEmpty ? "" : "\n\(output)"
            return "Timed out waiting for the lume run child during \(context).\(suffix)"
        case .processSetupFailed(let message): return "Could not prepare the lume run child: \(message)"
        case .timedOut(let operation): return "The overall sip timeout expired during \(operation)."
        case .verificationFailed(let message): return "SIP verification failed: \(message)"
        }
    }
}
