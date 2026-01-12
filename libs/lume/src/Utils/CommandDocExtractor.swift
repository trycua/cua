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
            imagesDoc,
            cloneDoc,
            getDoc,
            setDoc,
            listDoc,
            runDoc,
            stopDoc,
            ipswDoc,
            serveDoc,
            deleteDoc,
            pruneDoc,
            configDoc,
            logsDoc,
            setupDoc,
        ]
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
                OptionDoc(name: "disk-size", shortName: nil, help: "Disk size (e.g., 50GB)", type: "String", defaultValue: "50GB", isOptional: false),
                OptionDoc(name: "display", shortName: nil, help: "Display resolution (e.g., 1024x768)", type: "String", defaultValue: "1024x768", isOptional: false),
                OptionDoc(name: "ipsw", shortName: nil, help: "Path to IPSW file or 'latest' for macOS VMs", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "unattended", shortName: nil, help: "[Preview] Preset name or path to YAML config file for unattended macOS Setup Assistant automation. Built-in presets: tahoe. Only supported for macOS VMs.", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "debug-dir", shortName: nil, help: "Custom directory for debug screenshots during unattended setup (defaults to unique folder in system temp)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port to use for the VNC server during unattended setup. Defaults to 0 (auto-assign)", type: "Int", defaultValue: "0", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "debug", shortName: nil, help: "Enable debug mode for unattended setup - saves screenshots with click coordinates", defaultValue: false),
                FlagDoc(name: "no-display", shortName: nil, help: "Do not open the VNC client during unattended setup (default: true for unattended)", defaultValue: false),
            ],
            subcommands: []
        )
    }

    // MARK: - Pull

    private static var pullDoc: CommandDoc {
        CommandDoc(
            name: "pull",
            abstract: "Pull a macOS image from GitHub Container Registry",
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
            abstract: "Push a macOS VM to GitHub Container Registry",
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
                OptionDoc(name: "disk-size", shortName: nil, help: "New disk size (e.g., 100GB)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "display", shortName: nil, help: "New display resolution", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
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
                    abstract: "Manage cache settings",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "get", abstract: "Get current cache directory", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "set", abstract: "Set cache directory", discussion: nil, arguments: [
                            ArgumentDoc(name: "path", help: "Path to cache directory", type: "String", isOptional: false),
                        ], options: [], flags: [], subcommands: []),
                    ]
                ),
                CommandDoc(
                    name: "caching",
                    abstract: "Manage image caching settings",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        CommandDoc(name: "get", abstract: "Show current caching status", discussion: nil, arguments: [], options: [], flags: [], subcommands: []),
                        CommandDoc(name: "set", abstract: "Enable or disable image caching", discussion: nil, arguments: [
                            ArgumentDoc(name: "enabled", help: "Enable or disable caching (true/false)", type: "Bool", isOptional: false),
                        ], options: [], flags: [], subcommands: []),
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

    // MARK: - Setup

    private static var setupDoc: CommandDoc {
        CommandDoc(
            name: "setup",
            abstract: "[Preview] Run unattended Setup Assistant automation on a macOS VM",
            discussion: "This is an experimental feature. Unattended configurations are specific to macOS versions and may not work across different releases.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "unattended", shortName: nil, help: "Preset name or path to YAML config file for unattended macOS Setup Assistant automation. Built-in presets: tahoe.", type: "String", defaultValue: nil, isOptional: false),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use or direct path to VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port to use for the VNC server. Defaults to 0 (auto-assign)", type: "Int", defaultValue: "0", isOptional: true),
                OptionDoc(name: "debug-dir", shortName: nil, help: "Custom directory for debug screenshots (defaults to unique folder in system temp)", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-display", shortName: nil, help: "Do not open the VNC client automatically", defaultValue: false),
                FlagDoc(name: "debug", shortName: nil, help: "Enable debug mode - saves screenshots with click coordinates", defaultValue: false),
            ],
            subcommands: []
        )
    }
}
