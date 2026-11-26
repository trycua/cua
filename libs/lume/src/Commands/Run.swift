import ArgumentParser
import Foundation
import Virtualization

struct Run: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Run a virtual machine"
    )

    @Argument(
        help: "Name of the virtual machine or image to pull and run (format: name or name:tag)",
        completion: .custom(completeVMName))
    var name: String

    @Option(
        help: "Local viewer to open: 'vnc', 'native', or 'none'. The VNC server remains available in every mode."
    )
    var display: DisplayMode = .native

    @Flag(
        name: [.customShort("d"), .customLong("no-display")],
        help: "Compatibility alias for --display none"
    )
    var noDisplay: Bool = false

    @Flag(
        name: .customLong("detach"),
        help: "Run the VM in the background and return immediately"
    )
    var detach: Bool = false

    @Option(
        name: .customLong("log-file"),
        help: "Log path for --detach (default: ~/Library/Logs/lume/{vm}.log)"
    )
    var logFile: String?

    @Option(
        name: [.customLong("shared-dir")],
        help:
            "Directory to share with the VM. Can be just a path for read-write access (e.g. ~/src) or path:tag where tag is 'ro' for read-only or 'rw' for read-write (e.g. ~/src:ro)"
    )
    var sharedDirectories: [String] = []

    @Flag(
        name: .customLong("flat-shared-dir"),
        help:
            "Show the only --shared-dir's contents directly in the guest's shared folder (on macOS, /Volumes/My Shared Files) instead of in a subfolder named after it. Needs exactly one --shared-dir, and the VM then cannot take more shared folders while it runs."
    )
    var flatSharedDir: Bool = false

    @Option(
        help:
            "For Linux VMs only, a read-only disk image to attach to the VM (e.g. --mount=\"ubuntu.iso\")",
        completion: .file())
    var mount: String?

    @Option(
        name: [.customLong("usb-storage")],
        help: "Disk image to attach as a USB mass storage device (e.g. --usb-storage=\"disk.img\")",
        completion: .file())
    var usbStorageDevices: [String] = []

    @Option(
        name: [.customLong("disk")],
        help: "Disk image to attach as a read-write virtio-blk device (e.g. --disk=\"scratch.img\")",
        completion: .file())
    var additionalDisks: [String] = []

    @Option(
        name: [.customLong("disk-stack")],
        help: ArgumentHelp(
            """
            Attach a disk as a read-only base plus DiskImageKit layers, in the form \
            <base>[+<layer>[@cache|@overlay]]... (e.g. --disk-stack="data-base.asif+session.asif"). \
            Layers default to @overlay. Only the topmost layer takes writes, so one base can back \
            many VMs at once, and a layer that does not exist yet is created. Requires macOS 27.
            """),
        completion: .file())
    var stackedDisks: [String] = []

    @Option(
        name: [.customLong("nbd-disk")],
        help: ArgumentHelp(
            """
            Attach a read-write virtio-blk disk served over NBD, after every other disk \
            (e.g. --nbd-disk="nbd+unix:///disk?socket=/path/disk.sock"). Repeatable.
            """))
    var nbdDisks: [String] = []

    @Option(
        name: [.customLong("nbd-root")],
        help: ArgumentHelp(
            """
            Boot from a root disk served over NBD instead of the VM's disk image \
            (e.g. --nbd-root="nbd+unix:///root?socket=/path/root.sock").
            """))
    var nbdRoot: String?

    @Option(
        name: [.customLong("nbd-timeout")],
        help: ArgumentHelp(
            "Seconds an NBD disk waits on a request before reconnecting. Defaults to 120.",
            discussion: """
                A flush after gigabytes of writes can take longer than a few seconds, and a \
                reconnect while requests are in flight can leave the guest's disk stuck. A \
                server that has gone away is still noticed at once: its socket refuses the \
                reconnect.
                """))
    var nbdTimeout: Double = 120

    @Option(
        name: [.customLong("stop-timeout")],
        help: ArgumentHelp(
            """
            On SIGINT or SIGTERM, ask the guest to shut down and give it this many seconds \
            before forcing the VM off. The command then exits 0 only if the guest shut down \
            by itself. Without this option a signal ends the process at once. `lume stop` \
            forces the VM off after its own --timeout (10 seconds by default), so give it a \
            longer one.
            """))
    var stopTimeout: Int?

    @Option(
        name: [.customLong("vsock-forward")],
        help: ArgumentHelp(
            """
            Forward a host TCP port to a guest AF_VSOCK port, as \
            [<IPv4 address>:]<hostPort>:<guestPort> (e.g. --vsock-forward=8000:5005). The port \
            listens on 127.0.0.1 unless an address is given (e.g. 0.0.0.0:8000:5005 accepts \
            connections from other hosts). Lets a guest daemon be reached without guest \
            networking — no DHCP wait, and no reliance on per-host NAT addresses. Repeatable.
            """))
    var vsockForwards: [String] = []

    @Option(help: "Github Container Registry to pull the images from. Defaults to ghcr.io")
    var registry: String = "ghcr.io"

    @Option(help: "Organization to pull the images from. Defaults to trycua")
    var organization: String = "trycua"

    @Option(
        name: .customLong("vnc"),
        help:
            "VNC server policy: 'enabled' (default) or 'disabled'. 'disabled' starts the VM with no VNC listener and reports a null vncUrl."
    )
    var vnc: VNCPolicy = .enabled

    @Option(
        name: [.customLong("vnc-port")],
        help: "Port to use for the VNC server. Defaults to 0 (auto-assign)")
    var vncPort: Int = 0

    @Option(
        name: [.customLong("vnc-password")],
        help: "Password for the VNC server. Defaults to a random passphrase")
    var vncPassword: String?

    @Option(help: "For MacOS VMs only, boot into the VM in recovery mode")
    var recoveryMode: Bool = false

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

    @Option(
        name: .customLong("disk-path"),
        help: "Override path to disk image file. When set, uses this file instead of the default disk.img in the VM directory.",
        completion: .file())
    var diskPath: String?

    @Option(
        name: .customLong("nvram-path"),
        help: "Override path to NVRAM file. When set, uses this file instead of the default nvram.bin in the VM directory.",
        completion: .file())
    var nvramPath: String?

    @Option(
        name: .customLong("network"),
        help: "Optional network override: 'nat', 'bridged', 'bridged:<interface>' (e.g. 'bridged:en0'), or 'gateway:<socket path>' (a Cua network gateway's datagram socket). Defaults to the VM's configured mode.")
    var network: String?

    @Flag(
        name: .customLong("clipboard"),
        help: "Enable bidirectional clipboard sync via SSH. This is automatic for native macOS display."
    )
    var clipboard: Bool = false

    @Flag(
        name: .customLong("no-audio-device"),
        help: "Leave the virtio sound device (host audio input and output) out of the VM, so the guest has no audio device."
    )
    var noAudioDevice: Bool = false

    @Flag(
        name: .customLong("no-clipboard-device"),
        help: "Leave the clipboard-sharing device (SPICE agent port) out of the VM and turn off clipboard sync, so the guest shares no clipboard with the host. Cannot be combined with --clipboard."
    )
    var noClipboardDevice: Bool = false

    var hostDevices: HostDeviceOptions {
        HostDeviceOptions(audio: !noAudioDevice, clipboard: !noClipboardDevice)
    }

    @Option(
        name: .customLong("time-vsock"),
        help:
            "Serve the host's UTC time to the guest on this vsock port (adds a virtio socket device). Each guest connection to the host (CID 2) on the port receives one line, 'time-v1 <seconds> <nanoseconds>', for guests that have no clock to read at boot. Off by default."
    )
    var timeVsock: UInt32?

    private var parsedNetworkMode: NetworkMode? {
        get throws {
            guard let network else {
                return nil
            }
            guard let mode = NetworkMode.parse(network) else {
                throw ValidationError(
                    "Invalid network mode '\(network)'. Expected 'nat', 'bridged', 'bridged:<interface>' or 'gateway:<socket path>'."
                )
            }
            return mode
        }
    }

    private var parsedSharedDirectories: [SharedDirectory] {
        get throws {
            try sharedDirectories.map { dirString -> SharedDirectory in
                let components = dirString.split(separator: ":", maxSplits: 1)
                let hostPath = String(components[0])

                // If no tag is provided, default to read-write
                if components.count == 1 {
                    return SharedDirectory(
                        hostPath: hostPath,
                        tag: VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag,
                        readOnly: false
                    )
                }

                // Parse the tag if provided
                let tag = String(components[1])
                let readOnly: Bool
                switch tag.lowercased() {
                case "ro":
                    readOnly = true
                case "rw":
                    readOnly = false
                default:
                    throw ValidationError(
                        "Invalid tag value. Must be either 'ro' for read-only or 'rw' for read-write"
                    )
                }

                return SharedDirectory(
                    hostPath: hostPath,
                    tag: VZVirtioFileSystemDeviceConfiguration.macOSGuestAutomountTag,
                    readOnly: readOnly
                )
            }
        }
    }

    private var parsedUSBStorageDevices: [Path] {
        usbStorageDevices.map { Path($0) }
    }

    private var parsedAdditionalDisks: [Path] {
        additionalDisks.map { Path($0) }
    }

    private func parsedStackedDisks() throws -> [StackedDiskSpec] {
        try stackedDisks.map { try StackedDiskSpec(argument: $0) }
    }

    private func parsedVsockForwards() throws -> [VsockPortForwarder.Rule] {
        try vsockForwards.map { try VsockPortForwarder.Rule(argument: $0) }
    }

    init() {
    }

    private func parsedNBDDisks() throws -> NBDDisks? {
        if nbdRoot == nil && nbdDisks.isEmpty {
            return nil
        }
        return try NBDDisks(root: nbdRoot, extra: nbdDisks, timeout: nbdTimeout)
    }

    func validate() throws {
        if clipboard && noClipboardDevice {
            throw ValidationError("'--clipboard' cannot be combined with '--no-clipboard-device'.")
        }
        if flatSharedDir && sharedDirectories.count != 1 {
            throw ValidationError("'--flat-shared-dir' needs exactly one '--shared-dir'.")
        }
        if let timeVsock, !GuestTimeService.isValidPort(timeVsock) {
            throw ValidationError(
                "Invalid --time-vsock port \(timeVsock). Expected 1 to \(GuestTimeService.validPorts.upperBound).")
        }
        if nbdTimeout <= 0 {
            throw ValidationError("--nbd-timeout must be positive")
        }
        // Checked here as well as in run(): with --detach, run() returns before
        // it builds the disks, and a bad URL would only show in the child's log.
        _ = try parsedNBDDisks()
        _ = try parsedVsockForwards()
        if let stopTimeout, stopTimeout <= 0 {
            throw ValidationError("--stop-timeout must be positive")
        }
        if let option = VNCPolicy.conflictingOption(
            policy: vnc,
            displayMode: DisplayMode.resolve(requested: display, noDisplay: noDisplay),
            vncPort: vncPort,
            vncPassword: vncPassword
        ) {
            throw ValidationError(
                "'--vnc disabled' starts no VNC server, so it cannot be combined with '\(option)'.")
        }
    }

    @MainActor
    func run() async throws {
        if detach {
            let result = try DetachedVMRunner.launch(vmName: name, logPath: logFile)
            print("Started '\(name)' in the background (PID \(result.processIdentifier)).")
            print("Log: \(result.logURL.path)")
            return
        }

        // Record telemetry
        let displayMode = DisplayMode.resolve(requested: display, noDisplay: noDisplay)
        TelemetryClient.shared.record(event: TelemetryEvent.run, properties: [
            "headless": displayMode == .none
        ])

        try await runVM(displayMode: displayMode)
    }

    @MainActor
    private func runVM(displayMode: DisplayMode) async throws {
        try await LumeController().runVM(
            name: name,
            noDisplay: noDisplay,
            displayMode: displayMode,
            sharedDirectories: parsedSharedDirectories,
            mount: mount.map { Path($0) },
            registry: registry,
            organization: organization,
            vncPort: vncPort,
            vncPassword: vncPassword,
            recoveryMode: recoveryMode,
            storage: storage,
            diskPath: diskPath.map { Path($0) },
            nvramPath: nvramPath.map { Path($0) },
            usbMassStoragePaths: parsedUSBStorageDevices.isEmpty ? nil : parsedUSBStorageDevices,
            additionalDiskPaths: parsedAdditionalDisks.isEmpty ? nil : parsedAdditionalDisks,
            stackedDiskSpecs: stackedDisks.isEmpty ? nil : try parsedStackedDisks(),
            vsockForwards: vsockForwards.isEmpty ? nil : try parsedVsockForwards(),
            nbdDisks: try parsedNBDDisks(),
            stopTimeout: stopTimeout.map { TimeInterval($0) },
            flatSharedDirectory: flatSharedDir,
            networkMode: parsedNetworkMode,
            clipboard: clipboard,
            vncPolicy: vnc,
            hostDevices: hostDevices,
            guestTimePort: timeVsock
        )
    }
}
