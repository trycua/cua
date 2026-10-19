// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Foundation
import Testing
@testable import CuaSpacesMacKit

@Suite("Host resources and the low-disk warning")
struct HostResourcesTests {
    let gb: Int64 = 1_000_000_000

    @Test("warns under 50 GB or 15% free, capped at 100 GB; critical under 5 GiB")
    func thresholds() {
        // 1 TB: 15% is 150 GB, capped at 100 GB.
        #expect(DiskWarning(free: 120 * gb, total: 1000 * gb) == .none)
        #expect(DiskWarning(free: 90 * gb, total: 1000 * gb) == .low)
        // 256 GB: 15% is 38 GB, so the 50 GB line comes first.
        #expect(DiskWarning(free: 45 * gb, total: 256 * gb) == .low)
        #expect(DiskWarning(free: 60 * gb, total: 256 * gb) == .none)
        // 500 GB: 15% is 75 GB.
        #expect(DiskWarning(free: 70 * gb, total: 500 * gb) == .low)
        #expect(DiskWarning(free: 4 * gb, total: 1000 * gb) == .critical)
    }

    @Test("the menu bar says nothing when there's room")
    func menuText() {
        #expect(DiskWarning.none.menuText == nil)
        #expect(DiskWarning.low.menuText == "This Mac is almost out of space. Delete old Spaces or snapshots.")
        #expect(DiskWarning.critical.menuText?.hasPrefix("This Mac is out of space") == true)
    }

    @Test @MainActor
    func monitorReadsTheDisk() {
        var free: Int64 = 300 * gb
        let monitor = LowDiskMonitor(read: { (1000 * gb, free) })
        monitor.refresh()
        #expect(monitor.warning == .none)
        free = 41 * gb
        monitor.refresh()
        #expect(monitor.warning == .low)
        // A volume it can't read: no warning.
        let unreadable = LowDiskMonitor(read: { (0, 0) })
        unreadable.refresh()
        #expect(unreadable.warning == .none)
    }

    @Test("a sample reads real numbers, and the second says how busy")
    func sample() {
        let sampler = HostResourceSampler()
        let first = sampler.sample()
        #expect(first.cores > 0)
        #expect(first.memoryTotal > 0)
        #expect(first.diskTotal > 0 && first.diskFree >= 0)
        #expect(first.busyPercent == nil)
        let second = sampler.sample()
        if let busy = second.busyPercent { #expect(busy >= 0 && busy <= 100) }
        let snap = second.snapshot()
        let thisMac = snap["thisMac"] as? [String: Any]
        #expect(thisMac?["id"] as? String == "this-mac")
        #expect((snap["spaces"] as? [Any])?.isEmpty == true)
        #expect(["ok", "low", "critical"].contains((snap["warning"] as? [String: Any])?["level"] as? String ?? ""))
    }
}
