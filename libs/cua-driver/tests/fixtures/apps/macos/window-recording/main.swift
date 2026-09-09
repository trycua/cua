import AppKit

// The stdin protocol changes only this process's synthetic windows.
final class MarkerView: NSView {
    var pulse = false

    override func draw(_ dirtyRect: NSRect) {
        NSColor(srgbRed: 0, green: 1, blue: 0, alpha: 1).setFill()
        bounds.fill()
        NSColor(srgbRed: 0, green: 0, blue: 1, alpha: 1).setFill()
        NSRect(x: bounds.width * 0.375, y: bounds.height * 0.375,
               width: bounds.width * 0.25, height: bounds.height * 0.25).fill()
        NSColor.white.setFill()
        NSRect(x: 0, y: bounds.height * 0.05,
               width: bounds.width * (pulse ? 0.65 : 0.15),
               height: bounds.height * 0.06).fill()
    }
}

func reply(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data + Data([10]))
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let frame = NSRect(x: 140, y: 180, width: 320, height: 240)
let target = NSWindow(contentRect: frame,
                      styleMask: [.borderless, .miniaturizable], backing: .buffered, defer: false)
target.title = "Cua Recording Target"
target.isReleasedWhenClosed = false
target.hasShadow = false
let marker = MarkerView(frame: NSRect(origin: .zero, size: frame.size))
target.contentView = marker
let animationTimer = Timer.scheduledTimer(withTimeInterval: 0.3, repeats: true) { _ in
    marker.pulse.toggle()
    marker.needsDisplay = true
}
let sibling = NSWindow(contentRect: frame.insetBy(dx: -20, dy: -20),
                       styleMask: .borderless, backing: .buffered, defer: false)
sibling.title = "Cua Recording Red Sibling"
sibling.isReleasedWhenClosed = false
sibling.hasShadow = false
sibling.backgroundColor = NSColor(srgbRed: 1, green: 0, blue: 0, alpha: 1)
target.orderFrontRegardless()
app.activate(ignoringOtherApps: true)

DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
    reply(["pid": ProcessInfo.processInfo.processIdentifier,
           "window_id": target.windowNumber, "sibling_id": sibling.windowNumber,
           "width": frame.width, "height": frame.height,
           "scale": target.backingScaleFactor])
}
DispatchQueue.global().async {
    while let command = readLine() {
        DispatchQueue.main.async {
            switch command {
            case "occlude":
                sibling.setFrame(target.frame.insetBy(dx: -20, dy: -20), display: true)
                sibling.orderFrontRegardless()
            case "move":
                target.setFrameOrigin(NSPoint(x: target.frame.minX + 50, y: target.frame.minY + 30))
                sibling.setFrame(target.frame.insetBy(dx: -20, dy: -20), display: true)
                sibling.orderFrontRegardless()
            case "resize":
                target.setContentSize(NSSize(width: 360, height: 260))
            case "minimize": target.miniaturize(nil)
            case "close": target.close()
            default:
                reply(["error": "unknown command"])
                return
            }
            reply(["command": command, "miniaturized": target.isMiniaturized,
                   "visible": target.isVisible, "x": target.frame.minX,
                   "width": target.frame.width,
                   "sibling_visible": sibling.isVisible,
                   "sibling_covers_target": sibling.frame.contains(target.frame),
                   "sibling_in_front": NSApp.orderedWindows.first === sibling])
        }
    }
    DispatchQueue.main.async { app.terminate(nil) }
}
app.run()
