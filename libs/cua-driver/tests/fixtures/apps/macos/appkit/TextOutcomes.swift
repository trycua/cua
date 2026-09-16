import AppKit

final class TextOutcomeView: NSTextView {
    var stale = false
    var unsupported = false
    var writes = 0
    var keys = 0
    var changed: (() -> Void)?

    override func accessibilityValue() -> String? {
        stale ? "" : string
    }

    override func setAccessibilitySelectedText(_ text: String?) {
        guard let text else { return }
        string += text
        writes += 1
        changed?()
    }

    override func isAccessibilitySelectorAllowed(_ selector: Selector) -> Bool {
        if unsupported && selector == #selector(setAccessibilitySelectedText(_:)) {
            return false
        }
        return super.isAccessibilitySelectorAllowed(selector)
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
    private let unrelated = TextOutcomeView(frame: NSRect(x: 20, y: 330, width: 380, height: 60))

    init(directory: URL) {
        self.directory = directory
        window = NSWindow(
            contentRect: NSRect(x: 100, y: 100, width: 420, height: 420),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "CuaTestHarness Text Outcomes"
        window.isReleasedWhenClosed = false
        fields = Dictionary(uniqueKeysWithValues: ["native", "stale", "unsupported"].enumerated().map { index, name in
            let view = TextOutcomeView(frame: NSRect(x: 20, y: 20 + index * 100, width: 380, height: 60))
            view.stale = name == "stale"
            view.unsupported = name == "unsupported"
            view.setAccessibilityIdentifier(name)
            return (name, view)
        })
        for view in fields.values {
            window.contentView!.addSubview(view)
            view.changed = { [weak self] in self?.publish() }
        }
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
