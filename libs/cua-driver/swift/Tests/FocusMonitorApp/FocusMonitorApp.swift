/// FocusMonitorApp — counts how many times it loses focus.
///
/// Tracks three kinds of focus loss:
///
/// 1. **App-level** (NSApplication.didResignActiveNotification): the whole
///    app loses the system focus token. Written to /tmp/focus_monitor_losses.txt.
///
/// 2. **Window/keyboard-level** (NSWindow.didResignKeyNotification): the window
///    loses key status (another window, including a background Safari window,
///    becomes key). Written to /tmp/focus_monitor_key_losses.txt.
///
/// 3. **Text-field first-responder** (TrackingTextField.resignFirstResponder):
///    the editable NSTextField loses first-responder status, meaning another
///    app or window captured keyboard input. Written to
///    /tmp/focus_monitor_field_losses.txt.
///
/// A visible editable NSTextField is created in the window and made first
/// responder immediately, so keyboard input goes to it by default. Agents
/// operating in background-mode should NOT steal this keyboard focus even
/// when typing into other windows.
///
/// At startup, prints FOCUS_PID=<pid> to stdout so the test harness can
/// discover the process.

import AppKit

// ---------------------------------------------------------------------------
// Tracking NSTextField — calls back when it loses first-responder status.
// ---------------------------------------------------------------------------

class TrackingTextField: NSTextField {
    var onResignFirstResponder: (() -> Void)?

    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        if result {
            onResignFirstResponder?()
        }
        return result
    }
}

// ---------------------------------------------------------------------------
// App delegate
// ---------------------------------------------------------------------------

class AppDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var statusLabel: NSTextField!
    var inputField: TrackingTextField!

    var lossCount = 0          // app-level focus losses
    var keyLossCount = 0       // window key-status losses
    var fieldLossCount = 0     // text-field first-responder losses

    let lossFile    = "/tmp/focus_monitor_losses.txt"
    let keyFile     = "/tmp/focus_monitor_key_losses.txt"
    let fieldFile   = "/tmp/focus_monitor_field_losses.txt"

    func applicationDidFinishLaunching(_ notification: Notification) {
        let rect = NSRect(x: 200, y: 200, width: 460, height: 280)
        window = NSWindow(
            contentRect: rect,
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Focus Monitor"
        window.isReleasedWhenClosed = false

        // ── Status label ─────────────────────────────────────────────────
        statusLabel = NSTextField(labelWithString: "app_losses: 0 | key_losses: 0 | field_losses: 0")
        statusLabel.font = NSFont.monospacedSystemFont(ofSize: 16, weight: .bold)
        statusLabel.frame = NSRect(x: 20, y: 180, width: 420, height: 60)
        statusLabel.alignment = .center
        statusLabel.lineBreakMode = .byWordWrapping
        window.contentView?.addSubview(statusLabel)

        // ── Instruction label ─────────────────────────────────────────────
        let hint = NSTextField(labelWithString: "↓ This input holds keyboard focus ↓")
        hint.font = NSFont.systemFont(ofSize: 12)
        hint.textColor = .secondaryLabelColor
        hint.frame = NSRect(x: 20, y: 148, width: 420, height: 22)
        hint.alignment = .center
        window.contentView?.addSubview(hint)

        // ── Editable text field that acts as the keyboard-focus sentinel ──
        inputField = TrackingTextField()
        inputField.frame = NSRect(x: 60, y: 100, width: 340, height: 36)
        inputField.font = NSFont.systemFont(ofSize: 16)
        inputField.placeholderString = "keyboard focus stays here…"
        inputField.bezelStyle = .roundedBezel
        inputField.onResignFirstResponder = { [weak self] in
            self?.fieldDidResignFirstResponder()
        }
        window.contentView?.addSubview(inputField)

        // ── Secondary label showing the field-focus state ─────────────────
        let fieldHint = NSTextField(labelWithString: "")
        fieldHint.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        fieldHint.textColor = .systemOrange
        fieldHint.frame = NSRect(x: 20, y: 60, width: 420, height: 30)
        fieldHint.alignment = .center
        fieldHint.tag = 99
        window.contentView?.addSubview(fieldHint)

        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        // Give the window a run-loop cycle to become key before we
        // make the text field first responder.
        DispatchQueue.main.async {
            self.window.makeFirstResponder(self.inputField)
        }

        // ── Notifications ──────────────────────────────────────────────────
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(appDidResign),
            name: NSApplication.didResignActiveNotification,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(windowDidResignKey),
            name: NSWindow.didResignKeyNotification,
            object: window
        )

        // Write initial zeros so tests can open the files even before
        // any loss event fires.
        writeLosses()
        writeKeyLosses()
        writeFieldLosses()

        let pid = ProcessInfo.processInfo.processIdentifier
        print("FOCUS_PID=\(pid)")
        fflush(stdout)
    }

    // ── Notification handlers ────────────────────────────────────────────

    @objc func appDidResign(_ note: Notification) {
        lossCount += 1
        updateStatusLabel()
        writeLosses()
    }

    @objc func windowDidResignKey(_ note: Notification) {
        keyLossCount += 1
        updateStatusLabel()
        writeKeyLosses()
    }

    func fieldDidResignFirstResponder() {
        fieldLossCount += 1
        updateStatusLabel()
        writeFieldLosses()
        // Show which field stole focus via the orange hint label.
        if let lbl = window.contentView?.viewWithTag(99) as? NSTextField {
            lbl.stringValue = "⚠️ keyboard focus left the input field (\(fieldLossCount)x)"
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    func updateStatusLabel() {
        statusLabel.stringValue =
            "app_losses: \(lossCount) | key_losses: \(keyLossCount) | field_losses: \(fieldLossCount)"
    }

    func writeLosses() {
        try? "\(lossCount)".write(toFile: lossFile, atomically: true, encoding: .utf8)
    }

    func writeKeyLosses() {
        try? "\(keyLossCount)".write(toFile: keyFile, atomically: true, encoding: .utf8)
    }

    func writeFieldLosses() {
        try? "\(fieldLossCount)".write(toFile: fieldFile, atomically: true, encoding: .utf8)
    }
}

// ── Minimal NSApplication bootstrap — no XIB, no storyboard. ──────────────
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
