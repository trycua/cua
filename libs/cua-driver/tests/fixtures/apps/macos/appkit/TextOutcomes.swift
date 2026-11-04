import AppKit

final class TextOutcomeView: NSTextView {
    var stale = false
    var unsupported = false
    var lagging = false
    var writes = 0
    var keys = 0
    var changed: (() -> Void)?
    private var publishedAt = Date.distantPast

    override func accessibilityValue() -> String? {
        // A lagging field publishes a write's effect only after a delay, as
        // an app rebuilding its editor around the insertion does.
        stale || (lagging && Date() < publishedAt) ? "" : string
    }

    override func setAccessibilitySelectedText(_ text: String?) {
        guard let text, !unsupported else { return }
        string += text
        writes += 1
        publishedAt = Date().addingTimeInterval(0.4)
        changed?()
    }

    // An unsupported attribute has to leave the attribute list and the legacy
    // setter. AppKit accepts and drops a write that `isAccessibilitySelectorAllowed`
    // alone refuses: that is the `unsettable` field's shape, not this one.
    override func accessibilityAttributeNames() -> [NSAccessibility.Attribute] {
        let names = super.accessibilityAttributeNames()
        return unsupported ? names.filter { $0 != .selectedText } : names
    }

    override func accessibilitySetValue(_ value: Any?, forAttribute attribute: NSAccessibility.Attribute) {
        if unsupported && attribute == .selectedText { return }
        super.accessibilitySetValue(value, forAttribute: attribute)
    }

    override func keyDown(with event: NSEvent) {
        keys += 1
        super.keyDown(with: event)
        changed?()
    }
}

final class TextOutcomesFixture {
    let window: NSWindow
    private let directory: URL
    private let fields: [String: TextOutcomeView]
    private let unrelated = TextOutcomeView(frame: NSRect(x: 20, y: 530, width: 380, height: 60))
    /// A stock field that is not being edited reports `AXSelectedText` as not
    /// settable, and AppKit accepts a write to it and drops it.
    private let unsettable = NSTextField(frame: NSRect(x: 20, y: 420, width: 380, height: 24))

    init(directory: URL) {
        self.directory = directory
        window = NSWindow(
            contentRect: NSRect(x: 100, y: 100, width: 420, height: 620),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "CuaTestHarness Text Outcomes"
        window.isReleasedWhenClosed = false
        fields = Dictionary(uniqueKeysWithValues: ["native", "stale", "unsupported", "lagging"].enumerated().map { index, name in
            let view = TextOutcomeView(frame: NSRect(x: 20, y: 20 + index * 100, width: 380, height: 60))
            view.stale = name == "stale"
            view.unsupported = name == "unsupported"
            view.lagging = name == "lagging"
            view.setAccessibilityIdentifier(name)
            return (name, view)
        })
        for view in fields.values {
            window.contentView!.addSubview(view)
            view.changed = { [weak self] in self?.publish() }
        }
        unsettable.setAccessibilityIdentifier("unsettable")
        window.contentView!.addSubview(unsettable)
        NotificationCenter.default.addObserver(
            forName: NSControl.textDidChangeNotification, object: unsettable, queue: .main
        ) { [weak self] _ in self?.publish() }
        unrelated.string = "marker"
        unrelated.changed = { [weak self] in self?.publish() }
        unrelated.setAccessibilityIdentifier("unrelated")
        window.contentView!.addSubview(unrelated)
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(unrelated)
        publish()
    }

    private func publish() {
        let state: [String: Any] = [
            "pid": ProcessInfo.processInfo.processIdentifier,
            "window_id": window.windowNumber,
            "unrelated": unrelated.string,
            "unsettable": unsettable.currentEditor()?.string ?? unsettable.stringValue,
            "fields": fields.mapValues { view in
                ["value": view.string, "writes": view.writes, "keys": view.keys] as [String: Any]
            },
        ]
        do {
            let data = try JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
            try data.write(to: directory.appendingPathComponent("state.json"), options: .atomic)
        } catch {
            fatalError("text outcome fixture state write failed: \(error)")
        }
    }
}
