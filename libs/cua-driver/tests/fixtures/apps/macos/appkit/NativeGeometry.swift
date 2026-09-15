import AppKit
import WebKit

final class NativeGeometryWindow: NSWindow {
    var reportsMismatch = false
    var reportsUnavailable = false
    var geometryDelay = 0.0
    var inputEvents: [[String: Any]] = []
    var onInput: (() -> Void)?

    var reportedFrame: NSRect {
        if reportsUnavailable { return .zero }
        var frame = super.accessibilityFrame()
        if reportsMismatch {
            frame.size.width += 200
            frame.size.height += 120
        }
        return frame
    }

    override func accessibilityFrame() -> NSRect {
        if geometryDelay > 0 { Thread.sleep(forTimeInterval: geometryDelay) }
        return reportedFrame
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

final class NativeGeometryWebObserver: NSObject, WKScriptMessageHandler {
    var update: ((Double) -> Void)?

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let position = message.body as? NSNumber else { return }
        update?(position.doubleValue)
    }
}

final class NativeGeometryRow: NSView {
    var allowsSelection = false
    var selected = false
    var onSelection: (() -> Void)?

    override func accessibilityPerformShowMenu() -> Bool { false }
    override func isAccessibilitySelected() -> Bool { selected }
    override func setAccessibilitySelected(_ selected: Bool) {
        if allowsSelection {
            self.selected = selected
            onSelection?()
        }
    }
}

final class NativeGeometryFixture: NSObject {
    let window: NativeGeometryWindow
    private let directory: URL
    private let counterLabel = NSTextField(labelWithString: "geometry_count=0")
    private var counter = 0
    private let selectable = NativeGeometryRow(frame: NSRect(x: 190, y: 222, width: 150, height: 18))
    private var webScroll = -1.0
    private let webObserver = NativeGeometryWebObserver()
    private let web: WKWebView?
    private var timer: Timer?
    private var lastCommand: String?

    init(directory: URL) {
        self.directory = directory
        let includeWeb = ProcessInfo.processInfo.environment["CUA_APPKIT_GEOMETRY_NO_WEB"] != "1"
        web = includeWeb ? WKWebView(frame: NSRect(x: 200, y: 20, width: 140, height: 200)) : nil
        webScroll = includeWeb ? -1 : 0
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
        let increment = NSButton(title: "Increment", target: self, action: #selector(increment))
        increment.frame = NSRect(x: 20, y: 170, width: 160, height: 36)
        increment.setAccessibilityIdentifier("geometry-increment")
        content.addSubview(increment)
        let row = NativeGeometryRow(frame: NSRect(x: 20, y: 214, width: 160, height: 22))
        row.setAccessibilityElement(true)
        row.setAccessibilityRole(.row)
        row.setAccessibilityLabel("Selection probe")
        row.setAccessibilityIdentifier("geometry-selection")
        content.addSubview(row)
        selectable.allowsSelection = true
        selectable.setAccessibilityElement(true)
        selectable.setAccessibilityRole(.row)
        selectable.setAccessibilityLabel("Semantic selection probe")
        selectable.setAccessibilityIdentifier("geometry-selectable")
        selectable.onSelection = { [weak self] in self?.publish() }
        content.addSubview(selectable)
        let mismatch = NSButton(title: "Disagree", target: self, action: #selector(disagree))
        mismatch.frame = NSRect(x: 20, y: 100, width: 160, height: 36)
        mismatch.setAccessibilityIdentifier("geometry-mismatch")
        content.addSubview(mismatch)
        let reset = NSButton(title: "Reset web", target: self, action: #selector(resetWeb))
        reset.frame = NSRect(x: 20, y: 60, width: 160, height: 30)
        reset.setAccessibilityIdentifier("geometry-reset-web")
        content.addSubview(reset)
        webObserver.update = { [weak self] position in
            self?.webScroll = position
            self?.publish()
        }
        if let web {
        let scripts = web.configuration.userContentController
        scripts.add(webObserver, name: "geometry")
        scripts.addUserScript(WKUserScript(source: """
            const publish = () => window.webkit.messageHandlers.geometry.postMessage(window.scrollY);
            window.addEventListener('load', publish);
            window.addEventListener('scroll', publish);
            """, injectionTime: .atDocumentStart, forMainFrameOnly: true))
        web.loadHTMLString("""
            <!doctype html><html lang="en"><head><title>Geometry scroll probe</title></head>
            <body style="margin:0"><div style="height:800px">Native reveal probe</div>
            <button aria-label="Geometry reveal target">Reveal target</button>
            <div style="height:200px"></div></body></html>
            """, baseURL: nil)
        content.addSubview(web)
        }
        counterLabel.frame = NSRect(x: 20, y: 30, width: 160, height: 24)
        content.addSubview(counterLabel)
        window.contentView = content
        window.onInput = { [weak self] in self?.publish() }
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        publish()
        timer = Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in
            self?.readCommand()
        }
    }

    private func readCommand() {
        guard let command = try? String(contentsOf: directory.appendingPathComponent("command"), encoding: .utf8),
              command != lastCommand else { return }
        lastCommand = command
        switch command {
        case "align": setGeometry()
        case "mismatch": setGeometry(mismatched: true)
        case "unavailable": setGeometry(unavailable: true)
        case "slow": setGeometry(delay: 0.5)
        default: break
        }
    }

    @objc private func increment() {
        counter += 1
        counterLabel.stringValue = "geometry_count=\(counter)"
        publish()
    }

    @objc private func resetWeb() {
        web?.evaluateJavaScript("window.scrollTo(0, 0)")
    }

    @objc private func disagree() { setGeometry(mismatched: true) }

    private func setGeometry(mismatched: Bool = false, unavailable: Bool = false, delay: Double = 0) {
        window.reportsMismatch = mismatched
        window.reportsUnavailable = unavailable
        window.geometryDelay = delay
        NSAccessibility.post(element: window, notification: .moved)
        NSAccessibility.post(element: window, notification: .resized)
        publish()
    }

    private func publish() {
        do {
            let data = try JSONSerialization.data(withJSONObject: [
                "pid": ProcessInfo.processInfo.processIdentifier,
                "owned_windows": NSApplication.shared.windows.map { owned in
                    ["window_id": owned.windowNumber, "title": owned.title,
                     "class": NSStringFromClass(type(of: owned)), "visible": owned.isVisible] as [String: Any]
                },
                "window_id": window.windowNumber,
                "physical_width": window.frame.width,
                "reported_width": window.reportedFrame.width,
                "mismatched": window.reportsMismatch,
                "unavailable": window.reportsUnavailable,
                "geometry_delay_ms": window.geometryDelay * 1000,
                "counter": counter,
                "selected": selectable.selected,
                "web_scroll_y": webScroll,
                "input_events": window.inputEvents
            ], options: [.sortedKeys])
            try data.write(to: directory.appendingPathComponent("state.json"), options: .atomic)
        } catch {
            fatalError("native geometry fixture state write failed: \(error)")
        }
    }
}
