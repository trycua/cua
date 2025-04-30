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

    @Flag(name: [.short, .long], help: "Do not start the VNC client")
    var noDisplay: Bool = false

    @Option(
        name: [.customLong("shared-dir")],
        help:
            "Directory to share with the VM. Can be just a path for read-write access (e.g. ~/src) or path:tag where tag is 'ro' for read-only or 'rw' for read-write (e.g. ~/src:ro)"
    )
    var sharedDirectories: [String] = []

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

    @Option(help: "Github Container Registry to pull the images from. Defaults to ghcr.io")
    var registry: String = "ghcr.io"

    @Option(help: "Organization to pull the images from. Defaults to trycua")
    var organization: String = "trycua"

    @Option(
        name: [.customLong("vnc-port")],
        help: "Port to use for the VNC server. Defaults to 0 (auto-assign)")
    var vncPort: Int = 0

    @Option(help: "For MacOS VMs only, boot into the VM in recovery mode")
    var recoveryMode: Bool = false

    @Option(name: .customLong("storage"), help: "VM storage location to use or direct path to VM location")
    var storage: String?

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

    init() {
    }

    @MainActor
    func run() async throws {
        try await LumeController().runVM(
            name: name,
            noDisplay: noDisplay,
            sharedDirectories: parsedSharedDirectories,
            mount: mount.map { Path($0) },
            registry: registry,
            organization: organization,
            vncPort: vncPort,
            recoveryMode: recoveryMode,
            storage: storage,
            usbMassStoragePaths: parsedUSBStorageDevices.isEmpty ? nil : parsedUSBStorageDevices
        )
    }
}
