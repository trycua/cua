import ArgumentParser
import Foundation

struct Clipboard: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "clipboard",
        abstract: "Sync clipboard between host and VM",
        discussion: """
            Synchronize clipboard content between the macOS host and a running VM
            using SSH. Requires SSH/Remote Login to be enabled in the VM.

            Examples:
              lume clipboard push my-vm  # Copy host clipboard to VM
              lume clipboard pull my-vm  # Copy VM clipboard to host
            """,
        subcommands: [Push.self, Pull.self]
    )

    struct Push: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "push",
            abstract: "Push host clipboard to VM"
        )

        @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
        var name: String

        @Option(name: [.short, .long], help: "SSH username (default: lume)")
        var user: String = "lume"

        @Option(name: [.short, .long], help: "SSH password (default: lume)")
        var password: String = "lume"

        @Option(name: .customLong("storage"), help: "Storage location name or path")
        var storage: String?

        @MainActor
        func run() async throws {
            TelemetryClient.shared.record(event: "lume_clipboard_push")

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

            // Create SSH client and clipboard sync
            let sshClient = SSHClient(
                host: ipAddress,
                port: 22,
                user: user,
                password: password
            )
            let clipboardSync = ClipboardSync(sshClient: sshClient)

            let result = try await clipboardSync.push()
            print(result)
        }
    }

    struct Pull: AsyncParsableCommand {
        static let configuration = CommandConfiguration(
            commandName: "pull",
            abstract: "Pull VM clipboard to host"
        )

        @Argument(help: "Name of the virtual machine", completion: .custom(completeVMName))
        var name: String

        @Option(name: [.short, .long], help: "SSH username (default: lume)")
        var user: String = "lume"

        @Option(name: [.short, .long], help: "SSH password (default: lume)")
        var password: String = "lume"

        @Option(name: .customLong("storage"), help: "Storage location name or path")
        var storage: String?

        @MainActor
        func run() async throws {
            TelemetryClient.shared.record(event: "lume_clipboard_pull")

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

            // Create SSH client and clipboard sync
            let sshClient = SSHClient(
                host: ipAddress,
                port: 22,
                user: user,
                password: password
            )
            let clipboardSync = ClipboardSync(sshClient: sshClient)

            let result = try await clipboardSync.pull()
            print(result)
        }
    }
}
