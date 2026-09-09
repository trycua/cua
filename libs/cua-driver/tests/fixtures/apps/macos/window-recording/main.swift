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
let frame = NSRect(x: 140, y: 180, width: 321, height: 241)
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
sibling.setFrameOrigin(NSPoint(x: frame.maxX + 20, y: frame.minY))
sibling.orderFrontRegardless()
target.orderFrontRegardless()
app.activate(ignoringOtherApps: true)

func replyState(_ command: String) {
    let cursor = NSEvent.mouseLocation
    reply(["command": command, "miniaturized": target.isMiniaturized,
           "visible": target.isVisible, "x": target.frame.minX,
           "width": target.frame.width,
           "sibling_visible": sibling.isVisible,
           "sibling_adjacent": !sibling.frame.intersects(target.frame),
           "sibling_covers_target": sibling.frame.contains(target.frame),
           "sibling_in_front": NSApp.orderedWindows.first === sibling,
           "ui_state": [
               "frontmost_pid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1,
               "key_window": NSApp.keyWindow?.windowNumber ?? -1,
               "ordered_windows": NSApp.orderedWindows.map { $0.windowNumber },
               "cursor_x": cursor.x, "cursor_y": cursor.y,
           ]])
}

func replyAfterMinimize(until deadline: TimeInterval) {
    if target.isMiniaturized || ProcessInfo.processInfo.systemUptime >= deadline {
        replyState("minimize")
        return
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
        replyAfterMinimize(until: deadline)
    }
}

func replyWhenReady(until deadline: TimeInterval, stableSamples: Int = 0) {
    let windows = CGWindowListCopyWindowInfo(.optionIncludingWindow,
                                            CGWindowID(target.windowNumber)) as? [[String: Any]]
    let bounds = windows?.first?[kCGWindowBounds as String] as? [String: Any]
    let settled = (bounds?["Width"] as? Double) == frame.width
        && (bounds?["Height"] as? Double) == frame.height
        && (windows?.first?[kCGWindowIsOnscreen as String] as? Bool) == true
    let samples = settled ? stableSamples + 1 : 0
    if samples >= 20 {
        reply(["pid": ProcessInfo.processInfo.processIdentifier,
               "window_id": target.windowNumber, "sibling_id": sibling.windowNumber,
               "width": frame.width, "height": frame.height,
               "scale": target.backingScaleFactor])
    } else if ProcessInfo.processInfo.systemUptime >= deadline {
        reply(["error": "fixture_geometry_unsettled"])
    } else {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            replyWhenReady(until: deadline, stableSamples: samples)
        }
    }
}
// WindowServer can briefly expose an expanded launch frame after AppKit returns.
DispatchQueue.main.async {
    replyWhenReady(until: ProcessInfo.processInfo.systemUptime + 5)
}
DispatchQueue.global().async {
    while let command = readLine() {
        DispatchQueue.main.async {
            switch command {
            case "state": break
            case "occlude":
                sibling.setFrame(target.frame.insetBy(dx: -20, dy: -20), display: true)
                sibling.orderFrontRegardless()
            case "move":
                target.setFrameOrigin(NSPoint(x: target.frame.minX + 50, y: target.frame.minY + 30))
                sibling.setFrame(target.frame.insetBy(dx: -20, dy: -20), display: true)
                sibling.orderFrontRegardless()
            case "resize":
                target.setContentSize(NSSize(width: 360, height: 260))
            case "minimize":
                target.miniaturize(nil)
                // AppKit completes minimization asynchronously, after this handler.
                replyAfterMinimize(until: ProcessInfo.processInfo.systemUptime + 3)
                return
            case "close": target.close()
            default:
                reply(["error": "unknown command"])
                return
            }
            replyState(command)
        }
    }
    DispatchQueue.main.async { app.terminate(nil) }
}
app.run()
