import AppKit

final class NativeGeometryWindow: NSWindow {
    var reportsMismatch = false
    var inputEvents: [[String: Any]] = []
    var onInput: (() -> Void)?

    override func accessibilityFrame() -> NSRect {
        var frame = super.accessibilityFrame()
        if reportsMismatch {
            frame.size.width += 200
            frame.size.height += 120
        }
        return frame
    }

    override func sendEvent(_ event: NSEvent) {
        let inputTypes: Set<NSEvent.EventType> = [
            .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp,
            .otherMouseDown, .otherMouseUp, .leftMouseDragged, .rightMouseDragged,
            .otherMouseDragged, .keyDown, .keyUp, .flagsChanged, .scrollWheel
        ]
        if inputTypes.contains(event.type) {
            inputEvents.append([
                "type": event.type.rawValue,
                "window_id": event.windowNumber,
                "timestamp": event.timestamp
            ])
            onInput?()
        }
        super.sendEvent(event)
    }
}

final class NativeGeometryButton: NSButton {
    var onReveal: (() -> Void)?

    override func accessibilityActionNames() -> [NSAccessibility.Action] {
        super.accessibilityActionNames() + [NSAccessibility.Action(rawValue: "AXScrollToVisible")]
    }

    override func accessibilityPerformAction(_ action: NSAccessibility.Action) {
        if action.rawValue == "AXScrollToVisible" {
            onReveal?()
        } else {
            super.accessibilityPerformAction(action)
        }
    }
}

final class NativeGeometryFixture: NSObject {
    let window: NativeGeometryWindow
    private let directory: URL
    private let counterLabel = NSTextField(labelWithString: "geometry_count=0")
    private var counter = 0
    private var reveals = 0

    init(directory: URL) {
        self.directory = directory
        window = NativeGeometryWindow(
            contentRect: NSRect(x: 100, y: 100, width: 360, height: 240),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered, defer: false)
        super.init()
        window.title = "CuaTestHarness Native Geometry"
        window.isReleasedWhenClosed = false
        window.isRestorable = false
        window.setFrameAutosaveName("")
        window.setAccessibilityIdentifier("geometry-window")
        let content = NSView(frame: NSRect(x: 0, y: 0, width: 360, height: 240))
        let increment = NativeGeometryButton(title: "Increment", target: self, action: #selector(increment))
        increment.onReveal = { [weak self] in
            guard let self else { return }
            self.reveals += 1
            self.publish()
        }
        increment.frame = NSRect(x: 20, y: 170, width: 160, height: 36)
        increment.setAccessibilityIdentifier("geometry-increment")
        content.addSubview(increment)
        let mismatch = NSButton(title: "Disagree", target: self, action: #selector(disagree))
        mismatch.frame = NSRect(x: 20, y: 100, width: 160, height: 36)
        mismatch.setAccessibilityIdentifier("geometry-mismatch")
        content.addSubview(mismatch)
        counterLabel.frame = NSRect(x: 20, y: 30, width: 240, height: 24)
        content.addSubview(counterLabel)
        window.contentView = content
        window.onInput = { [weak self] in self?.publish() }
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        publish()
    }

    @objc private func increment() {
        counter += 1
        counterLabel.stringValue = "geometry_count=\(counter)"
        publish()
    }

    @objc private func disagree() {
        window.reportsMismatch = true
        NSAccessibility.post(element: window, notification: .moved)
        NSAccessibility.post(element: window, notification: .resized)
        publish()
    }

    private func publish() {
        do {
            let data = try JSONSerialization.data(withJSONObject: [
                "pid": ProcessInfo.processInfo.processIdentifier,
                "window_id": window.windowNumber,
                "physical_width": window.frame.width,
                "reported_width": window.accessibilityFrame().width,
                "mismatched": window.reportsMismatch,
                "counter": counter,
                "reveals": reveals,
                "input_events": window.inputEvents
            ], options: [.sortedKeys])
            try data.write(to: directory.appendingPathComponent("state.json"), options: .atomic)
        } catch {
            fatalError("native geometry fixture state write failed: \(error)")
        }
    }
}
