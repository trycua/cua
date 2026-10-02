// Read-only, permission-free WindowServer/NSWorkspace observer for the focused
// product-window regression. No AX calls, activation, or input synthesis.
import AppKit
import CoreGraphics
import Foundation

let pids = Set(CommandLine.arguments.dropFirst().compactMap(Int.init))
let workspace = NSWorkspace.shared
RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []
let result: [String: Any] = [
    "frontmost_pid": workspace.frontmostApplication?.processIdentifier ?? -1,
    "windows": windows.filter { pids.contains(($0[kCGWindowOwnerPID as String] as? Int) ?? 0) },
]
FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]))
