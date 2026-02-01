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
            capabilities: .init(
                prompts: .init(listChanged: false),
                resources: .init(subscribe: false, listChanged: false),
                tools: .init(listChanged: false)
            )
        )

        await registerHandlers()

        let transport = StdioTransport()
        try await mcpServer?.start(transport: transport)
        await mcpServer?.waitUntilCompleted()
    }

    private func registerHandlers() async {
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

        // Register ListResources handler
        await mcpServer?.withMethodHandler(ListResources.self) { [weak self] _ in
            guard let self = self else {
                return ListResources.Result(resources: [])
            }
            return await MainActor.run {
                ListResources.Result(resources: self.resourceDefinitions)
            }
        }

        // Register ReadResource handler
        await mcpServer?.withMethodHandler(ReadResource.self) { [weak self] params in
            guard let self = self else {
                return ReadResource.Result(contents: [])
            }
            return await MainActor.run {
                self.handleReadResource(params)
            }
        }

        // Register ListPrompts handler
        await mcpServer?.withMethodHandler(ListPrompts.self) { [weak self] _ in
            guard let self = self else {
                return ListPrompts.Result(prompts: [])
            }
            return await MainActor.run {
                ListPrompts.Result(prompts: self.promptDefinitions)
            }
        }

        // Register GetPrompt handler
        await mcpServer?.withMethodHandler(GetPrompt.self) { [weak self] params in
            guard let self = self else {
                return GetPrompt.Result(description: nil, messages: [])
            }
            return await MainActor.run {
                self.handleGetPrompt(params)
            }
        }
    }

    // MARK: - Resource Definitions

    private var resourceDefinitions: [Resource] {
        [
            Resource(
                name: "Lume Usage Guide",
                uri: "lume://usage-guide",
                description: "Best practices and workflows for managing macOS VMs with Lume",
                mimeType: "text/markdown"
            ),
            Resource(
                name: "Default Credentials",
                uri: "lume://credentials",
                description: "Default SSH credentials for VMs created with unattended setup",
                mimeType: "text/plain"
            )
        ]
    }

    private func handleReadResource(_ params: ReadResource.Parameters) -> ReadResource.Result {
        switch params.uri {
        case "lume://usage-guide":
            return ReadResource.Result(contents: [
                .text(usageGuideContent, uri: params.uri, mimeType: "text/markdown")
            ])
        case "lume://credentials":
            return ReadResource.Result(contents: [
                .text(credentialsContent, uri: params.uri, mimeType: "text/plain")
            ])
        default:
            return ReadResource.Result(contents: [])
        }
    }

    private var usageGuideContent: String {
        """
        # Lume VM Management Guide

        ## Overview
        Lume manages macOS virtual machines on Apple Silicon. Use these tools to create, run, and manage VMs for sandboxed development and testing.

        ## Typical Workflow

        ### 1. Check Existing VMs
        Always start by listing VMs to see what's available:
        ```
        lume_list_vms
        ```

        ### 2. Create a New VM (if needed)
        Creating a VM takes 15-30 minutes. Use `unattended: "tahoe"` for automatic setup with SSH enabled:
        ```
        lume_create_vm(name: "sandbox", unattended: "tahoe")
        ```
        The tool returns immediately. Poll `lume_list_vms` to monitor progress—status changes from `provisioning (ipsw_install)` → `running` (during unattended setup) → `stopped`.

        ### 3. Start the VM
        Start with optional shared directory for file access:
        ```
        lume_run_vm(name: "sandbox", shared_dir: "~/project", no_display: true)
        ```
        Shared files appear in the VM at `/Volumes/My Shared Files/`.

        ### 4. Execute Commands
        Run commands via SSH:
        ```
        lume_exec(name: "sandbox", command: "cd /Volumes/My\\\\ Shared\\\\ Files && npm test")
        ```

        ### 5. Stop the VM
        ```
        lume_stop_vm(name: "sandbox")
        ```

        ## Best Practices

        ### VM Naming
        - Use descriptive names: `dev-sandbox`, `test-runner`, `build-agent`
        - Avoid spaces and special characters

        ### Resource Allocation
        - Default: 4 CPU cores, 8GB RAM, 64GB disk
        - For builds: Consider 8 CPU cores, 16GB RAM
        - Disk grows dynamically (sparse files)

        ### Unattended Presets
        - `tahoe`: macOS Tahoe with SSH enabled, user `lume`/`lume`
        - `sequoia`: macOS Sequoia with SSH enabled, user `lume`/`lume`

        ### Golden Images
        Create a fully configured VM, then clone it for fast resets:
        ```
        lume_clone_vm(name: "configured-vm", new_name: "fresh-sandbox")
        ```

        ### Shared Directories
        - Read-write by default
        - Path in VM: `/Volumes/My Shared Files/`
        - Escape spaces in commands: `My\\ Shared\\ Files`

        ## Status Reference

        | Status | Meaning |
        |--------|---------|
        | `stopped` | Ready to start |
        | `running` | VM is active |
        | `provisioning (ipsw_install)` | Installing macOS |
        | `running` | VM is running (including during unattended setup) |

        ## Limitations
        - Max 2 macOS VMs running simultaneously (Apple licensing)
        - Linux VMs: Unlimited
        - Nested virtualization: Not supported for macOS guests
        """
    }

    private var credentialsContent: String {
        """
        Default credentials for VMs created with --unattended tahoe or sequoia:

        Username: lume
        Password: lume

        SSH is enabled automatically. Connect with:
        ssh lume@<vm-ip-address>

        Get VM IP with lume_get_vm(name: "vm-name")
        """
    }

    // MARK: - Prompt Definitions

    private var promptDefinitions: [Prompt] {
        [
            Prompt(
                name: "create-sandbox",
                description: "Create a new macOS sandbox VM with unattended setup",
                arguments: [
                    .init(name: "name", description: "Name for the new VM", required: true)
                ]
            ),
            Prompt(
                name: "run-in-sandbox",
                description: "Run a command in an existing sandbox VM",
                arguments: [
                    .init(name: "vm_name", description: "Name of the VM", required: true),
                    .init(name: "command", description: "Command to execute", required: true)
                ]
            ),
            Prompt(
                name: "reset-sandbox",
                description: "Reset a sandbox by cloning from a golden image",
                arguments: [
                    .init(name: "golden_image", description: "Name of the golden image VM", required: true),
                    .init(name: "sandbox_name", description: "Name for the fresh sandbox", required: true)
                ]
            )
        ]
    }

    private func handleGetPrompt(_ params: GetPrompt.Parameters) -> GetPrompt.Result {
        switch params.name {
        case "create-sandbox":
            let vmName = params.arguments?["name"] ?? "sandbox"
            return GetPrompt.Result(
                description: "Create a macOS sandbox VM",
                messages: [
                    .user("""
                        Create a new macOS sandbox VM named '\(vmName)' with these requirements:
                        1. Use unattended setup (tahoe preset) for automatic configuration
                        2. The VM should have SSH enabled with credentials lume/lume
                        3. Monitor the provisioning status until complete
                        4. Once ready, start the VM in headless mode
                        5. Verify SSH connectivity by running a simple command
                        """)
                ]
            )

        case "run-in-sandbox":
            let vmName = params.arguments?["vm_name"] ?? "sandbox"
            let command = params.arguments?["command"] ?? "echo 'Hello from sandbox'"
            return GetPrompt.Result(
                description: "Run command in sandbox VM",
                messages: [
                    .user("""
                        Run this command in the '\(vmName)' VM:
                        ```
                        \(command)
                        ```

                        Steps:
                        1. First check if the VM exists and is running (lume_list_vms)
                        2. If stopped, start it with lume_run_vm
                        3. Wait for it to get an IP address (lume_get_vm)
                        4. Execute the command with lume_exec
                        5. Report the output
                        """)
                ]
            )

        case "reset-sandbox":
            let goldenImage = params.arguments?["golden_image"] ?? "golden"
            let sandboxName = params.arguments?["sandbox_name"] ?? "sandbox"
            return GetPrompt.Result(
                description: "Reset sandbox from golden image",
                messages: [
                    .user("""
                        Reset the sandbox by cloning from the golden image:
                        1. Stop '\(sandboxName)' if it's running
                        2. Delete '\(sandboxName)' if it exists
                        3. Clone '\(goldenImage)' to '\(sandboxName)'
                        4. Start the new '\(sandboxName)' VM
                        5. Verify it's working with a simple SSH command
                        """)
                ]
            )

        default:
            return GetPrompt.Result(description: nil, messages: [])
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
                name: "lume_clipboard_push",
                description: "Push host clipboard content to a running VM. Copies text from the macOS host clipboard to the VM's clipboard via SSH. Requires SSH to be enabled in the VM.",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM")
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
                    "required": .array([.string("name")])
                ])
            ),
            Tool(
                name: "lume_clipboard_pull",
                description: "Pull clipboard content from a running VM to the host. Copies text from the VM's clipboard to the macOS host clipboard via SSH. Requires SSH to be enabled in the VM.",
                inputSchema: .object([
                    "type": .string("object"),
                    "properties": .object([
                        "name": .object([
                            "type": .string("string"),
                            "description": .string("Name of the VM")
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
                    "required": .array([.string("name")])
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
            case "lume_clipboard_push":
                return try await handleClipboardPush(params.arguments)
            case "lume_clipboard_pull":
                return try await handleClipboardPull(params.arguments)
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

        // Use getDetails() for consistent status including provisioning state
        let vmDetails = try controller.getDetails(name: name, storage: storage)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let json = try encoder.encode(vmDetails)
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

        // Run VM in detached task to avoid blocking (same pattern as HTTP API)
        Task.detached { @MainActor @Sendable in
            do {
                let vmController = LumeController()
                try await vmController.runVM(
                    name: name,
                    noDisplay: noDisplay,
                    sharedDirectories: sharedDirectories,
                    storage: storage
                )
            } catch {
                Logger.error(
                    "Failed to start VM in background task",
                    metadata: [
                        "name": name,
                        "error": error.localizedDescription,
                    ])
            }
        }

        // Wait briefly for VM to initialize and get IP
        try await Task.sleep(nanoseconds: 2_000_000_000)  // 2 seconds

        // Get VM details after starting to return IP
        let vmDetails = try controller.getDetails(name: name, storage: storage)
        var response = "VM '\(name)' started successfully."
        if let ip = vmDetails.ipAddress {
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

        // Execute command via native SSH client (no sshpass dependency)
        let sshClient = SSHClient(
            host: ip,
            port: 22,
            user: user,
            password: password
        )

        do {
            let sshResult = try await sshClient.execute(command: command, timeout: 60)

            var result = sshResult.output
            if result.isEmpty {
                result = sshResult.exitCode == 0
                    ? "Command completed successfully (no output)"
                    : "Command failed with exit code \(sshResult.exitCode)"
            }

            return CallTool.Result(content: [.text(result)], isError: sshResult.exitCode != 0)
        } catch let error as SSHError {
            return CallTool.Result(content: [.text("Error: \(error.localizedDescription)")], isError: true)
        } catch {
            return CallTool.Result(content: [.text("Error: \(error.localizedDescription)")], isError: true)
        }
    }

    private func handleClipboardPush(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
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

        let sshClient = SSHClient(
            host: ip,
            port: 22,
            user: user,
            password: password
        )
        let clipboardSync = ClipboardSync(sshClient: sshClient)

        do {
            let result = try await clipboardSync.push()
            return CallTool.Result(content: [.text(result)])
        } catch {
            return CallTool.Result(content: [.text("Error: \(error.localizedDescription)")], isError: true)
        }
    }

    private func handleClipboardPull(_ args: [String: Value]?) async throws -> CallTool.Result {
        guard let name = args?["name"]?.stringValue else {
            return CallTool.Result(content: [.text("Error: 'name' is required")], isError: true)
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

        let sshClient = SSHClient(
            host: ip,
            port: 22,
            user: user,
            password: password
        )
        let clipboardSync = ClipboardSync(sshClient: sshClient)

        do {
            let result = try await clipboardSync.pull()
            return CallTool.Result(content: [.text(result)])
        } catch {
            return CallTool.Result(content: [.text("Error: \(error.localizedDescription)")], isError: true)
        }
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
