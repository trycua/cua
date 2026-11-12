// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
import Darwin
import Foundation
import Observation

/// `resources.get`'s snapshot from the SDK's `Spaces.resources` (the
/// daemon's one sampler): this Mac, each of its Spaces, your other
/// computers' summaries and what your Spaces on them take there, in the
/// page's `ResourcesSnapshot` shape (a missing optional is `NSNull`).
func resourcesJSON(_ s: CuaSDK.ResourcesSnapshot) -> [String: Any] {
    func warning(_ w: CuaSDK.DiskWarning) -> [String: Any] {
        let level: String
        switch w.level {
        case .ok: level = "ok"
        case .low: level = "low"
        case .critical: level = "critical"
        }
        return ["level": level, "free": w.free, "total": w.total, "threshold": w.threshold]
    }
    func computer(_ c: CuaSDK.ComputerResources) -> [String: Any] {
        [
            "id": c.id, "name": c.name, "online": c.online, "sampledAtMs": c.sampledAtMs,
            "cpuCount": c.cpuCount, "cpuPercent": c.cpuPercent, "loadAverage1m": c.loadAverage1m,
            "memoryTotal": c.memoryTotal, "memoryUsed": c.memoryUsed,
            "diskTotal": c.diskTotal, "diskFree": c.diskFree,
            "spacesCount": c.spacesCount, "spacesRunning": c.spacesRunning,
            "spacesCpuAllocated": c.spacesCpuAllocated, "spacesMemoryAllocated": c.spacesMemoryAllocated,
            "spacesMemoryUsed": c.spacesMemoryUsed, "spacesDiskBytes": c.spacesDiskBytes,
            "spacesDiskExact": c.spacesDiskExact, "warning": warning(c.warning),
        ]
    }
    func space(_ r: CuaSDK.SpaceResources) -> [String: Any] {
        var usage: Any = NSNull()
        if let u = r.usage {
            usage = [
                "cpuCount": u.cpuCount, "cpuPercent": u.cpuPercent, "loadAverage1m": u.loadAverage1m,
                "memoryUsed": u.memoryUsed, "memoryTotal": u.memoryTotal, "memoryLimited": u.memoryLimited,
                // No size of its own is null, never a sentinel.
                "diskUsed": u.diskUsed, "diskTotal": u.diskTotal.map { $0 as Any } ?? NSNull(),
                "diskLimited": u.diskLimited,
            ] as [String: Any]
        }
        var disk: Any = NSNull()
        if let d = r.diskOnHost {
            disk = ["bytes": d.bytes.map { $0 as Any } ?? NSNull(), "allocated": d.allocated,
                    "sharesSpace": d.sharesSpace] as [String: Any]
        }
        var base: Any = NSNull()
        if let b = r.base {
            base = ["key": b.key, "image": b.image, "bytes": b.bytes.map { $0 as Any } ?? NSNull(),
                    "sharedWith": b.sharedWith] as [String: Any]
        }
        return [
            "id": r.id, "powerState": r.powerState, "cpuAllocated": r.cpuAllocated,
            "memoryAllocated": r.memoryAllocated, "diskAllocated": r.diskAllocated,
            "usage": usage, "diskOnHost": disk, "base": base,
        ]
    }
    return [
        "sequence": s.sequence, "sampledAtMs": s.sampledAtMs, "thisMac": computer(s.thisMac),
        "spaces": s.spaces.map(space), "computers": s.computers.map(computer), "warning": warning(s.warning),
    ]
}

/// This Mac's processor, memory and disk (CUA-1880), read by the app itself:
/// the core count, how busy the cores were since the last read, memory in
/// use, and the free space on the volume that holds your home folder (where
/// Spaces live). Numbers only: never file names, paths or process names.
///
/// The page's `resources.get` answers the SDK's snapshot
/// (`resourcesJSON`); this sample is its answer only while the SDK can't
/// (starting, or the daemon is down), and the menu bar warning's.
public struct HostResourceSample: Equatable, Sendable {
    public var cores: Int
    /// 0 to 100 since the last sample; nil on the first.
    public var busyPercent: Double?
    public var memoryTotal: UInt64
    public var memoryUsed: UInt64?
    public var diskTotal: Int64
    public var diskFree: Int64

    public init(cores: Int, busyPercent: Double?, memoryTotal: UInt64, memoryUsed: UInt64?, diskTotal: Int64, diskFree: Int64) {
        self.cores = cores
        self.busyPercent = busyPercent
        self.memoryTotal = memoryTotal
        self.memoryUsed = memoryUsed
        self.diskTotal = diskTotal
        self.diskFree = diskFree
    }

    /// `resources.get`'s snapshot with this Mac only (the "API (as built)"
    /// `ResourcesSnapshot`): no Space numbers, for while the SDK can't
    /// answer.
    func snapshot(now: Date = Date()) -> [String: Any] {
        let ms = Int64(now.timeIntervalSince1970 * 1000)
        let warning = DiskWarning(free: diskFree, total: diskTotal).wire(free: diskFree, total: diskTotal)
        let busy = busyPercent ?? 0
        let thisMac: [String: Any] = [
            "id": "this-mac", "name": "", "online": true, "sampledAtMs": ms,
            "cpuCount": cores, "cpuPercent": busy, "loadAverage1m": Self.loadAverage(),
            "memoryTotal": memoryTotal, "memoryUsed": memoryUsed ?? 0,
            "diskTotal": diskTotal, "diskFree": diskFree,
            "spacesCount": 0, "spacesRunning": 0, "spacesCpuAllocated": 0, "spacesMemoryAllocated": 0,
            "spacesMemoryUsed": 0, "spacesDiskBytes": 0, "spacesDiskExact": true,
            "warning": warning,
        ]
        return ["sequence": ms / 10_000, "sampledAtMs": ms, "thisMac": thisMac, "spaces": [Any](), "computers": [Any](), "warning": warning]
    }

    static func loadAverage() -> Double {
        var loads = [Double](repeating: 0, count: 3)
        return getloadavg(&loads, 3) > 0 ? loads[0] : 0
    }
}

/// How close the disk is to full (the web UI's `diskWarningFor`, the spec's
/// thresholds): `low` under 50 GB or 15% free, whichever comes first, never
/// above 100 GB; `critical` under 5 GiB, where new Spaces already fail.
public enum DiskWarning: String, Equatable, Sendable {
    case none, low, critical

    /// The contract's `DiskWarning` (`level` is `ok`, `low` or `critical`).
    func wire(free: Int64, total: Int64) -> [String: Any] {
        let gb: Double = 1_000_000_000
        let threshold: Double
        switch self {
        case .none: threshold = 0
        case .low: threshold = min(max(50 * gb, Double(total) * 0.15), 100 * gb)
        case .critical: threshold = 5 * 1_073_741_824
        }
        return ["level": self == .none ? "ok" : rawValue, "free": free, "total": total, "threshold": Int64(threshold)]
    }

    public init(free: Int64, total: Int64) {
        let gb: Double = 1_000_000_000
        if Double(free) < 5 * 1_073_741_824 { self = .critical; return }
        let line = min(max(50 * gb, Double(total) * 0.15), 100 * gb)
        self = Double(free) < line ? .low : .none
    }

    /// The menu bar's words, or nil when there's room.
    public var menuText: String? {
        switch self {
        case .none: return nil
        case .low: return "This Mac is almost out of space. Delete old Spaces or snapshots."
        case .critical: return "This Mac is out of space. Delete old Spaces or snapshots."
        }
    }
}

/// Reads `HostResourceSample`s; keeps the last processor ticks so each read
/// says how busy the cores were since the one before.
public final class HostResourceSampler: @unchecked Sendable {
    public static let shared = HostResourceSampler()

    private let lock = NSLock()
    private var lastTicks: (busy: UInt64, total: UInt64)?
    private let volume: URL

    public init(volume: URL = FileManager.default.homeDirectoryForCurrentUser) {
        self.volume = volume
    }

    public func sample() -> HostResourceSample {
        let disk = Self.disk(volume)
        return HostResourceSample(
            cores: ProcessInfo.processInfo.activeProcessorCount,
            busyPercent: busy(),
            memoryTotal: ProcessInfo.processInfo.physicalMemory,
            memoryUsed: Self.memoryUsed(),
            diskTotal: disk.total,
            diskFree: disk.free
        )
    }

    /// Free and total bytes on the volume holding `url`.
    public static func disk(_ url: URL) -> (total: Int64, free: Int64) {
        let keys: Set<URLResourceKey> = [.volumeTotalCapacityKey, .volumeAvailableCapacityForImportantUsageKey, .volumeAvailableCapacityKey]
        guard let v = try? url.resourceValues(forKeys: keys) else { return (0, 0) }
        let free = v.volumeAvailableCapacityForImportantUsage ?? Int64(v.volumeAvailableCapacity ?? 0)
        return (Int64(v.volumeTotalCapacity ?? 0), free)
    }

    /// Active, wired and compressed memory.
    static func memoryUsed() -> UInt64? {
        var stats = vm_statistics64()
        var count = mach_msg_type_number_t(MemoryLayout<vm_statistics64>.size / MemoryLayout<integer_t>.size)
        let result = withUnsafeMutablePointer(to: &stats) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { host_statistics64(mach_host_self(), HOST_VM_INFO64, $0, &count) }
        }
        guard result == KERN_SUCCESS else { return nil }
        let page = UInt64(sysconf(_SC_PAGESIZE))
        return (UInt64(stats.active_count) + UInt64(stats.wire_count) + UInt64(stats.compressor_page_count)) * page
    }

    private func busy() -> Double? {
        var info = host_cpu_load_info()
        var count = mach_msg_type_number_t(MemoryLayout<host_cpu_load_info>.size / MemoryLayout<integer_t>.size)
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, $0, &count) }
        }
        guard result == KERN_SUCCESS else { return nil }
        let t = info.cpu_ticks
        let user = UInt64(t.0), system = UInt64(t.1), idle = UInt64(t.2), nice = UInt64(t.3)
        let now = (busy: user + system + nice, total: user + system + idle + nice)
        lock.lock()
        defer { lock.unlock() }
        let before = lastTicks
        lastTicks = now
        guard let before, now.total > before.total else { return nil }
        return min(100, max(0, Double(now.busy - before.busy) / Double(now.total - before.total) * 100))
    }
}

/// The menu bar's low-disk warning: This Mac's free space, read every 5
/// minutes (statvfs is cheap) whether or not a window is open. Nothing
/// when there's room.
@MainActor
@Observable
public final class LowDiskMonitor {
    public static let shared = LowDiskMonitor()

    public private(set) var warning: DiskWarning = .none
    @ObservationIgnored private var timer: Timer?
    @ObservationIgnored private let read: () -> (total: Int64, free: Int64)

    public init(read: @escaping () -> (total: Int64, free: Int64) = { HostResourceSampler.disk(FileManager.default.homeDirectoryForCurrentUser) }) {
        self.read = read
    }

    public static let interval: TimeInterval = 5 * 60

    /// Reads now, then every 5 minutes.
    public func start() {
        guard timer == nil else { return }
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: Self.interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
    }

    public func refresh() {
        let disk = read()
        // A volume that can't be read says nothing.
        warning = disk.total > 0 ? DiskWarning(free: disk.free, total: disk.total) : .none
    }
}
