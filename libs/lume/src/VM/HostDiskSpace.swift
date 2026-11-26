import Foundation

/// Free space on the host volume that holds a file.
protocol HostVolumeSpaceProbe {
    /// The volume holding `url` and the bytes it can still allocate, or nil
    /// when that cannot be determined.
    func volume(of url: URL) -> (volume: URL, availableBytes: UInt64)?
}

struct FoundationHostVolumeSpaceProbe: HostVolumeSpaceProbe {
    func volume(of url: URL) -> (volume: URL, availableBytes: UInt64)? {
        guard
            let values = try? url.resourceValues(forKeys: [
                .volumeURLKey, .volumeAvailableCapacityForImportantUsageKey,
            ]),
            let volume = values.volume,
            let available = values.volumeAvailableCapacityForImportantUsage
        else { return nil }
        return (volume, UInt64(max(0, available)))
    }
}

/// The host room a writable VM start needs.
///
/// A guest write into a sparse disk image, or into blocks an APFS clone still
/// shares with its source, allocates new blocks on the host volume. The guest
/// sees only the image's logical free space, so when the host volume fills,
/// its writes fail and its file system is damaged (#2767). A start therefore
/// needs `reserveBytes` free on each volume holding a writable disk, or the
/// disks' whole logical size when that is smaller.
enum HostDiskSpace {
    static let reserveBytes: UInt64 = 2 * 1024 * 1024 * 1024

    struct Shortfall: Equatable {
        let volume: String
        let requiredBytes: UInt64
        let availableBytes: UInt64
    }

    /// The first volume without the room the writable `disks` need, or nil.
    /// A volume whose free space cannot be read does not block the start.
    ///
    /// `growingLayers` are the disk-stack layers that take a stack's writes.
    /// One may not exist yet (the run creates it), and an ASIF layer's file
    /// size says nothing of how much the guest can still write into it, so
    /// each needs the full reserve on the volume of its directory.
    static func shortfall(
        writableDisks disks: [Path],
        growingLayers: [Path] = [],
        probe: HostVolumeSpaceProbe
    ) -> Shortfall? {
        var logicalBytes: [URL: UInt64] = [:]
        var available: [URL: UInt64] = [:]
        var order: [URL] = []
        func add(_ found: (volume: URL, availableBytes: UInt64), bytes: UInt64) {
            if logicalBytes[found.volume] == nil { order.append(found.volume) }
            logicalBytes[found.volume, default: 0] += bytes
            available[found.volume] = found.availableBytes
        }
        for disk in disks {
            guard let found = probe.volume(of: disk.url) else { continue }
            let size =
                (try? disk.url.resourceValues(forKeys: [.fileSizeKey]).fileSize)
                .map { UInt64(max(0, $0)) } ?? 0
            add(found, bytes: size)
        }
        for layer in growingLayers {
            guard let found = probe.volume(of: layer.url.deletingLastPathComponent()) else {
                continue
            }
            add(found, bytes: reserveBytes)
        }
        for volume in order {
            let required = min(logicalBytes[volume] ?? 0, reserveBytes)
            let free = available[volume] ?? 0
            if free < required {
                return Shortfall(
                    volume: volume.path(percentEncoded: false),
                    requiredBytes: required,
                    availableBytes: free)
            }
        }
        return nil
    }
}
