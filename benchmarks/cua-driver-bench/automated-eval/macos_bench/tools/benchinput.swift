// benchinput: a tiny CGEvent injector used only to validate the MB-* fixtures (oracle solutions).
// Real HID-level events at global screen points (origin top-left, points).
//
//   benchinput move X Y
//   benchinput click X Y [left|right] [count]
//   benchinput drag X1 Y1 X2 Y2 [steps] [duration_ms]
//   benchinput scroll X Y DY            (pixel units, negative scrolls down)
//   benchinput key NAME [cmd] [shift] [opt] [ctrl] [repeat N]
//   benchinput type "text"
//   benchinput hover X Y SECONDS        (move there in small steps, then dwell)
//   benchinput pos
//   benchinput activate PID             (bring that app to the front)
import AppKit
import CoreGraphics
import Foundation

let args = CommandLine.arguments
func f(_ i: Int) -> CGFloat { return CGFloat(Double(args[i])!) }
func post(_ e: CGEvent?) { e?.post(tap: .cghidEventTap) }
func pause(_ ms: Int) { usleep(UInt32(ms * 1000)) }

func mouse(_ type: CGEventType, _ p: CGPoint, _ button: CGMouseButton = .left, clickState: Int64 = 1) {
    let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: button)
    e?.setIntegerValueField(.mouseEventClickState, value: clickState)
    post(e)
}

let keyCodes: [String: CGKeyCode] = [
    "return": 36, "tab": 48, "space": 49, "delete": 51, "escape": 53, "left": 123, "right": 124,
    "down": 125, "up": 126, "a": 0, "c": 8, "v": 9, "b": 11, "x": 7, "comma": 43, "home": 115, "end": 119,
    "pagedown": 121, "pageup": 116,
]

func key(_ code: CGKeyCode, flags: CGEventFlags) {
    for down in [true, false] {
        let e = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)
        e?.flags = flags
        post(e)
        pause(6)
    }
}

guard args.count >= 2 else { exit(2) }
switch args[1] {
case "activate":
    if let app = NSRunningApplication(processIdentifier: pid_t(args[2])!) {
        app.activate(options: [.activateAllWindows])
        pause(500)
    }
case "pos":
    let p = CGEvent(source: nil)!.location
    print("\(p.x) \(p.y)")
case "move":
    mouse(.mouseMoved, CGPoint(x: f(2), y: f(3)))
case "click":
    let right = args.count > 4 && args[4] == "right"
    let count = args.count > 5 ? Int(args[5])! : 1
    let p = CGPoint(x: f(2), y: f(3))
    mouse(.mouseMoved, p)
    pause(60)
    for n in 1...count {
        mouse(right ? .rightMouseDown : .leftMouseDown, p, right ? .right : .left, clickState: Int64(n))
        pause(40)
        mouse(right ? .rightMouseUp : .leftMouseUp, p, right ? .right : .left, clickState: Int64(n))
        pause(60)
    }
case "drag":
    let a = CGPoint(x: f(2), y: f(3)), b = CGPoint(x: f(4), y: f(5))
    let steps = args.count > 6 ? Int(args[6])! : 20
    let ms = args.count > 7 ? Int(args[7])! : 500
    mouse(.mouseMoved, a)
    pause(100)
    mouse(.leftMouseDown, a)
    pause(120)
    for s in 1...steps {
        let t = CGFloat(s) / CGFloat(steps)
        mouse(.leftMouseDragged, CGPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t))
        pause(max(1, ms / steps))
    }
    pause(120)
    mouse(.leftMouseUp, b)
case "scroll":
    mouse(.mouseMoved, CGPoint(x: f(2), y: f(3)))
    pause(60)
    var remaining = Int(Double(args[4])!)
    while remaining != 0 {
        let step = max(min(remaining, 300), -300)
        let e = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: Int32(step), wheel2: 0, wheel3: 0)
        post(e)
        remaining -= step
        pause(20)
    }
case "key":
    var flags: CGEventFlags = []
    var repeatN = 1
    var i = 3
    while i < args.count {
        switch args[i] {
        case "cmd": flags.insert(.maskCommand)
        case "shift": flags.insert(.maskShift)
        case "opt": flags.insert(.maskAlternate)
        case "ctrl": flags.insert(.maskControl)
        case "repeat": i += 1; repeatN = Int(args[i])!
        default: break
        }
        i += 1
    }
    guard let code = keyCodes[args[2]] else { FileHandle.standardError.write(Data("unknown key\n".utf8)); exit(2) }
    for _ in 0..<repeatN { key(code, flags: flags) }
case "type":
    // US-layout virtual key codes, so the events look like a physical keyboard to every text system.
    let base: [Character: CGKeyCode] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12,
        "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
        "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34,
        "p": 35, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46,
        ".": 47, " ": 49,
    ]
    let shifted: [Character: Character] = ["*": "8", "+": "=", "_": "-", "?": "/", ":": ";", "\"": "'", "!": "1"]
    for ch in args[2] {
        var c = ch
        var shift = false
        if let u = shifted[ch] { c = u; shift = true }
        else if ch.isUppercase { c = Character(ch.lowercased()); shift = true }
        guard let code = base[c] else {
            FileHandle.standardError.write(Data("cannot type \(ch)\n".utf8))
            exit(2)
        }
        key(code, flags: shift ? .maskShift : [])
        pause(6)
    }
case "hover":
    let p = CGPoint(x: f(2), y: f(3))
    let start = CGEvent(source: nil)!.location
    for s in 1...12 {
        let t = CGFloat(s) / 12
        mouse(.mouseMoved, CGPoint(x: start.x + (p.x - start.x) * t, y: start.y + (p.y - start.y) * t))
        pause(15)
    }
    // Small jiggles keep the pointer "moving" inside the target, then it rests.
    mouse(.mouseMoved, CGPoint(x: p.x + 1, y: p.y))
    pause(30)
    mouse(.mouseMoved, p)
    pause(Int(Double(args[4])! * 1000))
default:
    exit(2)
}
