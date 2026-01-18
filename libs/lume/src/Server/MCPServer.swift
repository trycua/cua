import Foundation
import MCP

/// MCP (Model Context Protocol) server for Lume VM management
/// Allows AI agents like Claude to manage VMs through MCP tools
@MainActor
final class LumeMCPServer {
    private let controller: LumeController
    private var mcpServer: MCP.Server?

    init(controller: LumeController) {
        self.controller = controller
    }

    func start() async throws {
        mcpServer = MCP.Server(
            name: "lume",
            version: "1.0.0",
            capabilities: .init(tools: .init(listChanged: false))
        )

        await registerTools()

        let transport = StdioTransport()
        try await mcpServer?.start(transport: transport)
        await mcpServer?.waitUntilCompleted()
    }

    private func registerTools() async {
        // Register ListTools handler
        await mcpServer?.withMethodHandler(ListTools.self) { [weak self] _ in
            guard let self = self else {
                return ListTools.Result(tools: [])
            }
            return await MainActor.run {
                ListTools.Result(tools: self.toolDefinitions)
            }
        }

        // Register CallTool handler
        await mcpServer?.withMethodHandler(CallTool.self) { [weak self] params in
            guard let self = self else {
                return CallTool.Result(content: [.text("Server not available")], isError: true)
            }
            return await self.handleToolCall(params)
        }
    }

    // MARK: - Tool Definitions

    private var toolDefinitions: [Tool] {
        [
            Tool(
                name: "lume_list_vms",
                description: "List all virtual machines with their status, IP addresses, and resource allocation",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path to filter VMs")
                        ])
                    ])
                ])
            ),
            Tool(
                name: "lume_get_vm",
                description: "Get detailed information about a specific VM including IP address, VNC URL, and SSH availability",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM")
                        ]),
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path")
                        ])
                    ]),
                    "required": .array([.string("name")])
                ])
            ),
            Tool(
                name: "lume_run_vm",
                description: "Start a VM with optional shared directory for file access. The shared directory will be available at /Volumes/My Shared Files inside the VM.",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM to start")
                        ]),
                        "shared_dir": .object([
                            "type": .string("string"),
                            "description": .string("Host directory path to share with the VM (appears at /Volumes/My Shared Files)")
                        ]),
                        "no_display": .object([
                            "type": .string("boolean"),
                            "description": .string("Run headless without VNC window (default: true)")
                        ]),
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path")
                        ])
                    ]),
                    "required": .array([.string("name")])
                ])
            ),
            Tool(
                name: "lume_stop_vm",
                description: "Stop a running VM gracefully",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM to stop")
                        ]),
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path")
                        ])
                    ]),
                    "required": .array([.string("name")])
                ])
            ),
            Tool(
                name: "lume_clone_vm",
                description: "Clone a VM to create a copy. Useful for creating golden images for instant reset.",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the source VM to clone")
                        ]),
                        "new_name": .object([
                            "type": .string("string"),
                            "description": .string("Name for the cloned VM")
                        ])
                    ]),
                    "required": .array([.string("name"), .string("new_name")])
                ])
            ),
            Tool(
                name: "lume_delete_vm",
                description: "Delete a VM and all its associated files",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM to delete")
                        ]),
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path")
                        ])
                    ]),
                    "required": .array([.string("name")])
                ])
            ),
            Tool(
                name: "lume_exec",
                description: "Execute a command inside a running VM via SSH. Requires SSH to be enabled in the VM (default for VMs created with --unattended tahoe).",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM")
                        ]),
                        "command": .object([
                            "type": .string("string"),
                            "description": .string("Shell command to execute inside the VM")
                        ]),
                        "user": .object([
                            "type": .string("string"),
                            "description": .string("SSH username (default: lume)")
                        ]),
                        "password": .object([
                            "type": .string("string"),
                            "description": .string("SSH password (default: lume)")
                        ]),
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path")
                        ])
                    ]),
                    "required": .array([.string("name"), .string("command")])
                ])
            ),
            Tool(
                name: "lume_create_vm",
                description: "Create a new macOS VM asynchronously. Returns immediately while the VM is being provisioned. Poll lume_list_vms or lume_get_vm to check progress (status will be 'provisioning' until complete).",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name for the new VM")
                        ]),
                        "ipsw": .object([
                            "type": .string("string"),
                            "description": .string("Path to IPSW file or 'latest' to download (default: latest)")
                        ]),
                        "unattended": .object([
                            "type": .string("string"),
                            "description": .string("Unattended setup preset (e.g., 'tahoe', 'sequoia') for automatic macOS configuration")
                        ]),
                        "cpu": .object([
                            "type": .string("integer"),
                            "description": .string("Number of CPU cores (default: 4)")
                        ]),
                        "memory": .object([
                            "type": .string("string"),
                            "description": .string("Memory size, e.g., '8GB' (default: 8GB)")
                        ]),
                        "disk_size": .object([
                            "type": .string("string"),
                            "description": .string("Disk size, e.g., '64GB' (default: 64GB)")
                        ]),
                        "storage": .object([
                            "type": .string("string"),
                            "description": .string("Optional storage location name or path")
                        ])
                    ]),
                    "required": .array([.string("name")])
                ])
            )
        ]
    }

    // MARK: - Tool Call Handler

    private func handleToolCall(_ params: CallTool.Parameters) async -> CallTool.Result {
        do {
            switch params.name {
            case "lume_list_vms":
                return try await handleListVMs(params.arguments)
            case "lume_get_vm":
                return try await handleGetVM(params.arguments)
            case "lume_run_vm":
                return try await handleRunVM(params.arguments)
            case "lume_stop_vm":
                return try await handleStopVM(params.arguments)
            case "lume_clone_vm":
                return try await handleCloneVM(params.arguments)
            case "lume_delete_vm":
                return try await handleDeleteVM(params.arguments)
            case "lume_exec":
                return try await handleExec(params.arguments)
            case "lume_create_vm":
                return try await handleCreateVM(params.arguments)
            default:
                return CallTool.Result(
                    content: [.text("Unknown tool: \(params.name)")],
                    isError: true
                )
            }
        } catch {
            return CallTool.Result(
                content: [.text("Error: \(error.localizedDescription)")],
                isError: true
            )
        }
    }

    // MARK: - Tool Implementations

    private func handleListVMs(_ args: [String: Value]?) async throws -> CallTool.Result {
        let storage = args?["storage"]?.stringValue
        let vms = try controller.list(storage: storage)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let json = try encoder.encode(vms)
        return CallTool.Result(content: [.text(String(data: json, encoding: .utf8) ?? "[]")])
    }

    private func handleGetVM(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }
        let storage = args?["storage"]?.stringValue

        let vm = try controller.get(name: name, storage: storage)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let json = try encoder.encode(vm.details)
        return CallTool.Result(content: [.text(String(data: json, encoding: .utf8) ?? "{}")])
    }

    private func handleRunVM(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }

        let storage = args?["storage"]?.stringValue
        let noDisplay = args?["no_display"]?.boolValue ?? true

        var sharedDirectories: [SharedDirectory] = []
        if let sharedDir = args?["shared_dir"]?.stringValue {
            // Expand ~ to home directory
            let expandedPath = (sharedDir as NSString).expandingTildeInPath
            sharedDirectories.append(SharedDirectory(hostPath: expandedPath, tag: "shared", readOnly: false))
        }

        try await controller.runVM(
            name: name,
            noDisplay: noDisplay,
            sharedDirectories: sharedDirectories,
            storage: storage
        )

        // Get VM details after starting to return IP
        let vm = try controller.get(name: name, storage: storage)
        var response = "VM '\(name)' started successfully."
        if let ip = vm.details.ipAddress {
            response += "\nIP Address: \(ip)"
            response += "\nSSH: ssh lume@\(ip) (password: lume)"
        }
        if !sharedDirectories.isEmpty {
            response += "\nShared directory available at: /Volumes/My Shared Files"
        }

        return CallTool.Result(content: [.text(response)])
    }

    private func handleStopVM(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }
        let storage = args?["storage"]?.stringValue

        try await controller.stopVM(name: name, storage: storage)
        return CallTool.Result(content: [.text("VM '\(name)' stopped successfully.")])
    }

    private func handleCloneVM(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }
        guard let newName = args?["new_name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'new_name' is required")], isError: true)
        }

        try controller.clone(name: name, newName: newName)
        return CallTool.Result(content: [.text("VM '\(name)' cloned to '\(newName)' successfully.")])
    }

    private func handleDeleteVM(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }
        let storage = args?["storage"]?.stringValue

        try await controller.delete(name: name, storage: storage)
        return CallTool.Result(content: [.text("VM '\(name)' deleted successfully.")])
    }

    private func handleExec(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }
        guard let command = args?["command"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'command' is required")], isError: true)
        }

        let user = args?["user"]?.stringValue ?? "lume"
        let password = args?["password"]?.stringValue ?? "lume"
        let storage = args?["storage"]?.stringValue

        // Get VM to find IP address
        let vm = try controller.get(name: name, storage: storage)
        guard let ip = vm.details.ipAddress else {
            return CallTool.Result(
                content: [.text("Error: VM '\(name)' has no IP address. Is it running?")],
                isError: true
            )
        }

        // Check if SSH is available
        if vm.details.sshAvailable == false {
            return CallTool.Result(
                content: [.text("Error: SSH is not available on VM '\(name)'. Make sure SSH is enabled in the VM.")],
                isError: true
            )
        }

        // Find sshpass in common locations
        let sshpassPaths = [
            "/opt/homebrew/bin/sshpass",
            "/usr/local/bin/sshpass",
            "/usr/bin/sshpass"
        ]

        var sshpassPath: String?
        for path in sshpassPaths {
            if FileManager.default.fileExists(atPath: path) {
                sshpassPath = path
                break
            }
        }

        guard let sshpass = sshpassPath else {
            return CallTool.Result(
                content: [.text("Error: sshpass not found. Install it with: brew install hudochenkov/sshpass/sshpass")],
                isError: true
            )
        }

        // Execute command via SSH using sshpass
        let process = Process()
        process.executableURL = URL(fileURLWithPath: sshpass)
        process.arguments = [
            "-p", password,
            "ssh",
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "LogLevel=ERROR",
            "\(user)@\(ip)",
            command
        ]

        let outputPipe = Pipe()
        let errorPipe = Pipe()
        process.standardOutput = outputPipe
        process.standardError = errorPipe

        try process.run()
        process.waitUntilExit()

        let outputData = outputPipe.fileHandleForReading.readDataToEndOfFile()
        let errorData = errorPipe.fileHandleForReading.readDataToEndOfFile()

        let output = String(data: outputData, encoding: .utf8) ?? ""
        let errorOutput = String(data: errorData, encoding: .utf8) ?? ""

        let isError = process.terminationStatus != 0

        var result = output
        if !errorOutput.isEmpty {
            if !result.isEmpty {
                result += "\n"
            }
            result += errorOutput
        }
        if result.isEmpty {
            result = isError ? "Command failed with exit code \(process.terminationStatus)" : "Command completed successfully (no output)"
        }

        return CallTool.Result(content: [.text(result)], isError: isError)
    }

    private func handleCreateVM(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
        }

        let ipsw = args?["ipsw"]?.stringValue ?? "latest"
        let unattendedPreset = args?["unattended"]?.stringValue
        let cpuCount = args?["cpu"]?.intValue ?? 4
        let storage = args?["storage"]?.stringValue

        // Parse memory size (default 8GB)
        let memorySize: UInt64
        if let memoryStr = args?["memory"]?.stringValue {
            memorySize = try parseSize(memoryStr)
        } else {
            memorySize = 8 * 1024 * 1024 * 1024  // 8GB default
        }

        // Parse disk size (default 64GB)
        let diskSize: UInt64
        if let diskStr = args?["disk_size"]?.stringValue {
            diskSize = try parseSize(diskStr)
        } else {
            diskSize = 64 * 1024 * 1024 * 1024  // 64GB default
        }

        // Load unattended config if specified
        var unattendedConfig: UnattendedConfig? = nil
        if let preset = unattendedPreset {
            unattendedConfig = try UnattendedConfig.load(from: preset)
        }

        // Use async create - returns immediately
        try controller.createAsync(
            name: name,
            os: "macos",
            diskSize: diskSize,
            cpuCount: cpuCount,
            memorySize: memorySize,
            display: "1920x1080",
            ipsw: ipsw,
            storage: storage,
            unattendedConfig: unattendedConfig
        )

        var response = "VM '\(name)' creation started. Status: provisioning."
        response += "\nUse lume_list_vms or lume_get_vm to monitor progress."
        if unattendedPreset != nil {
            response += "\nUnattended setup will run automatically after IPSW installation."
        }

        return CallTool.Result(content: [.text(response)])
    }
}

// MARK: - Value Extension for type-safe argument access

extension Value {
    var stringValue: String? {
        if case .string(let value) = self {
            return value
        }
        return nil
    }

    var boolValue: Bool? {
        if case .bool(let value) = self {
            return value
        }
        return nil
    }

    var intValue: Int? {
        if case .int(let value) = self {
            return value
        }
        return nil
    }
}
