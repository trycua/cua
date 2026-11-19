import ArgumentParser
import Foundation
import Virtualization

/// Disks the VM reads from NBD servers instead of disk image files, such as
/// `nbd+unix:///disk?socket=/path/disk.sock`. Each is a virtio-blk device.
struct NBDDisks {
    /// The root disk, used instead of the VM's disk image.
    let root: URL?
    /// Extra disks, attached after every other disk.
    let extra: [URL]
    /// How long the framework waits on the server before it reconnects.
    let timeout: TimeInterval
    /// Watches this run's NBD disks.
    let monitor = NBDDiskMonitor()

    init(root: String?, extra: [String], timeout: TimeInterval) throws {
        self.root = try root.map(Self.url)
        self.extra = try extra.map(Self.url)
        self.timeout = timeout
    }

    private static func url(_ string: String) throws -> URL {
        guard let url = URL(string: string) else {
            throw ValidationError("Invalid NBD URL: \(string)")
        }
        try VZNetworkBlockDeviceStorageDeviceAttachment.validate(url)
        return url
    }

    /// The root disk's device, or nil if the root disk is the VM's disk image.
    func rootDevice() throws -> VZStorageDeviceConfiguration? {
        try root.map(device)
    }

    func extraDevices() throws -> [VZStorageDeviceConfiguration] {
        try extra.map(device)
    }

    /// A read-write virtio-blk device for url. Writes are synchronized with the
    /// server (full), so a flush in the guest reaches it.
    private func device(_ url: URL) throws -> VZStorageDeviceConfiguration {
        let attachment = try VZNetworkBlockDeviceStorageDeviceAttachment(
            url: url, timeout: timeout, isForcedReadOnly: false, synchronizationMode: .full)
        attachment.delegate = monitor
        return VZVirtioBlockDeviceConfiguration(attachment: attachment)
    }
}

/// Logs when an NBD disk connects and when it fails. The framework reconnects
/// on its own; a failure reported here is one it gave up on, so the guest has
/// lost the disk. The first one calls onFailure, which the run sets to force the
/// VM off, and the run must not end as a clean stop.
final class NBDDiskMonitor: NSObject, VZNetworkBlockDeviceStorageDeviceAttachmentDelegate,
    @unchecked Sendable
{
    private let lock = NSLock()
    private var _failed = false
    private var _onFailure: (@MainActor @Sendable () -> Void)?

    /// An NBD disk failed for good during this run.
    var failed: Bool { lock.withLock { _failed } }

    /// Runs on the main actor after the first failure.
    var onFailure: (@MainActor @Sendable () -> Void)? {
        get { lock.withLock { _onFailure } }
        set { lock.withLock { _onFailure = newValue } }
    }

    func attachmentWasConnected(_ attachment: VZNetworkBlockDeviceStorageDeviceAttachment) {
        Logger.info("NBD disk connected", metadata: ["url": attachment.url.absoluteString])
    }

    func attachment(
        _ attachment: VZNetworkBlockDeviceStorageDeviceAttachment,
        didEncounterError error: any Error
    ) {
        Logger.error(
            "NBD disk failed",
            metadata: ["url": attachment.url.absoluteString, "error": String(describing: error)])
        let hook = lock.withLock { () -> (@MainActor @Sendable () -> Void)? in
            defer { _failed = true }
            return _failed ? nil : _onFailure
        }
        if let hook {
            Task { @MainActor in hook() }
        }
    }
}
