import ArgumentParser
import Foundation

/// Copy files and directories between the host and a running VM.
///
/// `lume ssh` only runs remote commands, so moving a file into or out of a VM
/// previously required encoding it into the command line. `lume cp` performs the
/// transfer directly over the same SSH credentials `lume ssh` already uses.
struct Cp: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "cp",
        abstract: "Copy files between the host and a VM",
        discussion: """
            Copy a file or directory between the host and a running VM over SSH.
            Exactly one of the source or destination is a VM path, written as
            VM:PATH (the same credentials as `lume ssh` are used).

            Examples:
              lume cp ./build.zip my-vm:/tmp/build.zip   # host → guest
              lume cp my-vm:/tmp/out.log ./out.log       # guest → host
              lume cp -r ./assets my-vm:/tmp/assets      # copy a directory

            Directories are copied recursively automatically when the source is a
            host directory; pass --recursive for a recursive guest → host copy.
            """
    )

    @Argument(help: "Source path. Use VM:PATH for a path inside a VM.")
    var source: String

    @Argument(help: "Destination path. Use VM:PATH for a path inside a VM.")
    var destination: String

    @Option(name: [.short, .long], help: "SSH username (default: lume)")
    var user: String = "lume"

    @Option(name: [.short, .long], help: "SSH password (default: lume)")
    var password: String = "lume"

    @Option(name: .customLong("storage"), help: "Storage location name or path")
    var storage: String?

    @Flag(name: [.short, .long], help: "Copy directories recursively")
    var recursive: Bool = false

    @Option(
        name: [.short, .long],
        help: "Transfer timeout in seconds (0 for no timeout, default: 600)")
    var timeout: Int = 600

    @MainActor
    func run() async throws {
        TelemetryClient.shared.record(event: "lume_cp")

        let src = Self.parseEndpoint(source)
        let dst = Self.parseEndpoint(destination)

        switch (src, dst) {
        case (.local, .local):
            throw ValidationError(
                "Either the source or the destination must be a VM path (VM:PATH).")
        case (.remote, .remote):
            throw ValidationError(
                "Copying directly between two VMs is not supported; one path must be on the host.")
        case let (.local(localPath), .remote(vm, remotePath)):
            try transfer(
                vm: vm, localPath: localPath, remotePath: remotePath, upload: true)
        case let (.remote(vm, remotePath), .local(localPath)):
            try transfer(
                vm: vm, localPath: localPath, remotePath: remotePath, upload: false)
        }
    }

    @MainActor
    private func transfer(
        vm: String,
        localPath: String,
        remotePath: String,
        upload: Bool
    ) throws {
        var recursive = self.recursive

        if upload {
            var isDirectory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: localPath, isDirectory: &isDirectory)
            else {
                throw CpError.localSourceNotFound(localPath)
            }
            // Directories need scp -r; enable it transparently.
            if isDirectory.boolValue {
                recursive = true
            }
        }

        let controller = LumeController()

        let vmDetails: VMDetails
        do {
            vmDetails = try controller.getDetails(name: vm, storage: storage)
        } catch {
            throw SSHError.vmNotFound(vm)
        }

        guard vmDetails.status == "running" else {
            throw SSHError.vmNotRunning(vm)
        }

        guard let ipAddress = vmDetails.ipAddress, !ipAddress.isEmpty else {
            throw SSHError.noIPAddress(vm)
        }

        guard vmDetails.sshAvailable == true else {
            throw SSHError.sshNotAvailable(vm)
        }

        let client = SystemSSHClient(
            host: ipAddress,
            port: 22,
            user: user,
            password: password
        )

        try client.copyFile(
            localPath: localPath,
            remotePath: remotePath,
            upload: upload,
            recursive: recursive,
            timeout: TimeInterval(timeout)
        )

        if upload {
            print("Copied \(localPath) to \(vm):\(remotePath)")
        } else {
            print("Copied \(vm):\(remotePath) to \(localPath)")
        }
    }

    /// A source or destination argument: either a host path or a `VM:PATH`.
    enum Endpoint: Equatable {
        case local(String)
        case remote(vm: String, path: String)
    }

    /// Classify an argument as a host path or a `VM:PATH` reference.
    ///
    /// Following scp's convention, the argument is a remote reference only when a
    /// colon appears before any path separator. This keeps host paths that
    /// contain a colon (e.g. `./a:b` or `/tmp/x:y`) local.
    static func parseEndpoint(_ raw: String) -> Endpoint {
        if let colon = raw.firstIndex(of: ":") {
            let prefix = raw[raw.startIndex..<colon]
            if !prefix.isEmpty && !prefix.contains("/") {
                let path = String(raw[raw.index(after: colon)...])
                return .remote(vm: String(prefix), path: path)
            }
        }
        return .local(raw)
    }
}

/// Errors specific to `lume cp`.
enum CpError: Error, LocalizedError {
    case localSourceNotFound(String)

    var errorDescription: String? {
        switch self {
        case .localSourceNotFound(let path):
            return "Source path '\(path)' does not exist on the host"
        }
    }
}
