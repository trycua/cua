// winlist: print the on-screen windows as JSON, for the cc-claude-cu-helper adapter (Amendment 11).
// Public CoreGraphics API only (CGWindowListCopyWindowInfo); no Accessibility, no private symbols, no input.
// Window titles need Screen Recording for the responsible process; without it "title" is empty.
//
// Build (in the VM): swiftc -O -o winlist winlist.swift
// Output: [{"app","pid","window_id","title","x","y","width","height","layer"}], front to back, layer 0 only
// (normal app windows), windows smaller than 40x40 points dropped.
import CoreGraphics
import Foundation

let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let infos = (CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]]) ?? []
var out: [[String: Any]] = []
for info in infos {
    let layer = info[kCGWindowLayer as String] as? Int ?? -1
    guard layer == 0 else { continue }
    guard let bounds = info[kCGWindowBounds as String] as? [String: Any],
          let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary) else { continue }
    if rect.width < 40 || rect.height < 40 { continue }
    out.append([
        "app": info[kCGWindowOwnerName as String] as? String ?? "",
        "pid": info[kCGWindowOwnerPID as String] as? Int ?? 0,
        "window_id": info[kCGWindowNumber as String] as? Int ?? 0,
        "title": info[kCGWindowName as String] as? String ?? "",
        "x": Double(rect.origin.x), "y": Double(rect.origin.y),
        "width": Double(rect.size.width), "height": Double(rect.size.height),
        "layer": layer,
    ])
}
let data = try JSONSerialization.data(withJSONObject: out, options: [])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write("\n".data(using: .utf8)!)
