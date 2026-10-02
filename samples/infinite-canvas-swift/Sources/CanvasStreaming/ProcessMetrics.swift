// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import Darwin
import Foundation
import IOKit

/// CPU, memory and GPU readings for the benchmark and the perf HUD, from the
/// same kernel counters Activity Monitor and Instruments read.
public enum ProcessMetrics {
    /// This process's physical footprint in bytes (what Activity Monitor
    /// calls Memory).
    public static func footprint() -> UInt64 {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return kr == KERN_SUCCESS ? UInt64(info.phys_footprint) : 0
    }

    /// Total user+system CPU seconds this process has used.
    public static func cpuSeconds() -> Double {
        var usage = rusage()
        getrusage(RUSAGE_SELF, &usage)
        func s(_ t: timeval) -> Double { Double(t.tv_sec) + Double(t.tv_usec) / 1e6 }
        return s(usage.ru_utime) + s(usage.ru_stime)
    }

    /// Whole-GPU utilization (percent) from the accelerator's
    /// `PerformanceStatistics`. System-wide, not per process: read it with
    /// the machine otherwise idle.
    public static func gpuUtilization() -> Double? {
        var iterator: io_iterator_t = 0
        guard IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOAccelerator"), &iterator)
            == KERN_SUCCESS else { return nil }
        defer { IOObjectRelease(iterator) }
        var best: Double?
        var entry = IOIteratorNext(iterator)
        while entry != 0 {
            var props: Unmanaged<CFMutableDictionary>?
            if IORegistryEntryCreateCFProperties(entry, &props, kCFAllocatorDefault, 0) == KERN_SUCCESS,
               let dict = props?.takeRetainedValue() as? [String: Any],
               let stats = dict["PerformanceStatistics"] as? [String: Any],
               let v = (stats["Device Utilization %"] as? NSNumber)?.doubleValue {
                best = max(best ?? 0, v)
            }
            IOObjectRelease(entry)
            entry = IOIteratorNext(iterator)
        }
        return best
    }

    /// CPU time spent on the main thread, in seconds. Sampled with
    /// `thread_info` on the main thread's port, so it is exact rather than
    /// statistical.
    public static func mainThreadCPUSeconds() -> Double {
        let port = mainThreadPort
        var info = thread_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<thread_basic_info_data_t>.size / MemoryLayout<integer_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                thread_info(port, thread_flavor_t(THREAD_BASIC_INFO), $0, &count)
            }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        func s(_ t: time_value_t) -> Double { Double(t.seconds) + Double(t.microseconds) / 1e6 }
        return s(info.user_time) + s(info.system_time)
    }

    /// Captured once, on the main thread, at startup.
    nonisolated(unsafe) public static var mainThreadPort: thread_act_t = mach_thread_self()

    /// Call from the main thread before sampling main-thread CPU.
    public static func captureMainThread() {
        precondition(Thread.isMainThread)
        mainThreadPort = mach_thread_self()
    }
}

/// CPU percentage over an interval (100 = one core).
public struct CPUMeter {
    private var lastWall = Date()
    private var lastCPU = ProcessMetrics.cpuSeconds()
    private var lastMain = ProcessMetrics.mainThreadCPUSeconds()

    public init() {}

    /// Returns (process %, main thread %) since the last call.
    public mutating func sample() -> (process: Double, main: Double) {
        let now = Date()
        let cpu = ProcessMetrics.cpuSeconds()
        let main = ProcessMetrics.mainThreadCPUSeconds()
        let wall = now.timeIntervalSince(lastWall)
        defer { lastWall = now; lastCPU = cpu; lastMain = main }
        guard wall > 0 else { return (0, 0) }
        return ((cpu - lastCPU) / wall * 100, (main - lastMain) / wall * 100)
    }
}
