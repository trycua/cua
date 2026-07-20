import ArgumentParser
import Foundation

// MARK: - Documentation Types

/// Represents documentation for a single CLI command
struct CommandDoc: Codable {
    let name: String
    let abstract: String
    let discussion: String?
    let arguments: [ArgumentDoc]
    let options: [OptionDoc]
    let flags: [FlagDoc]
    let subcommands: [CommandDoc]
}

/// Represents documentation for a command argument
struct ArgumentDoc: Codable {
    let name: String
    let help: String
    let type: String
    let isOptional: Bool
}

/// Represents documentation for a command option
struct OptionDoc: Codable {
    let name: String
    let shortName: String?
    let help: String
    let type: String
    let defaultValue: String?
    let isOptional: Bool
}

/// Represents documentation for a command flag
struct FlagDoc: Codable {
    let name: String
    let shortName: String?
    let help: String
    let defaultValue: Bool
}

/// Root documentation structure
struct CLIDocumentation: Codable {
    let name: String
    let version: String
    let abstract: String
    let commands: [CommandDoc]
}

// MARK: - Command Documentation Extractor

/// Extracts CLI documentation from command definitions.
/// This uses a combination of ArgumentParser's configuration metadata
/// and manually maintained help text to provide accurate documentation.
enum CommandDocExtractor {
    /// Extract documentation from all registered commands
    static func extractAll() -> CLIDocumentation {
        let coverage = documentationCoverage
        precondition(
            coverage.missing.isEmpty && coverage.extra.isEmpty,
            "CLI documentation coverage mismatch. Missing: \(coverage.missing); extra: \(coverage.extra)"
        )
        return CLIDocumentation(
            name: "lume",
            version: Lume.Version.current,
            abstract: "A lightweight CLI and local API server to build, run and manage macOS VMs.",
            commands: allCommandDocs
        )
    }

    // MARK: - Command Documentation Definitions

    /// All command documentation, kept in sync with source code.
    /// These definitions are validated against ArgumentParser metadata at build time.
    private static var allCommandDocs: [CommandDoc] {
        return [
            createDoc,
            pullDoc,
            pushDoc,
            convertDoc,
            importDoc,
            exportDoc,
            imagesDoc,
            cloneDoc,
            getDoc,
            setDoc,
            listDoc,
            runDoc,
            stopDoc,
            sshDoc,
            sipDoc,
            ipswDoc,
            serveDoc,
            deleteDoc,
            pruneDoc,
            configDoc,
            logsDoc,
            checkUpdateDoc,
            updateDoc,
            setupDoc,
            dumpDocsDoc,
        ]
    }

    static var documentationCoverage: (missing: [String], extra: [String]) {
        let registered = Swift.Set(CommandRegistry.allCommands.map(commandName))
        let documented = Swift.Set(allCommandDocs.map(\.name))
        return (
            missing: registered.subtracting(documented).sorted(),
            extra: documented.subtracting(registered).sorted()
        )
    }

    private static func commandName(_ command: ParsableCommand.Type) -> String {
        command.configuration.commandName ?? String(describing: command).lowercased()
    }

    // MARK: - Create

    private static var createDoc: CommandDoc {
        CommandDoc(
            name: "create",
            abstract: "Create a new virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name for the virtual machine", type: "String", isOptional: false)
            ],
            options: [
                OptionDoc(name: "os", shortName: nil, help: "Operating system to install (macOS or linux)", type: "String", defaultValue: "macOS", isOptional: false),
                OptionDoc(name: "cpu", shortName: nil, help: "Number of CPU cores", type: "Int", defaultValue: "4", isOptional: false),
                OptionDoc(name: "memory", shortName: nil, help: "Memory size (e.g., 8GB)", type: "String", defaultValue: "8GB", isOptional: false),
                OptionDoc(name: "disk-size", shortName: nil, help: "Disk size (e.g., 100GB)", type: "String", defaultValue: "100GB for macOS; 50GB for Linux", isOptional: false),
                OptionDoc(name: "display", shortName: nil, help: "Display resolution (e.g., 1024x768)", type: "String", defaultValue: "1024x768", isOptional: false),
                OptionDoc(name: "ipsw", shortName: nil, help: "Path to IPSW file or 'latest' for macOS VMs", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "unattended", shortName: nil, help: "Prepare macOS unattended setup offline after install. Preset name or YAML path is accepted for compatibility. Built-in presets: sequoia, tahoe. Only supported for macOS VMs.", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "debug-dir", shortName: nil, help: "Compatibility option; ignored by offline setup.", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port to use for the temporary verification VNC server. Defaults to 0 (auto-assign).", type: "Int", defaultValue: "0", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "debug", shortName: nil, help: "Compatibility flag; ignored by offline setup.", defaultValue: false),
                FlagDoc(name: "no-display", shortName: nil, help: "Compatibility flag; offline setup verifies headlessly.", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Pull

    private static var pullDoc: CommandDoc {
        CommandDoc(
            name: "pull",
            abstract: "Pull a prebuilt or custom macOS image from an OCI-compatible registry",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "image", help: "Image to pull (format: name:tag)", type: "String", isOptional: false),
                ArgumentDoc(name: "name", help: "Name for the resulting VM", type: "String", isOptional: true),
            ],
            options: [
                OptionDoc(name: "registry", shortName: nil, help: "Container registry URL", type: "String", defaultValue: "ghcr.io", isOptional: false),
                OptionDoc(name: "organization", shortName: nil, help: "Organization to pull from", type: "String", defaultValue: "trycua", isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Push

    private static var pushDoc: CommandDoc {
        CommandDoc(
            name: "push",
            abstract: "Push a macOS VM to an OCI-compatible registry",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of VM to push", type: "String", isOptional: false),
                ArgumentDoc(name: "image", help: "Image tag (format: name:tag)", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "additional-tags", shortName: nil, help: "Additional tags to push", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "registry", shortName: nil, help: "Container registry URL", type: "String", defaultValue: "ghcr.io", isOptional: false),
                OptionDoc(name: "organization", shortName: nil, help: "Organization to push to", type: "String", defaultValue: "trycua", isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "chunk-size-mb", shortName: nil, help: "Chunk size for upload in MB", type: "Int", defaultValue: "512", isOptional: false),
            ],
            flags: [
                FlagDoc(name: "verbose", shortName: nil, help: "Enable verbose logging", defaultValue: false),
                FlagDoc(name: "dry-run", shortName: nil, help: "Prepare files without uploading", defaultValue: false),
                FlagDoc(name: "reassemble", shortName: nil, help: "Verify integrity by reassembling chunks", defaultValue: true),
            ],
            subcommands: []
        )
    }

    // MARK: - Convert

    private static var convertDoc: CommandDoc {
        CommandDoc(
            name: "convert",
            abstract: "Convert a legacy Lume image to OCI-compliant format",
            discussion: nil,
            arguments: [
                ArgumentDoc(
                    name: "source-image",
                    help: "Source image to convert (legacy format, for example macos-tahoe:latest)",
                    type: "String",
                    isOptional: false
                ),
                ArgumentDoc(
                    name: "target-image",
                    help: "Target image to push in OCI format (name:tag)",
                    type: "String",
                    isOptional: false
                ),
            ],
            options: [
                OptionDoc(name: "additional-tags", shortName: nil, help: "Additional tags for the OCI image", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "registry", shortName: nil, help: "Registry to pull from and push to", type: "String", defaultValue: "ghcr.io", isOptional: false),
                OptionDoc(name: "organization", shortName: nil, help: "Registry organization", type: "String", defaultValue: "trycua", isOptional: false),
            ],
            flags: [
                FlagDoc(name: "verbose", shortName: nil, help: "Enable verbose logging", defaultValue: false),
                FlagDoc(name: "dry-run", shortName: nil, help: "Prepare files without uploading", defaultValue: false),
                FlagDoc(name: "single-layer", shortName: nil, help: "Push one kubelet-compatible disk layer", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Import

    private static var importDoc: CommandDoc {
        CommandDoc(
            name: "import",
            abstract: "Import a virtual machine from another format",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [importUTMDoc]
        )
    }

    private static var importUTMDoc: CommandDoc {
        CommandDoc(
            name: "utm",
            abstract: "Import a stopped UTM Apple/macOS VM into Lume",
            discussion: "Creates an independent Lume VM from a UTM .utm bundle while preserving the raw disk, auxiliary storage, and hardware model. The imported VM receives a fresh machine identifier and MAC address.",
            arguments: [
                ArgumentDoc(name: "bundle", help: "Path to the source .utm bundle", type: "Path", isOptional: false),
                ArgumentDoc(name: "name", help: "Name for the imported Lume VM", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "storage", shortName: nil, help: "Destination Lume storage location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Export

    private static var exportDoc: CommandDoc {
        CommandDoc(
            name: "export",
            abstract: "Export a virtual machine to another format",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [exportUTMDoc]
        )
    }

    private static var exportUTMDoc: CommandDoc {
        CommandDoc(
            name: "utm",
            abstract: "Export a stopped Lume macOS VM as a UTM bundle",
            discussion: "Creates an independent UTM Apple/macOS .utm bundle while preserving the raw disk, auxiliary storage, and hardware model. The exported VM receives a fresh machine identifier and MAC address.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the source Lume VM", type: "String", isOptional: false),
                ArgumentDoc(name: "output", help: "Destination .utm bundle path", type: "Path", isOptional: false),
            ],
            options: [
                OptionDoc(name: "storage", shortName: nil, help: "Source Lume storage location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Images

    private static var imagesDoc: CommandDoc {
        CommandDoc(
            name: "images",
            abstract: "List available macOS images from local cache",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "organization", shortName: nil, help: "Organization to list images for", type: "String", defaultValue: "trycua", isOptional: false),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Clone

    private static var cloneDoc: CommandDoc {
        CommandDoc(
            name: "clone",
            abstract: "Clone an existing virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the source VM", type: "String", isOptional: false),
                ArgumentDoc(name: "new-name", help: "Name for the cloned VM", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "source-storage", shortName: nil, help: "Source VM storage location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "dest-storage", shortName: nil, help: "Destination VM storage location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Get

    private static var getDoc: CommandDoc {
        CommandDoc(
            name: "get",
            abstract: "Get detailed information about a virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "format", shortName: "f", help: "Output format", type: "String", defaultValue: "text", isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Set

    private static var setDoc: CommandDoc {
        CommandDoc(
            name: "set",
            abstract: "Set new values for CPU, memory, and disk size of a virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "cpu", shortName: nil, help: "New number of CPU cores", type: "Int", defaultValue: nil, isOptional: true),
                OptionDoc(name: "memory", shortName: nil, help: "New memory size (e.g., 8GB)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "disk-size", shortName: nil, help: "New total disk size. Increase only; macOS resizing preserves RecoveryOS and grows APFS.", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "display", shortName: nil, help: "New display resolution", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-backup", shortName: nil, help: "Skip the macOS rollback backup", defaultValue: false),
                FlagDoc(name: "keep-backup", shortName: nil, help: "Keep rollback files after a successful macOS resize", defaultValue: false),
                FlagDoc(name: "dry-run", shortName: nil, help: "Validate the resize plan without modifying the disk", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - List

    private static var listDoc: CommandDoc {
        CommandDoc(
            name: "ls",
            abstract: "List virtual machines",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "format", shortName: "f", help: "Output format (json or text)", type: "String", defaultValue: "text", isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "Filter by storage location name", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Run

    private static var runDoc: CommandDoc {
        CommandDoc(
            name: "run",
            abstract: "Run a virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM or image to run (format: name or name:tag)", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "shared-dir", shortName: nil, help: "Directory to share with the VM (format: path or path:ro or path:rw)", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "mount", shortName: nil, help: "For Linux VMs only, attach a read-only disk image", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "usb-storage", shortName: nil, help: "Disk image to attach as USB mass storage device", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "registry", shortName: nil, help: "Container registry URL", type: "String", defaultValue: "ghcr.io", isOptional: false),
                OptionDoc(name: "organization", shortName: nil, help: "Organization to pull from", type: "String", defaultValue: "trycua", isOptional: false),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port for VNC server (0 for auto-assign)", type: "Int", defaultValue: "0", isOptional: false),
                OptionDoc(name: "recovery-mode", shortName: nil, help: "For macOS VMs only, boot in recovery mode", type: "Bool", defaultValue: "false", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-display", shortName: "d", help: "Do not start the VNC client", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Stop

    private static var stopDoc: CommandDoc {
        CommandDoc(
            name: "stop",
            abstract: "Stop a virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM to stop", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - SSH

    private static var sshDoc: CommandDoc {
        CommandDoc(
            name: "ssh",
            abstract: "Connect to a VM via SSH or execute commands remotely",
            discussion: "Requires Remote Login to be enabled on the VM (System Settings > General > Sharing > Remote Login). VMs created with --unattended have this enabled automatically with credentials lume/lume.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
                ArgumentDoc(name: "command", help: "Command to execute (omit for interactive shell)", type: "[String]", isOptional: true),
            ],
            options: [
                OptionDoc(name: "user", shortName: "u", help: "SSH username", type: "String", defaultValue: "lume", isOptional: false),
                OptionDoc(name: "password", shortName: "p", help: "SSH password", type: "String", defaultValue: "lume", isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "Storage location name or path", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "timeout", shortName: "t", help: "Command timeout in seconds (0 for no timeout)", type: "Int", defaultValue: "60", isOptional: false),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - SIP

    private static var sipDoc: CommandDoc {
        CommandDoc(
            name: "sip",
            abstract: "Enable or disable System Integrity Protection on a macOS VM",
            discussion: "Validates the administrator account, changes SIP from paired Recovery over VNC, and verifies the canonical csrutil status after a normal boot.",
            arguments: [
                ArgumentDoc(name: "state", help: "Desired SIP state: on or off", type: "String", isOptional: false),
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "admin-user", shortName: nil, help: "Administrator username in the guest", type: "String", defaultValue: "lume", isOptional: false),
                OptionDoc(name: "admin-password", shortName: nil, help: "Administrator password in the guest; prefer --admin-password-stdin", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "screenshot-dir", shortName: nil, help: "Directory for Recovery framebuffer screenshots", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port for the temporary Recovery VNC server", type: "Int", defaultValue: "5999", isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "timeout", shortName: nil, help: "Overall timeout in seconds", type: "Int", defaultValue: "900", isOptional: false),
            ],
            flags: [
                FlagDoc(name: "yes", shortName: "y", help: "Skip the interactive confirmation prompt", defaultValue: false),
                FlagDoc(name: "admin-password-stdin", shortName: nil, help: "Read one administrator-password line from standard input without echo", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - IPSW

    private static var ipswDoc: CommandDoc {
        CommandDoc(
            name: "ipsw",
            abstract: "Get macOS restore image IPSW URL",
            discussion: "Download IPSW file manually, then use in create command with --ipsw",
            arguments: [],
            options: [],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Serve

    private static var serveDoc: CommandDoc {
        CommandDoc(
            name: "serve",
            abstract: "Start the VM management server",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "port", shortName: nil, help: "Port to listen on", type: "Int", defaultValue: "7777", isOptional: false),
            ],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Delete

    private static var deleteDoc: CommandDoc {
        CommandDoc(
            name: "delete",
            abstract: "Delete a virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM to delete", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "force", shortName: nil, help: "Force deletion without confirmation", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Prune

    private static var pruneDoc: CommandDoc {
        CommandDoc(
            name: "prune",
            abstract: "Remove cached images",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: []
        )
    }

    // MARK: - Config

    private static var configDoc: CommandDoc {
        CommandDoc(
            name: "config",
            abstract: "Get or set lume configuration",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                CommandDoc(
                    name: "get",
                    abstract: "Get current configuration",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: []
                ),
                CommandDoc(
                    name: "storage",
                    abstract: "Manage VM storage locations",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "add", abstract: "Add a new VM storage location", discussion: nil, arguments: [
                            ArgumentDoc(name: "name", help: "Storage name", type: "String", isOptional: false),
                            ArgumentDoc(name: "path", help: "Path to storage directory", type: "String", isOptional: false),
                        ], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "remove", abstract: "Remove a VM storage location", discussion: nil, arguments: [
                            ArgumentDoc(name: "name", help: "Storage name to remove", type: "String", isOptional: false),
                        ], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "list", abstract: "List all VM storage locations", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "default", abstract: "Set the default VM storage location", discussion: nil, arguments: [
                            ArgumentDoc(name: "name", help: "Storage name to set as default", type: "String", isOptional: false),
                        ], options: [], flags: [], subcommands: []),
                    ]
                ),
                CommandDoc(
                    name: "cache",
                    abstract: "Manage image cache settings",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "status", abstract: "Show cache status and directory", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "dir", abstract: "Get or set cache directory", discussion: nil, arguments: [
                            ArgumentDoc(name: "path", help: "Path to cache directory. Omit to show current directory.", type: "String", isOptional: true),
                        ], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "enable", abstract: "Enable image caching", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "disable", abstract: "Disable image caching", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                    ]
                ),
                CommandDoc(
                    name: "telemetry",
                    abstract: "Manage pseudonymous telemetry settings",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "status", abstract: "Show current telemetry status", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "enable", abstract: "Enable pseudonymous telemetry", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "disable", abstract: "Disable pseudonymous telemetry", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "reset-id", abstract: "Delete the pseudonymous installation ID and registration markers", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                    ]
                ),
            ]
        )
    }

    // MARK: - Logs

    private static var logsDoc: CommandDoc {
        CommandDoc(
            name: "logs",
            abstract: "View lume serve logs",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                CommandDoc(
                    name: "info",
                    abstract: "View info logs from the daemon",
                    discussion: nil,
                    arguments: [],
                    options: [
                        OptionDoc(name: "lines", shortName: "n", help: "Number of lines to display", type: "Int", defaultValue: nil, isOptional: true),
                    ],
                    flags: [
                        FlagDoc(name: "follow", shortName: "f", help: "Follow log file continuously", defaultValue: false),
                    ],
                    subcommands: []
                ),
                CommandDoc(
                    name: "error",
                    abstract: "View error logs from the daemon",
                    discussion: nil,
                    arguments: [],
                    options: [
                        OptionDoc(name: "lines", shortName: "n", help: "Number of lines to display", type: "Int", defaultValue: nil, isOptional: true),
                    ],
                    flags: [
                        FlagDoc(name: "follow", shortName: "f", help: "Follow log file continuously", defaultValue: false),
                    ],
                    subcommands: []
                ),
                CommandDoc(
                    name: "all",
                    abstract: "View both info and error logs",
                    discussion: nil,
                    arguments: [],
                    options: [
                        OptionDoc(name: "lines", shortName: "n", help: "Number of lines to display", type: "Int", defaultValue: nil, isOptional: true),
                    ],
                    flags: [
                        FlagDoc(name: "follow", shortName: "f", help: "Follow log files continuously", defaultValue: false),
                    ],
                    subcommands: []
                ),
            ]
        )
    }

    // MARK: - Update

    private static var checkUpdateDoc: CommandDoc {
        CommandDoc(
            name: "check-update",
            abstract: "Check whether a newer Lume release is available",
            discussion: "Read-only update check. Uses GitHub Releases, caches the result briefly, and never installs anything.",
            arguments: [],
            options: [],
            flags: [
                FlagDoc(name: "json", shortName: nil, help: "Emit the structured update-state payload as JSON", defaultValue: false),
                FlagDoc(name: "no-cache", shortName: nil, help: "Bypass the local update-check cache", defaultValue: false),
            ],
            subcommands: []
        )
    }

    private static var updateDoc: CommandDoc {
        CommandDoc(
            name: "update",
            abstract: "Check for a Lume update and optionally apply it",
            discussion: "Without --apply, this command only checks for a newer release and prints the command to install it. With --apply, it delegates installation to the canonical Lume installer pinned to the discovered version.",
            arguments: [],
            options: [],
            flags: [
                FlagDoc(name: "apply", shortName: nil, help: "Apply the update by re-running the official installer", defaultValue: false),
                FlagDoc(name: "json", shortName: nil, help: "Emit the structured update-state payload as JSON", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Setup

    private static var setupDoc: CommandDoc {
        CommandDoc(
            name: "setup",
            abstract: "Prepare unattended macOS setup",
            discussion: "Lume prepares the macOS disk offline, skips Setup Assistant, enables autologin and SSH, disables screensaver lock, verifies SSH, then stops the VM.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "unattended", shortName: nil, help: "Defaults to tahoe. Preset name or YAML path for compatibility and optional post-SSH commands. Built-in presets: sequoia, tahoe.", type: "String", defaultValue: "tahoe", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use or direct path to VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port to use for the temporary verification VNC server. Defaults to 0 (auto-assign)", type: "Int", defaultValue: "0", isOptional: true),
                OptionDoc(name: "debug-dir", shortName: nil, help: "Compatibility option; ignored by offline setup", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-display", shortName: nil, help: "Compatibility flag; offline setup verifies headlessly", defaultValue: false),
                FlagDoc(name: "debug", shortName: nil, help: "Compatibility flag; ignored by offline setup", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Dump Docs

    private static var dumpDocsDoc: CommandDoc {
        CommandDoc(
            name: "dump-docs",
            abstract: "Output CLI and API documentation as JSON for tooling and integrations",
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "type", shortName: nil, help: "Documentation type: cli, api, or all", type: "String", defaultValue: "cli", isOptional: false),
            ],
            flags: [
                FlagDoc(name: "pretty", shortName: nil, help: "Pretty-print JSON output", defaultValue: false),
            ],
            subcommands: []
        )
    }
}
