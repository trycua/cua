import ArgumentParser
import Foundation

// MARK: - Documentation Types

/// One example invocation of a command, with an optional one-line description.
struct ExampleDoc: Codable {
    let command: String
    let description: String?

    init(_ command: String, _ description: String? = nil) {
        self.command = command
        self.description = description
    }
}

/// Represents documentation for a single CLI command
struct CommandDoc: Codable {
    let name: String
    let abstract: String
    let discussion: String?
    let arguments: [ArgumentDoc]
    let options: [OptionDoc]
    let flags: [FlagDoc]
    let subcommands: [CommandDoc]
    /// Example invocations. `CommandDocExtractorTests` parses every one with
    /// ArgumentParser, so an example cannot use a flag that does not exist.
    var examples: [ExampleDoc] = []
}

/// Represents documentation for a command argument
struct ArgumentDoc: Codable {
    let name: String
    let help: String
    let type: String
    let isOptional: Bool
    /// The accepted values, for arguments backed by an enum.
    var possibleValues: [String]? = nil
}

/// Represents documentation for a command option
struct OptionDoc: Codable {
    let name: String
    let shortName: String?
    let help: String
    let type: String
    let defaultValue: String?
    let isOptional: Bool
    /// The accepted values, for options backed by an enum.
    var possibleValues: [String]? = nil
    /// Takes every value up to the next option (`parsing: .upToNextOption`).
    var multipleValues: Bool? = nil
}

/// Represents documentation for a command flag
struct FlagDoc: Codable {
    let name: String
    let shortName: String?
    let help: String
    let defaultValue: Bool
}

/// One process exit status and what it means.
struct ExitCodeDoc: Codable {
    let code: Int32
    let meaning: String
}

/// Root documentation structure
struct CLIDocumentation: Codable {
    let name: String
    let version: String
    let abstract: String
    let exitCodes: [ExitCodeDoc]
    let commands: [CommandDoc]
}

// MARK: - Command Documentation Extractor

/// Extracts CLI documentation from command definitions.
///
/// The command tree below is hand-maintained (it carries value types, defaults
/// and examples that ArgumentParser does not expose). `CommandDocExtractorTests`
/// compares it with ArgumentParser's own metadata (`--experimental-dump-help`),
/// so a renamed, added or removed argument, option or flag fails the tests.
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
            exitCodes: exitCodes,
            commands: allCommandDocs
        )
    }

    /// Exit statuses: ArgumentParser maps `ValidationError` and parse errors
    /// to `EX_USAGE` (64), any other thrown error to `EXIT_FAILURE` (1).
    static let exitCodes: [ExitCodeDoc] = [
        ExitCodeDoc(code: 0, meaning: "Success"),
        ExitCodeDoc(code: 1, meaning: "The command failed (for example the VM does not exist or the operation errored)"),
        ExitCodeDoc(code: 64, meaning: "Usage error: an unknown command or option, a missing argument, or an invalid value"),
    ]

    // MARK: - Command Documentation Definitions

    /// All command documentation, kept in sync with source code.
    static var allCommandDocs: [CommandDoc] {
        return [
            createDoc,
            pullDoc,
            pushDoc,
            convertDoc,
            imagesDoc,
            cloneDoc,
            getDoc,
            setDoc,
            listDoc,
            runDoc,
            attachDoc,
            stopDoc,
            shutdownDoc,
            restartDoc,
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
            channelDoc,
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

    private static func values<T: CaseIterable & RawRepresentable>(_: T.Type) -> [String]
    where T.RawValue == String {
        T.allCases.map(\.rawValue)
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
                OptionDoc(name: "os", shortName: nil, help: "Operating system to install: macOS or linux", type: "String", defaultValue: "macOS", isOptional: true),
                OptionDoc(name: "cpu", shortName: nil, help: "Number of CPU cores", type: "Int", defaultValue: "4", isOptional: true),
                OptionDoc(name: "memory", shortName: nil, help: "Memory size (8, 8GB or 8192MB; a bare number is GB)", type: "String", defaultValue: "8GB", isOptional: true),
                OptionDoc(name: "disk-size", shortName: nil, help: "Disk size (100, 100GB or 102400MB; a bare number is GB). Defaults to 100GB for macOS and 50GB for Linux", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "display", shortName: nil, help: "Display resolution as WIDTHxHEIGHT", type: "String", defaultValue: "1024x768", isOptional: true),
                OptionDoc(name: "ipsw", shortName: nil, help: "Path to a macOS restore image (IPSW), or 'latest' to download the latest supported version. Required for macOS VMs", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "unattended", shortName: nil, help: "Prepare macOS unattended setup offline after install. Built-in presets: sequoia, tahoe; a YAML path is accepted for compatibility. macOS VMs only", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "debug-dir", shortName: nil, help: "Compatibility option; ignored by offline setup", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port for the temporary verification VNC server (0 picks a free port)", type: "Int", defaultValue: "0", isOptional: true),
                OptionDoc(name: "network", shortName: nil, help: "Network mode: nat, bridged (picks an interface), or bridged:<interface> (for example bridged:en0)", type: "String", defaultValue: "nat", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "debug", shortName: nil, help: "Compatibility flag; ignored by offline setup", defaultValue: false),
                FlagDoc(name: "no-display", shortName: nil, help: "Compatibility flag; offline setup verifies headlessly", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume create macos-tahoe --ipsw latest --unattended tahoe", "A macOS VM from the latest IPSW, set up unattended (SSH user lume/lume)"),
                ExampleDoc("lume create dev --os linux --cpu 4 --memory 8GB --disk-size 50GB", "A Linux VM"),
                ExampleDoc("lume create macos-tahoe --ipsw latest --network bridged:en0", "Bridge the VM onto the host's en0 network"),
            ]
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
                ArgumentDoc(name: "name", help: "Name for the resulting VM (defaults to the image name without its tag)", type: "String", isOptional: true),
            ],
            options: [
                OptionDoc(name: "registry", shortName: nil, help: "Container registry to pull from", type: "String", defaultValue: "ghcr.io", isOptional: true),
                OptionDoc(name: "organization", shortName: nil, help: "Organization to pull from", type: "String", defaultValue: "trycua", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "username", shortName: nil, help: "Registry username for authentication", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "password", shortName: nil, help: "Registry password for authentication", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "force", shortName: nil, help: "Download again even if the image is cached", defaultValue: false),
                FlagDoc(name: "verbose", shortName: nil, help: "Enable verbose logging", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume pull macos-tahoe-vanilla:latest", "Pull an image from ghcr.io/trycua as a VM named macos-tahoe-vanilla"),
                ExampleDoc("lume pull macos-tahoe-vanilla:latest my-vm --storage external", "Pull it as my-vm into another storage location"),
            ]
        )
    }

    // MARK: - Push

    private static var pushDoc: CommandDoc {
        CommandDoc(
            name: "push",
            abstract: "Push a macOS VM to an OCI-compatible registry",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM to push", type: "String", isOptional: false),
                ArgumentDoc(name: "image", help: "Image tag to push (format: name:tag)", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "additional-tags", shortName: nil, help: "Additional tags to push the same image to", type: "[String]", defaultValue: nil, isOptional: true, multipleValues: true),
                OptionDoc(name: "registry", shortName: nil, help: "Container registry to push to", type: "String", defaultValue: "ghcr.io", isOptional: true),
                OptionDoc(name: "organization", shortName: nil, help: "Organization to push to", type: "String", defaultValue: "trycua", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "chunk-size-mb", shortName: nil, help: "Chunk size for large files, in MB", type: "Int", defaultValue: "512", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "verbose", shortName: nil, help: "Enable verbose logging", defaultValue: false),
                FlagDoc(name: "dry-run", shortName: nil, help: "Prepare files without uploading to the registry", defaultValue: false),
                FlagDoc(name: "reassemble", shortName: nil, help: "With --dry-run, also reassemble the chunks to verify integrity", defaultValue: false),
                FlagDoc(name: "single-layer", shortName: nil, help: "Push the disk as a single layer (kubelet-compatible, no chunking)", defaultValue: false),
                FlagDoc(name: "legacy", shortName: nil, help: "Use the legacy Lume LZ4-chunked format instead of the OCI-compliant format", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume push my-vm my-image:latest --organization my-org", "Push my-vm as ghcr.io/my-org/my-image:latest"),
                ExampleDoc("lume push my-vm my-image:1.0 --additional-tags latest stable", "Push one image under several tags"),
                ExampleDoc("lume push my-vm my-image:latest --dry-run --reassemble", "Prepare and verify the chunks without uploading"),
            ]
        )
    }

    // MARK: - Convert

    private static var convertDoc: CommandDoc {
        CommandDoc(
            name: "convert",
            abstract: "Convert a legacy Lume image to OCI-compliant format",
            discussion: "Pulls a legacy Lume image from the registry, pushes it again in OCI-compliant format under a new name and tag, then removes the temporary local VM.",
            arguments: [
                ArgumentDoc(name: "source-image", help: "Source image to convert (legacy format, for example macos-tahoe:latest)", type: "String", isOptional: false),
                ArgumentDoc(name: "target-image", help: "Target image to push in OCI format (format: name:tag)", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "additional-tags", shortName: nil, help: "Additional tags to push the OCI image to", type: "[String]", defaultValue: nil, isOptional: true, multipleValues: true),
                OptionDoc(name: "registry", shortName: nil, help: "Registry to pull from and push to", type: "String", defaultValue: "ghcr.io", isOptional: true),
                OptionDoc(name: "organization", shortName: nil, help: "Registry organization", type: "String", defaultValue: "trycua", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "verbose", shortName: nil, help: "Enable verbose logging", defaultValue: false),
                FlagDoc(name: "dry-run", shortName: nil, help: "Prepare files without uploading to the registry", defaultValue: false),
                FlagDoc(name: "single-layer", shortName: nil, help: "Push the disk as a single layer (kubelet-compatible, no chunking)", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume convert macos-tahoe:latest macos-tahoe:latest-oci"),
            ]
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
                OptionDoc(name: "organization", shortName: nil, help: "Organization to list images for", type: "String", defaultValue: "trycua", isOptional: true),
            ],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume images"),
            ]
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
            subcommands: [],
            examples: [
                ExampleDoc("lume clone macos-tahoe macos-tahoe-backup", "Keep a golden copy before changing a VM"),
                ExampleDoc("lume clone macos-tahoe macos-tahoe --source-storage default --dest-storage external", "Copy a VM to another storage location"),
            ]
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
                OptionDoc(name: "format", shortName: "f", help: "Output format", type: "String", defaultValue: "text", isOptional: true, possibleValues: values(FormatOption.self)),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume get my-vm"),
                ExampleDoc("lume get my-vm --format json", "Machine-readable, including the IP address and VNC URL"),
            ]
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
                OptionDoc(name: "memory", shortName: nil, help: "New memory size (8, 8GB or 8192MB; a bare number is GB)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "disk-size", shortName: nil, help: "New total disk size, increase only. For macOS VMs this relocates the recovery partition and grows the main APFS container; the VM must be stopped and it may take several minutes", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "display", shortName: nil, help: "New display resolution as WIDTHxHEIGHT", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "machine-identifier", shortName: nil, help: "New machine identifier: 'random', or a base64 identifier from `lume get --format json` to give this VM an existing machine identity (per-machine licenses then see one machine). macOS VMs only; the VM must be stopped", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "mac-address", shortName: nil, help: "New MAC address: 'random' (locally administered) or aa:bb:cc:dd:ee:ff. The VM must be stopped. The DHCP lease follows the MAC, so two running VMs with one MAC claim the same IP", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-backup", shortName: nil, help: "Skip the pre-resize disk backup (macOS resize only). Faster, but a failure cannot be rolled back", defaultValue: false),
                FlagDoc(name: "keep-backup", shortName: nil, help: "Keep the pre-resize backup after a successful macOS resize", defaultValue: false),
                FlagDoc(name: "dry-run", shortName: nil, help: "Validate the disk-resize plan and print it without changing anything (macOS resize only)", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume set my-vm --cpu 8 --memory 16GB"),
                ExampleDoc("lume set my-vm --disk-size 120GB --dry-run", "Check a disk resize plan, then run it without --dry-run"),
                ExampleDoc("lume set my-vm --mac-address random"),
            ]
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
                OptionDoc(name: "format", shortName: "f", help: "Output format", type: "String", defaultValue: "text", isOptional: true, possibleValues: values(FormatOption.self)),
                OptionDoc(name: "storage", shortName: nil, help: "Show only VMs in this storage location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume ls"),
                ExampleDoc("lume ls --format json"),
            ]
        )
    }

    // MARK: - Run

    private static var runDoc: CommandDoc {
        CommandDoc(
            name: "run",
            abstract: "Run a virtual machine",
            discussion: nil,
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the VM, or an image to pull and run (format: name or name:tag)", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "display", shortName: nil, help: "Local viewer to open. The VNC server stays available in every mode", type: "DisplayMode", defaultValue: "native", isOptional: true, possibleValues: values(DisplayMode.self)),
                OptionDoc(name: "log-file", shortName: nil, help: "Log path for --detach (default: ~/Library/Logs/lume/{vm}.log)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "shared-dir", shortName: nil, help: "Directory to share with the VM: a path (read-write) or path:ro / path:rw. Repeatable", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "mount", shortName: nil, help: "For Linux VMs only, a read-only disk image to attach", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "usb-storage", shortName: nil, help: "Disk image to attach as a USB mass storage device. Repeatable", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "disk", shortName: nil, help: "Disk image to attach as a read-write virtio-blk device. Repeatable", type: "[String]", defaultValue: nil, isOptional: true),
                OptionDoc(name: "registry", shortName: nil, help: "Container registry to pull images from", type: "String", defaultValue: "ghcr.io", isOptional: true),
                OptionDoc(name: "organization", shortName: nil, help: "Organization to pull images from", type: "String", defaultValue: "trycua", isOptional: true),
                OptionDoc(name: "vnc", shortName: nil, help: "VNC server policy. disabled starts the VM with no VNC listener and reports a null vncUrl", type: "VNCPolicy", defaultValue: "enabled", isOptional: true, possibleValues: values(VNCPolicy.self)),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port for the VNC server (0 picks a free port)", type: "Int", defaultValue: "0", isOptional: true),
                OptionDoc(name: "vnc-password", shortName: nil, help: "Password for the VNC server (default: a random passphrase)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "recovery-mode", shortName: nil, help: "For macOS VMs only, boot into recovery mode", type: "Bool", defaultValue: "false", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "disk-path", shortName: nil, help: "Use this disk image instead of disk.img in the VM directory", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "nvram-path", shortName: nil, help: "Use this NVRAM file instead of nvram.bin in the VM directory", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "network", shortName: nil, help: "Network override for this run: nat, bridged, or bridged:<interface> (default: the VM's configured mode)", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-display", shortName: "d", help: "Compatibility alias for --display none", defaultValue: false),
                FlagDoc(name: "detach", shortName: nil, help: "Run the VM in the background and return immediately", defaultValue: false),
                FlagDoc(name: "clipboard", shortName: nil, help: "Sync the clipboard both ways over SSH. Automatic with the native macOS display", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume run my-vm"),
                ExampleDoc("lume run my-vm --shared-dir ~/src:ro --display none", "Headless, with a read-only shared folder"),
                ExampleDoc("lume run my-vm --detach --display none --vnc disabled", "In the background with no VNC listener"),
            ]
        )
    }

    // MARK: - Attach

    private static var attachDoc: CommandDoc {
        CommandDoc(
            name: "attach",
            abstract: "Open a viewer for a running virtual machine",
            discussion: "The native display is used when the `lume run` process that owns the VM supports live attachment; VNC is the fallback.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "display", shortName: nil, help: "Viewer to open (default: native, falling back to VNC)", type: "AttachDisplayMode", defaultValue: nil, isOptional: true, possibleValues: values(AttachDisplayMode.self)),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume attach my-vm"),
                ExampleDoc("lume attach my-vm --display vnc", "Open Screen Sharing over VNC"),
            ]
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
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "timeout", shortName: nil, help: "Seconds to wait for a graceful shutdown before forcing power off", type: "Int", defaultValue: "10", isOptional: false),
            ],
            flags: [
                FlagDoc(name: "force", shortName: nil, help: "Power off the VM immediately instead of attempting a graceful shutdown first", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume stop my-vm"),
                ExampleDoc("lume stop my-vm --force"),
            ]
        )
    }

    // MARK: - Guest power

    private static var shutdownDoc: CommandDoc {
        guestPowerDoc(
            name: "shutdown",
            abstract: "Gracefully shut down a virtual machine"
        )
    }

    private static var restartDoc: CommandDoc {
        guestPowerDoc(
            name: "restart",
            abstract: "Gracefully restart a virtual machine"
        )
    }

    private static func guestPowerDoc(name: String, abstract: String) -> CommandDoc {
        CommandDoc(
            name: name,
            abstract: abstract,
            discussion: "Requests the operation inside the guest over SSH. VMs created with --unattended use lume/lume credentials by default.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "user", shortName: "u", help: "SSH username", type: "String", defaultValue: "lume", isOptional: true),
                OptionDoc(name: "password", shortName: "p", help: "SSH and sudo password", type: "String", defaultValue: "lume", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location to use", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "timeout", shortName: "t", help: "SSH command timeout in seconds", type: "Int", defaultValue: "30", isOptional: true),
            ],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume \(name) my-vm"),
                ExampleDoc("lume \(name) my-vm --user admin --timeout 60", "With another SSH user and a longer timeout"),
            ]
        )
    }

    // MARK: - SSH

    private static var sshDoc: CommandDoc {
        CommandDoc(
            name: "ssh",
            abstract: "Connect to a VM via SSH or execute commands remotely",
            discussion: "Opens an interactive shell, or runs one command and exits with its status. Password authentication is handled for you (no sshpass needed). Requires Remote Login in the guest; VMs created with --unattended have it enabled with credentials lume/lume.",
            arguments: [
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
                ArgumentDoc(name: "command", help: "Command to execute (omit for an interactive shell)", type: "[String]", isOptional: true),
            ],
            options: [
                OptionDoc(name: "user", shortName: "u", help: "SSH username", type: "String", defaultValue: "lume", isOptional: true),
                OptionDoc(name: "password", shortName: "p", help: "SSH password", type: "String", defaultValue: "lume", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "Storage location name or path", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "timeout", shortName: "t", help: "Command timeout in seconds (0 for no timeout)", type: "Int", defaultValue: "60", isOptional: true),
            ],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume ssh my-vm", "Interactive shell"),
                ExampleDoc("lume ssh my-vm \"ls -la\"", "Run one command"),
                ExampleDoc("lume ssh my-vm --timeout 0 \"cd /app && npm test\"", "A long-running command with no timeout"),
            ]
        )
    }

    // MARK: - SIP

    private static var sipDoc: CommandDoc {
        CommandDoc(
            name: "sip",
            abstract: "Enable or disable System Integrity Protection on a macOS VM",
            discussion: "Boots the stopped VM normally to validate the admin credentials, runs `csrutil disable` (or `enable`) in paired recoveryOS over VNC, then boots normally once more to verify the result. The VM needs an admin account and Remote Login; VMs from unattended setup use lume/lume. Prefer --admin-password-stdin: --admin-password is visible to other processes. Requires vncdotool (pip3 install vncdotool).",
            arguments: [
                ArgumentDoc(name: "state", help: "Desired SIP state", type: "String", isOptional: false, possibleValues: values(Sip.State.self)),
                ArgumentDoc(name: "name", help: "Name of the virtual machine", type: "String", isOptional: false),
            ],
            options: [
                OptionDoc(name: "admin-user", shortName: nil, help: "Administrator username in the guest", type: "String", defaultValue: "lume", isOptional: true),
                OptionDoc(name: "admin-password", shortName: nil, help: "Administrator password in the guest (default: lume); prefer --admin-password-stdin", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "screenshot-dir", shortName: nil, help: "Save step-by-step framebuffer PNGs here for debugging", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "TCP port for the temporary recovery VNC server", type: "Int", defaultValue: "5999", isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "timeout", shortName: nil, help: "Overall timeout in seconds", type: "Int", defaultValue: "900", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "yes", shortName: "y", help: "Skip the interactive confirmation prompt", defaultValue: false),
                FlagDoc(name: "admin-password-stdin", shortName: nil, help: "Read one administrator-password line from standard input without echo", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume sip off my-vm --yes"),
                ExampleDoc("lume sip on my-vm --yes --admin-user alice --admin-password-stdin", "With another admin account, password read from stdin"),
            ]
        )
    }

    // MARK: - IPSW

    private static var ipswDoc: CommandDoc {
        CommandDoc(
            name: "ipsw",
            abstract: "Get macOS restore image IPSW URL",
            discussion: "Prints the URL of the latest supported restore image. Download it, then pass the file to `lume create --ipsw`.",
            arguments: [],
            options: [],
            flags: [],
            subcommands: [],
            examples: [
                ExampleDoc("lume ipsw"),
            ]
        )
    }

    // MARK: - Serve

    private static var serveDoc: CommandDoc {
        CommandDoc(
            name: "serve",
            abstract: "Start the VM management server",
            discussion: "Serves the HTTP API on localhost, or with --mcp an MCP server over stdio for AI agents.",
            arguments: [],
            options: [
                OptionDoc(name: "port", shortName: nil, help: "Port to listen on (HTTP mode only)", type: "Int", defaultValue: "7777", isOptional: true),
            ],
            flags: [
                FlagDoc(name: "mcp", shortName: nil, help: "Run as an MCP server (stdio transport) for AI agents", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume serve", "HTTP API on http://localhost:7777"),
                ExampleDoc("lume serve --port 7778"),
                ExampleDoc("lume serve --mcp", "MCP server over stdio (what MCP clients launch)"),
            ]
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
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "force", shortName: nil, help: "Delete without asking for confirmation", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume delete my-vm"),
                ExampleDoc("lume delete my-vm --force", "Without the confirmation prompt"),
            ]
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
            subcommands: [],
            examples: [
                ExampleDoc("lume prune"),
            ]
        )
    }

    // MARK: - Config

    private static func leaf(
        _ name: String, _ abstract: String, arguments: [ArgumentDoc] = [], options: [OptionDoc] = [],
        examples: [ExampleDoc]
    ) -> CommandDoc {
        CommandDoc(
            name: name, abstract: abstract, discussion: nil, arguments: arguments, options: options,
            flags: [], subcommands: [], examples: examples)
    }

    private static var configDoc: CommandDoc {
        CommandDoc(
            name: "config",
            abstract: "Get or set lume configuration",
            discussion: "Without a subcommand, prints the current configuration (`lume config get`).",
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                leaf("get", "Get current configuration", examples: [ExampleDoc("lume config get")]),
                CommandDoc(
                    name: "storage",
                    abstract: "Manage VM storage locations",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        leaf("add", "Add a new VM storage location", arguments: [
                            ArgumentDoc(name: "name", help: "Storage name (letters, digits, dashes and underscores)", type: "String", isOptional: false),
                            ArgumentDoc(name: "path", help: "Path to the VM storage directory", type: "String", isOptional: false),
                        ], examples: [ExampleDoc("lume config storage add external /Volumes/External/lume")]),
                        leaf("remove", "Remove a VM storage location", arguments: [
                            ArgumentDoc(name: "name", help: "Storage name to remove", type: "String", isOptional: false),
                        ], examples: [ExampleDoc("lume config storage remove external")]),
                        leaf("list", "List all VM storage locations", examples: [ExampleDoc("lume config storage list")]),
                        leaf("default", "Set the default VM storage location", arguments: [
                            ArgumentDoc(name: "name", help: "Storage name to set as default", type: "String", isOptional: false),
                        ], examples: [ExampleDoc("lume config storage default external")]),
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
                        leaf("dir", "Get or set cache directory", arguments: [
                            ArgumentDoc(name: "path", help: "Path to the cache directory (omit to show the current one)", type: "String", isOptional: true),
                        ], examples: [ExampleDoc("lume config cache dir"), ExampleDoc("lume config cache dir /Volumes/External/lume-cache")]),
                        leaf("enable", "Enable image caching", examples: [ExampleDoc("lume config cache enable")]),
                        leaf("disable", "Disable image caching", examples: [ExampleDoc("lume config cache disable")]),
                        leaf("status", "Show cache status and directory", examples: [ExampleDoc("lume config cache status")]),
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
                        leaf("status", "Show current telemetry status", examples: [ExampleDoc("lume config telemetry status")]),
                        leaf("enable", "Enable pseudonymous telemetry", examples: [ExampleDoc("lume config telemetry enable")]),
                        leaf("disable", "Disable pseudonymous telemetry", examples: [ExampleDoc("lume config telemetry disable")]),
                        leaf("reset-id", "Delete the pseudonymous installation ID and registration markers", examples: [ExampleDoc("lume config telemetry reset-id")]),
                    ]
                ),
                CommandDoc(
                    name: "registry",
                    abstract: "Manage container registry settings",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        leaf("status", "Show current registry configuration", examples: [ExampleDoc("lume config registry status")]),
                        leaf("type", "Get or set registry type", arguments: [
                            ArgumentDoc(name: "type", help: "Registry type to use: ghcr or gcs (omit to show the current one)", type: "String", isOptional: true),
                        ], examples: [ExampleDoc("lume config registry type"), ExampleDoc("lume config registry type ghcr")]),
                        leaf("ghcr", "Configure GitHub Container Registry settings", options: [
                            OptionDoc(name: "registry", shortName: nil, help: "Registry URL (default: ghcr.io)", type: "String", defaultValue: nil, isOptional: true),
                            OptionDoc(name: "organization", shortName: nil, help: "Organization or namespace (default: trycua)", type: "String", defaultValue: nil, isOptional: true),
                        ], examples: [ExampleDoc("lume config registry ghcr --organization my-org")]),
                        leaf("gcs", "Configure GCS registry settings", options: [
                            OptionDoc(name: "api-url", shortName: nil, help: "API URL for signed URL generation", type: "String", defaultValue: nil, isOptional: true),
                            OptionDoc(name: "api-key", shortName: nil, help: "API key for authentication", type: "String", defaultValue: nil, isOptional: true),
                        ], examples: [ExampleDoc("lume config registry gcs --api-url https://images.example.com --api-key <key>")]),
                    ]
                ),
                CommandDoc(
                    name: "network",
                    abstract: "Manage network settings",
                    discussion: nil,
                    arguments: [],
                    options: [],
                    flags: [],
                    subcommands: [
                        leaf("interfaces", "List available network interfaces for bridged networking", examples: [ExampleDoc("lume config network interfaces")]),
                    ]
                ),
            ]
        )
    }

    // MARK: - Logs

    private static func logsLeaf(_ name: String, _ abstract: String, files: String) -> CommandDoc {
        CommandDoc(
            name: name,
            abstract: abstract,
            discussion: nil,
            arguments: [],
            options: [
                OptionDoc(name: "lines", shortName: "l", help: "Number of lines to display from the end of \(files)", type: "Int", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "follow", shortName: "f", help: "Follow the log continuously (like tail -f)", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume logs \(name) --lines 100"),
                ExampleDoc("lume logs \(name) --follow"),
            ]
        )
    }

    private static var logsDoc: CommandDoc {
        CommandDoc(
            name: "logs",
            abstract: "View lume serve logs",
            discussion: nil,
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                logsLeaf("info", "View info logs from the daemon", files: "the file"),
                logsLeaf("error", "View error logs from the daemon", files: "the file"),
                logsLeaf("all", "View both info and error logs from the daemon", files: "each file"),
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
            subcommands: [],
            examples: [
                ExampleDoc("lume check-update"),
                ExampleDoc("lume check-update --json --no-cache"),
            ]
        )
    }

    private static var updateDoc: CommandDoc {
        CommandDoc(
            name: "update",
            abstract: "Check for a Lume update and optionally apply it",
            discussion: "Without --apply, this command only checks for a newer release and prints the command to install it. With --apply, it runs the official Lume installer pinned to the discovered version and exits with the installer's status.",
            arguments: [],
            options: [],
            flags: [
                FlagDoc(name: "apply", shortName: nil, help: "Apply the update by re-running the official installer", defaultValue: false),
                FlagDoc(name: "json", shortName: nil, help: "Emit the structured update-state payload as JSON", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume update"),
                ExampleDoc("lume update --apply"),
            ]
        )
    }

    private static var channelDoc: CommandDoc {
        CommandDoc(
            name: "channel",
            abstract: "Inspect or change the stable/nightly update channel; selection does not install",
            discussion: "The selection persists but never installs by itself; run `lume update --apply` after changing it. Without a subcommand, prints the status.",
            arguments: [],
            options: [],
            flags: [],
            subcommands: [
                CommandDoc(
                    name: "status",
                    abstract: "Show selected and current release channels",
                    discussion: nil,
                    arguments: [], options: [],
                    flags: [FlagDoc(name: "json", shortName: nil, help: "Emit machine-readable channel state as JSON", defaultValue: false)],
                    subcommands: [],
                    examples: [ExampleDoc("lume channel status --json")]
                ),
                CommandDoc(
                    name: "set",
                    abstract: "Save stable or nightly as the update channel",
                    discussion: nil,
                    arguments: [ArgumentDoc(name: "channel", help: "Release channel: stable or nightly", type: "String", isOptional: false)],
                    options: [],
                    flags: [FlagDoc(name: "json", shortName: nil, help: "Emit machine-readable channel state as JSON", defaultValue: false)],
                    subcommands: [],
                    examples: [ExampleDoc("lume channel set nightly"), ExampleDoc("lume channel set stable")]
                ),
            ]
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
                OptionDoc(name: "unattended", shortName: nil, help: "Built-in preset (sequoia, tahoe) or a YAML path, kept for compatibility and optional post-SSH commands (default: tahoe)", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "storage", shortName: nil, help: "VM storage location name, or a direct path to the VM location", type: "String", defaultValue: nil, isOptional: true),
                OptionDoc(name: "vnc-port", shortName: nil, help: "Port for the temporary verification VNC server (0 picks a free port)", type: "Int", defaultValue: "0", isOptional: true),
                OptionDoc(name: "debug-dir", shortName: nil, help: "Compatibility option; ignored by offline setup", type: "String", defaultValue: nil, isOptional: true),
            ],
            flags: [
                FlagDoc(name: "no-display", shortName: nil, help: "Compatibility flag; offline setup verifies headlessly", defaultValue: false),
                FlagDoc(name: "debug", shortName: nil, help: "Compatibility flag; ignored by offline setup", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume setup my-vm"),
                ExampleDoc("lume setup my-vm --unattended sequoia"),
            ]
        )
    }

    // MARK: - Dump Docs

    private static var dumpDocsDoc: CommandDoc {
        CommandDoc(
            name: "dump-docs",
            abstract: "Output CLI and API documentation as JSON for tooling and integrations",
            discussion: "Emits the CLI, HTTP API or MCP tool metadata (arguments, options, flags, endpoints, tool schemas, help text, defaults and types) that the generated reference is built from.",
            arguments: [],
            options: [
                OptionDoc(name: "type", shortName: nil, help: "Documentation type to output", type: "String", defaultValue: "cli", isOptional: true, possibleValues: values(DocType.self)),
            ],
            flags: [
                FlagDoc(name: "pretty", shortName: nil, help: "Pretty-print the JSON output with sorted keys", defaultValue: false),
            ],
            subcommands: [],
            examples: [
                ExampleDoc("lume dump-docs --type cli --pretty"),
                ExampleDoc("lume dump-docs --type mcp", "The MCP server's tool list"),
            ]
        )
    }
}
