// BenchSpaces: deterministic desktop-Space layout for the off-Space tasks (Amendment 18, OS-*).
//
//   benchspaces list                         JSON: spaces in Mission Control order, the active one, and their types
//   benchspaces window-spaces WID [WID...]   JSON: the Space ids each window is on
//   benchspaces goto N                       make the N-th Space (1-based, Mission Control order) active, the way a
//                                            user does: Control-Left / Control-Right key presses, one Space at a time,
//                                            each confirmed before the next; exit 1 if N is not reached in 20 s
//   benchspaces add-desktop X Y              VM provisioning only: add a desktop through Mission Control's button
//   benchspaces fullscreen PID TITLE         put the app's window whose title contains TITLE into full screen through
//                                            accessibility (AXFullScreen), and wait until it has its own Space
//
// Read-only Space queries use the WindowServer calls that Mission Control tools have used for years
// (CGSCopyManagedDisplaySpaces, CGSCopySpacesForWindows). Nothing moves a window between Spaces: a window lands on a
// Space because it opens while that Space is active, as for a person. The harness only runs this before the agent
// starts and after it ends, never during a trial.
import AppKit
import ApplicationServices
import Foundation

@_silgen_name("CGSMainConnectionID") func CGSMainConnectionID() -> Int32
@_silgen_name("CGSCopyManagedDisplaySpaces") func CGSCopyManagedDisplaySpaces(_ cid: Int32) -> CFArray
@_silgen_name("CGSCopySpacesForWindows") func CGSCopySpacesForWindows(_ cid: Int32, _ mask: Int32, _ wids: CFArray) -> CFArray

struct Space { let id: UInt64; let type: Int; let index: Int }

func spaces() -> (all: [Space], current: UInt64) {
    let cid = CGSMainConnectionID()
    var out: [Space] = []
    var current: UInt64 = 0
    let displays = CGSCopyManagedDisplaySpaces(cid) as? [[String: Any]] ?? []
    for display in displays {
        if let cur = display["Current Space"] as? [String: Any] {
            current = (cur["ManagedSpaceID"] as? NSNumber)?.uint64Value ?? (cur["id64"] as? NSNumber)?.uint64Value ?? 0
        }
        for s in display["Spaces"] as? [[String: Any]] ?? [] {
            let id = (s["ManagedSpaceID"] as? NSNumber)?.uint64Value ?? (s["id64"] as? NSNumber)?.uint64Value ?? 0
            out.append(Space(id: id, type: (s["type"] as? NSNumber)?.intValue ?? -1, index: out.count + 1))
        }
        break  // the bench VM has one display
    }
    return (out, current)
}

func printJSON(_ value: Any) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}

func listJSON() -> [String: Any] {
    let s = spaces()
    let cur = s.all.first { $0.id == s.current }
    return [
        "current_id": s.current,
        "current_index": cur?.index ?? 0,
        // type 0 = desktop, 4 = full-screen app
        "spaces": s.all.map { ["id": $0.id, "type": $0.type, "index": $0.index] },
    ]
}

func press(_ keyCode: CGKeyCode) {
    let src = CGEventSource(stateID: .hidSystemState)
    for down in [true, false] {
        let e = CGEvent(keyboardEventSource: src, virtualKey: keyCode, keyDown: down)!
        e.flags = [.maskControl, .maskSecondaryFn, .maskNumericPad]
        e.post(tap: .cghidEventTap)
        usleep(30_000)
    }
}

func goto(_ target: Int) -> Bool {
    let deadline = Date().addingTimeInterval(20)
    while Date() < deadline {
        let s = spaces()
        guard let cur = s.all.first(where: { $0.id == s.current }) else { usleep(200_000); continue }
        if cur.index == target { return true }
        if target < 1 || target > s.all.count { return false }
        press(cur.index < target ? 124 : 123)  // Right / Left arrow with Control
        let stepDeadline = Date().addingTimeInterval(4)
        while Date() < stepDeadline && spaces().current == s.current { usleep(100_000) }
        usleep(700_000)  // let the switch animation finish before the next step
    }
    return false
}

func fullscreen(pid: pid_t, title: String) -> Bool {
    let app = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else { return false }
    for w in windows {
        var t: CFTypeRef?
        AXUIElementCopyAttributeValue(w, kAXTitleAttribute as CFString, &t)
        if let s = t as? String, title.isEmpty || s.contains(title) {
            let before = spaces().all.count
            guard AXUIElementSetAttributeValue(w, "AXFullScreen" as CFString, kCFBooleanTrue) == .success else { return false }
            let deadline = Date().addingTimeInterval(10)
            while Date() < deadline {
                if spaces().all.count > before && spaces().all.contains(where: { $0.type == 4 }) { usleep(800_000); return true }
                usleep(200_000)
            }
            return false
        }
    }
    return false
}

@main
struct BenchSpaces {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        switch args.first {
        case "list":
            printJSON(listJSON())
        case "window-spaces":
            let ids = args.dropFirst().compactMap { UInt32($0) }.map { NSNumber(value: $0) }
            let got = CGSCopySpacesForWindows(CGSMainConnectionID(), 0x7, ids as CFArray) as? [NSNumber] ?? []
            printJSON(["windows": ids.map { $0.intValue }, "spaces": got.map { $0.uint64Value }])
        case "goto":
            guard args.count == 2, let n = Int(args[1]) else { exit(2) }
            let ok = goto(n)
            var out = listJSON(); out["ok"] = ok
            printJSON(out)
            exit(ok ? 0 : 1)
        case "add-desktop":
            // VM provisioning only (A18.6): open Mission Control and press its "add desktop" button at X Y.
            guard args.count == 3, let x = Double(args[1]), let y = Double(args[2]) else { exit(2) }
            let before = spaces().all.filter { $0.type == 0 }.count
            NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/Mission Control.app"))
            usleep(1_500_000)
            let src = CGEventSource(stateID: .hidSystemState)
            for (kind, wait) in [(CGEventType.mouseMoved, 600_000), (.leftMouseDown, 80_000), (.leftMouseUp, 1_500_000)] {
                CGEvent(mouseEventSource: src, mouseType: kind, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left)!.post(tap: .cghidEventTap)
                usleep(useconds_t(wait))
            }
            press(53)  // Escape closes Mission Control
            usleep(800_000)
            var out = listJSON(); out["ok"] = spaces().all.filter { $0.type == 0 }.count > before
            printJSON(out)
        case "fullscreen":
            guard args.count >= 2, let pid = pid_t(args[1]) else { exit(2) }
            let ok = fullscreen(pid: pid, title: args.count > 2 ? args[2] : "")
            var out = listJSON(); out["ok"] = ok
            printJSON(out)
            exit(ok ? 0 : 1)
        default:
            FileHandle.standardError.write("usage: benchspaces list | window-spaces WID... | goto N | fullscreen PID TITLE\n".data(using: .utf8)!)
            exit(2)
        }
    }
}
