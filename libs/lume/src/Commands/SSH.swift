import ArgumentParser
import Foundation

struct SSH: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "ssh",
        abstract: "Connect to a VM via SSH or execute commands remotely",
        discussion: """
            Connect to a running VM via SSH. Can be used for interactive sessions
            or to execute a single command.

            This command handles password authentication automatically, so you don't
            need sshpass installed. VMs created with --unattended use credentials
            lume/lume by default.

            Examples:
              lume ssh my-vm                       # Interactive shell
              lume ssh my-vm "ls -la"              # Execute command
              lume ssh my-vm "cd /app && npm test" # Execute complex command
            """
    )

    @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
    var name: String

    @Argument(parsing: .remaining, help: "Command to execute (omit for interactive shell)")
    var command: [String] = []

    @Option(name: [.short, .long], help: "SSH username (default: lume)")
    var user: String = "lume"

    @Option(name: [.short, .long], help: "SSH password (default: lume)")
    var password: String = "lume"

    @Option(name: .customLong("storage"), help: "Storage location name or path")
    var storage: String?

    @Option(name: [.short, .long], help: "Command timeout in seconds (0 for no timeout, default: 60)")
    var timeout: Int = 60

    @MainActor
    func run() async throws {
        // Record telemetry
        TelemetryClient.shared.record(event: "lume_ssh")

        let controller = LumeController()

        // Get VM details
        let vmDetails: VMDetails
        do {
            vmDetails = try controller.getDetails(name: name, storage: storage)
        } catch {
            throw SSHError.vmNotFound(name)
        }

        // Validate VM state
        guard vmDetails.status == "running" else {
            throw SSHError.vmNotRunning(name)
        }

        guard let ipAddress = vmDetails.ipAddress, !ipAddress.isEmpty else {
            throw SSHError.noIPAddress(name)
        }

        guard vmDetails.sshAvailable == true else {
            throw SSHError.sshNotAvailable(name)
        }

        // Try NIO SSH first, fall back to system SSH on connection failure
        do {
            try await executeWithNIOSSH(host: ipAddress)
        } catch let error as SSHError {
            // Only fall back on connection errors (not auth failures, timeouts, etc.)
            if case .connectionFailed = error {
                Logger.debug(
                    "NIO SSH connection failed, falling back to system SSH",
                    metadata: ["host": "\(ipAddress)"])
                try executeWithSystemSSH(host: ipAddress)
            } else {
                throw error
            }
        }
    }

    /// Execute using the built-in NIO SSH client
    @MainActor
    private func executeWithNIOSSH(host: String) async throws {
        let sshClient = SSHClient(
            host: host,
            port: 22,
            user: user,
            password: password
        )

        if command.isEmpty {
            try await sshClient.interactive()
        } else {
            let fullCommand = command.joined(separator: " ")
            let result = try await sshClient.execute(
                command: fullCommand,
                timeout: TimeInterval(timeout)
            )

            if !result.output.isEmpty {
                print(result.output)
            }

            if result.exitCode != 0 {
                throw ExitCode(result.exitCode)
            }
        }
    }

    /// Execute using the system /usr/bin/ssh binary (fallback for restricted environments)
    private func executeWithSystemSSH(host: String) throws {
        let systemClient = SystemSSHClient(
            host: host,
            port: 22,
            user: user,
            password: password
        )

        if command.isEmpty {
            try systemClient.interactive()
        } else {
            let fullCommand = command.joined(separator: " ")
            let result = try systemClient.execute(
                command: fullCommand,
                timeout: TimeInterval(timeout)
            )

            if !result.output.isEmpty {
                print(result.output)
            }

            if result.exitCode != 0 {
                throw ExitCode(result.exitCode)
            }
        }
    }
}
