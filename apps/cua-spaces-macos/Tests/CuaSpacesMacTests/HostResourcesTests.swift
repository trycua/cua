// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import CuaSDK
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

    @Test("resources.get answers the SDK's snapshot in the page's shape: every Space, here and on other computers")
    func sdkSnapshot() throws {
        let g: UInt64 = 1_000_000_000
        let warning = CuaSDK.DiskWarning(level: .low, free: 41 * g, total: 1000 * g, threshold: 100 * g)
        func computer(_ id: String, _ name: String) -> CuaSDK.ComputerResources {
            CuaSDK.ComputerResources(
                id: id, name: name, online: true, sampledAtMs: 7, cpuCount: 12, cpuPercent: 30.5, loadAverage1m: 2.5,
                memoryTotal: 64 * g, memoryUsed: 41 * g, diskTotal: 1000 * g, diskFree: 41 * g,
                spacesCount: 1, spacesRunning: 1, spacesCpuAllocated: 4, spacesMemoryAllocated: 8 * g,
                spacesMemoryUsed: 3 * g, spacesDiskBytes: 19 * g, spacesDiskExact: true, warning: warning)
        }
        let snapshot = CuaSDK.ResourcesSnapshot(
            sequence: 9, sampledAtMs: 7, thisMac: computer("this-mac", ""),
            spaces: [
                CuaSDK.SpaceResources(id: "local:design-review", powerState: "running", cpuAllocated: 4,
                                      memoryAllocated: 8 * g, diskAllocated: 80 * g, usage: nil,
                                      diskOnHost: CuaSDK.DiskOnHost(bytes: 19 * g, allocated: 34 * g, sharesSpace: true),
                                      base: CuaSDK.SharedBase(key: "sha256:m", image: "ghcr.io/trycua/macos:26",
                                                              bytes: 25 * g, sharedWith: 2)),
                // One of yours on the Mac mini: only its size there.
                CuaSDK.SpaceResources(id: "relay:space-qa-1234", powerState: "stopped", cpuAllocated: 0,
                                      memoryAllocated: 0, diskAllocated: 0, usage: nil,
                                      diskOnHost: CuaSDK.DiskOnHost(bytes: 21 * g, allocated: 21 * g, sharesSpace: false),
                                      base: nil),
                // Only an upper bound, and a container with no disk at all.
                CuaSDK.SpaceResources(id: "local:clone", powerState: "stopped", cpuAllocated: 2, memoryAllocated: 0,
                                      diskAllocated: 0, usage: nil,
                                      diskOnHost: CuaSDK.DiskOnHost(bytes: nil, allocated: 30 * g, sharesSpace: true),
                                      base: nil),
                // A gVisor container: its guest's root has no size of its own.
                CuaSDK.SpaceResources(id: "local:box", powerState: "running", cpuAllocated: 2, memoryAllocated: 0,
                                      diskAllocated: 0,
                                      usage: CuaSDK.GuestUsage(cpuCount: 2, cpuPercent: 13, loadAverage1m: 0,
                                                               memoryUsed: 1 * g, memoryTotal: 4 * g, memoryLimited: true,
                                                               diskUsed: 249_856, diskTotal: nil, diskLimited: false),
                                      diskOnHost: nil, base: nil),
            ],
            computers: [computer("mac-mini", "Mac mini")], warning: warning)
        let json = resourcesJSON(snapshot)
        // What the page reads: plain JSON numbers, null for what is missing.
        let data = try JSONSerialization.data(withJSONObject: json)
        let back = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(back["sequence"] as? Int == 9)
        let thisMac = try #require(back["thisMac"] as? [String: Any])
        #expect(thisMac["id"] as? String == "this-mac")
        #expect(thisMac["spacesDiskBytes"] as? Int == 19_000_000_000)
        #expect((thisMac["warning"] as? [String: Any])?["level"] as? String == "low")
        #expect((back["warning"] as? [String: Any])?["threshold"] as? Int == 100_000_000_000)
        let spaces = try #require(back["spaces"] as? [[String: Any]])
        #expect(spaces.map { $0["id"] as? String } == ["local:design-review", "relay:space-qa-1234", "local:clone", "local:box"])
        #expect((spaces[1]["diskOnHost"] as? [String: Any])?["bytes"] as? Int == 21_000_000_000)
        #expect(spaces[1]["usage"] is NSNull)
        #expect((spaces[2]["diskOnHost"] as? [String: Any])?["bytes"] is NSNull)
        #expect((spaces[2]["diskOnHost"] as? [String: Any])?["sharesSpace"] as? Bool == true)
        #expect(spaces[3]["diskOnHost"] is NSNull)
        // No size of its own is null, never a sentinel number.
        #expect((spaces[3]["usage"] as? [String: Any])?["diskTotal"] is NSNull)
        let base = try #require(spaces[0]["base"] as? [String: Any])
        #expect(base["image"] as? String == "ghcr.io/trycua/macos:26")
        #expect(base["sharedWith"] as? Int == 2)
        #expect(spaces[1]["base"] is NSNull)
        #expect((back["computers"] as? [[String: Any]])?.map { $0["id"] as? String } == ["mac-mini"])
        // Every field the page's contract names, no more.
        #expect(Set(spaces[0].keys) == ["id", "powerState", "cpuAllocated", "memoryAllocated", "diskAllocated", "usage", "diskOnHost", "base"])
        #expect(Set(thisMac.keys) == Set((HostResourceSampler().sample().snapshot()["thisMac"] as? [String: Any] ?? [:]).keys))
    }
}
