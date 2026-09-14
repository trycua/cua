import AppKit

func emit(_ kind: String, _ values: [String: Any] = [:]) {
    var event = values
    event["kind"] = kind
    let data = try! JSONSerialization.data(withJSONObject: event, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([10]))
}

final class FieldObserver: NSObject, NSTextFieldDelegate {
    func controlTextDidChange(_ notification: Notification) {
        if let field = notification.object as? NSTextField {
            emit("value", ["value": field.stringValue])
        }
    }
}

let observer = FieldObserver()
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 160, y: 160, width: 420, height: 240), styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.title = "Cua Keyboard Oracle"
window.isReleasedWhenClosed = false
let field = NSTextField(frame: NSRect(x: 30, y: 80, width: 350, height: 35))
field.stringValue = "unchanged"
field.delegate = observer
window.contentView!.addSubview(field)
let monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) { event in
    emit(event.type == .keyDown ? "down" : "up", ["key": Int(event.keyCode), "flags": event.modifierFlags.rawValue])
    if event.type == .keyDown && ProcessInfo.processInfo.environment["CUA_KEYBOARD_CLOSE_ON_KEY"] == "1" {
        window.close()
        emit("closed")
    }
    return event
}
app.finishLaunching()
var companion: NSWindow?
if ProcessInfo.processInfo.environment["CUA_KEYBOARD_COMPANION"] == "1" {
    companion = NSWindow(contentRect: NSRect(x: 610, y: 160, width: 360, height: 240), styleMask: [.titled, .closable], backing: .buffered, defer: false)
    companion!.title = "Cua Keyboard Oracle Companion"
    companion!.makeKeyAndOrderFront(nil)
}
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(field)
app.activate(ignoringOtherApps: true)
DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) {
    field.selectText(nil)
    emit("ready", ["window": window.windowNumber])
}
app.run()
withExtendedLifetime(monitor) {}
