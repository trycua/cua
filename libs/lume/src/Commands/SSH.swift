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

    @Argument(parsing: .captureForPassthrough, help: "Command to execute (omit for interactive shell)")
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

        // Create SSH client
        let sshClient = SSHClient(
            host: ipAddress,
            port: 22,
            user: user,
            password: password
        )

        if command.isEmpty {
            // Interactive mode
            try await sshClient.interactive()
        } else {
            // Execute command
            let fullCommand = command.joined(separator: " ")
            let result = try await sshClient.execute(
                command: fullCommand,
                timeout: TimeInterval(timeout)
            )

            // Print output
            if !result.output.isEmpty {
                print(result.output)
            }

            // Exit with same code as remote command
            if result.exitCode != 0 {
                throw ExitCode(result.exitCode)
            }
        }
    }
}
