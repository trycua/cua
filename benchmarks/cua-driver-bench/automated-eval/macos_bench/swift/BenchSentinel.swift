// BenchSentinel: a tiny always-there window that stays frontmost and key while an
// agent works in other apps. It logs focus changes, leaked keys/clicks/scrolls and
// pointer position as JSONL so a run can be scored for foreground disturbance.
//
// Usage: BenchSentinel --log PATH [--no-activate] [--user-activity] [--frame X,Y,W,H]
// --user-activity (Amendment 15): the window plays the user's own app. It is titled "User activity" and, while
// armed, "types" one character a second into its own field when it is active and key. A tick when it is not is
// logged as user_blocked; text that differs from what the user typed is logged as user_text_disrupted.
// --frame: outer frame in top-left screen coordinates (default 360x140 at the bottom right).
// Signals: SIGUSR1 re-activate, SIGUSR2 toggle armed.

import AppKit
import CoreGraphics
import Foundation

func nowMs() -> Double { Date().timeIntervalSince1970 * 1000.0 }

/// Append-only JSONL writer; one write(2) per line so every line is flushed.
final class JSONLWriter {
    private var fd: Int32 = -1

    init(path: String?) {
        guard let path = path else { return }
        fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0o644)
    }

    func write(_ obj: [String: Any]) {
        guard fd >= 0, JSONSerialization.isValidJSONObject(obj),
              var data = try? JSONSerialization.data(
                  withJSONObject: obj, options: [.sortedKeys, .withoutEscapingSlashes])
        else { return }
        data.append(0x0A)
        data.withUnsafeBytes { buf in
            _ = Darwin.write(fd, buf.baseAddress, buf.count)
        }
    }
}

func installMainMenu() {
    let main = NSMenu()
    let appItem = NSMenuItem()
    main.addItem(appItem)
    let appMenu = NSMenu()
    appMenu.addItem(
        withTitle: "Quit Bench Sentinel", action: #selector(NSApplication.terminate(_:)),
        keyEquivalent: "q")
    appItem.submenu = appMenu
    let editItem = NSMenuItem()
    main.addItem(editItem)
    let edit = NSMenu(title: "Edit")
    edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
    edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
    edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
    edit.addItem(
        withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
    editItem.submenu = edit
    NSApp.mainMenu = main
}

final class SentinelDelegate: NSObject, NSApplicationDelegate, NSTextFieldDelegate {
    let log: JSONLWriter
    let noActivate: Bool
    var userActivity = false
    var frameTopLeft: NSRect?
    let userScript = Array("the user keeps writing notes while the agent works in the background. ")
    var userIndex = 0
    var userExpected = ""
    var userTimer: DispatchSourceTimer?
    let selfBid = Bundle.main.bundleIdentifier ?? "ai.cua.benchsentinel"
    var armed = false
    var window: NSWindow!
    var field: NSTextField!
    var monitor: Any?
    var signalSources: [DispatchSourceSignal] = []
    var sampler: DispatchSourceTimer?
    var activity: NSObjectProtocol?

    init(log: JSONLWriter, noActivate: Bool) {
        self.log = log
        self.noActivate = noActivate
    }

    // MARK: logging

    func frontInfo(_ app: NSRunningApplication? = NSWorkspace.shared.frontmostApplication)
        -> [String: Any]
    {
        return [
            "bid": app?.bundleIdentifier ?? NSNull(),
            "pid": Int(app?.processIdentifier ?? -1),
        ]
    }

    func idleInfo() -> [String: Any] {
        func idle(_ t: CGEventType) -> Double {
            let v = CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: t)
            return v.isFinite ? v : -1  // NaN/inf would make the whole JSON line invalid
        }
        return [
            "move": idle(.mouseMoved), "down": idle(.leftMouseDown),
            "key": idle(.keyDown), "scroll": idle(.scrollWheel),
        ]
    }

    func mouseInfo() -> [Double] {
        let p = NSEvent.mouseLocation
        return [Double(p.x), Double(p.y)]
    }

    /// Every line carries "ev", "t" (epoch ms) and "armed".
    func emit(_ ev: String, _ extra: [String: Any] = [:]) {
        var line: [String: Any] = ["ev": ev, "t": nowMs(), "armed": armed]
        for (k, v) in extra { line[k] = v }
        log.write(line)
    }

    /// Owners of the normal-layer (layer 0) on-screen windows ordered above the sentinel's window,
    /// other than the sentinel's own (Amendment 14, CUA-1282: windows raised over the user's app).
    /// Owner names and pids need no Screen Recording permission; window titles are not read.
    func windowsAbove() -> [String] {
        guard let window = window, window.windowNumber > 0,
              let list = CGWindowListCopyWindowInfo(
                  [.optionOnScreenAboveWindow, .excludeDesktopElements],
                  CGWindowID(window.windowNumber)) as? [[String: Any]]
        else { return [] }
        let me = Int(getpid())
        var owners: [String] = []
        for w in list {
            guard (w[kCGWindowLayer as String] as? Int) == 0,
                  let pid = w[kCGWindowOwnerPID as String] as? Int, pid != me
            else { continue }
            if let a = w[kCGWindowAlpha as String] as? Double, a <= 0 { continue }
            if let b = w[kCGWindowBounds as String] as? [String: Any],
               let bw = b["Width"] as? Double, let bh = b["Height"] as? Double, bw <= 1 || bh <= 1
            { continue }
            let name = w[kCGWindowOwnerName as String] as? String ?? "pid:\(pid)"
            if !owners.contains(name) { owners.append(name) }
        }
        return owners
    }

    func sampleFields() -> [String: Any] {
        var out: [String: Any] = [
            "mouse": mouseInfo(), "front": frontInfo(), "active": NSApp.isActive,
            "key": window?.isKeyWindow ?? false, "idle": idleInfo(),
        ]
        let above = windowsAbove()
        out["above_n"] = above.count
        if !above.isEmpty { out["above"] = above }
        return out
    }

    // MARK: lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        installMainMenu()
        buildWindow()
        installObservers()
        installSignals()
        // Keep the 20 Hz sampler out of App Nap and timer coalescing.
        activity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiated, .latencyCritical], reason: "bench sentinel sampling")
        startSampler()
        if userActivity { startUserActivity() }
        emit("start", ["pid": Int(getpid()), "bid": selfBid, "front": frontInfo()])
        if !noActivate { activate() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return true
    }

    func buildWindow() {
        let w: CGFloat = frameTopLeft?.width ?? 360, h: CGFloat = frameTopLeft?.height ?? 140
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: w, height: h),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered, defer: false)
        window.title = userActivity ? "User activity" : "Bench Sentinel"
        window.isReleasedWhenClosed = false
        let screen = NSScreen.screens.first ?? NSScreen.main
        let vis = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        // Outer frame is exactly 360x140, bottom-right of the primary screen.
        if let f = frameTopLeft {
            let screenH = screen?.frame.height ?? 900
            window.setFrame(NSRect(x: f.minX, y: screenH - f.minY - h, width: w, height: h), display: false)
        } else {
            window.setFrame(
                NSRect(x: vis.maxX - w - 24, y: vis.minY + 24, width: w, height: h), display: false)
        }

        field = NSTextField(frame: NSRect(x: 20, y: 40, width: w - 40, height: 28))
        field.placeholderString = userActivity ? "notes" : "sentinel"
        field.delegate = self
        field.setAccessibilityLabel("Sentinel input")
        window.contentView?.addSubview(field)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(field)
    }

    func installObservers() {
        let nc = NotificationCenter.default
        nc.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main
        ) { [weak self] _ in self?.emit("didBecomeKey", ["front": self?.frontInfo() ?? [:]]) }
        nc.addObserver(
            forName: NSWindow.didResignKeyNotification, object: window, queue: .main
        ) { [weak self] _ in self?.emit("didResignKey", ["front": self?.frontInfo() ?? [:]]) }
        nc.addObserver(
            forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.emit("didBecomeActive", ["front": self?.frontInfo() ?? [:]]) }
        nc.addObserver(
            forName: NSApplication.didResignActiveNotification, object: nil, queue: .main
        ) { [weak self] _ in self?.emit("didResignActive", ["front": self?.frontInfo() ?? [:]]) }
        // Catches focus flips shorter than one sampler tick.
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] note in
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            self?.emit("front", ["front": self?.frontInfo(app) ?? [:]])
        }

        let mask: NSEvent.EventTypeMask = [
            .keyDown, .leftMouseDown, .rightMouseDown, .otherMouseDown,
            .leftMouseUp, .rightMouseUp, .otherMouseUp, .scrollWheel,
        ]
        monitor = NSEvent.addLocalMonitorForEvents(matching: mask) { [weak self] event in
            self?.record(event)
            return event
        }
    }

    func record(_ e: NSEvent) {
        let loc = [Double(e.locationInWindow.x), Double(e.locationInWindow.y)]
        switch e.type {
        case .keyDown:
            emit("keyDown", ["chars": e.characters ?? "", "keyCode": Int(e.keyCode)])
        case .leftMouseDown, .rightMouseDown, .otherMouseDown:
            emit("mouseDown", ["button": e.buttonNumber, "loc": loc])
        case .leftMouseUp, .rightMouseUp, .otherMouseUp:
            emit("mouseUp", ["button": e.buttonNumber, "loc": loc])
        case .scrollWheel:
            emit(
                "scrollWheel",
                ["dx": Double(e.scrollingDeltaX), "dy": Double(e.scrollingDeltaY)])
        default:
            break
        }
    }

    func controlTextDidChange(_ obj: Notification) {
        emit("text", ["len": field.stringValue.count])
    }

    // MARK: signals

    func installSignals() {
        func source(_ sig: Int32, _ handler: @escaping () -> Void) {
            signal(sig, SIG_IGN)
            let s = DispatchSource.makeSignalSource(signal: sig, queue: .main)
            s.setEventHandler(handler: handler)
            s.resume()
            signalSources.append(s)
        }
        source(SIGUSR1) { [weak self] in self?.activate() }
        source(SIGUSR2) { [weak self] in self?.toggleArmed() }
        source(SIGTERM) { [weak self] in
            self?.emit("exit")
            NSApp.terminate(nil)
        }
    }

    func activate() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(field)
        emit("activate", ["front": frontInfo()])
        // Cooperative activation can be refused; ask LaunchServices as a fallback.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
            guard let self = self, !NSApp.isActive else { return }
            self.emit("activate_fallback", ["front": self.frontInfo()])
            let cfg = NSWorkspace.OpenConfiguration()
            cfg.activates = true
            NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: cfg) {
                _, _ in
            }
        }
    }

    func toggleArmed() {
        armed.toggle()
        if userActivity && !armed {
            emit(
                "user_text",
                ["expected_len": userExpected.count, "actual_len": field.stringValue.count,
                 "intact": field.stringValue == userExpected])
        }
        if userActivity && armed { userExpected = field.stringValue }
        emit(armed ? "armed" : "disarmed", sampleFields())
    }

    // MARK: simulated user activity (Amendment 15)

    func startUserActivity() {
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + 1.0, repeating: .milliseconds(1000), leeway: .milliseconds(5))
        t.setEventHandler { [weak self] in self?.userTick() }
        t.resume()
        userTimer = t
    }

    func userTick() {
        guard armed else { return }
        if field.stringValue != userExpected {
            emit("user_text_disrupted", ["expected_len": userExpected.count, "actual_len": field.stringValue.count])
            userExpected = field.stringValue
        }
        let ch = String(userScript[userIndex % userScript.count])
        guard NSApp.isActive, window.isKeyWindow,
              let editor = field.currentEditor() as? NSTextView
        else {
            emit("user_blocked", ["active": NSApp.isActive, "key": window.isKeyWindow, "front": frontInfo()])
            return
        }
        let end = (editor.string as NSString).length
        editor.setSelectedRange(NSRange(location: end, length: 0))
        editor.insertText(ch, replacementRange: NSRange(location: end, length: 0))
        userIndex += 1
        userExpected = field.stringValue
        emit("user_type", ["n": userIndex])
    }

    // MARK: sampler

    func startSampler() {
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + 0.05, repeating: .milliseconds(50), leeway: .milliseconds(1))
        t.setEventHandler { [weak self] in
            guard let self = self else { return }
            self.emit("s", self.sampleFields())
        }
        t.resume()
        sampler = t
    }
}

@main
enum BenchSentinelMain {
    @MainActor
    static func main() {
        let args = CommandLine.arguments
        var logPath: String?
        var noActivate = false
        var userActivity = false
        var frame: NSRect?
        var i = 1
        while i < args.count {
            switch args[i] {
            case "--log":
                if i + 1 < args.count { logPath = args[i + 1]; i += 1 }
            case "--no-activate":
                noActivate = true
            case "--user-activity":
                userActivity = true
            case "--frame":
                if i + 1 < args.count {
                    let v = args[i + 1].split(separator: ",").compactMap { Double($0) }
                    if v.count == 4 { frame = NSRect(x: v[0], y: v[1], width: v[2], height: v[3]) }
                    i += 1
                }
            default:
                break
            }
            i += 1
        }
        if logPath == nil {
            FileHandle.standardError.write(
                Data("BenchSentinel: no --log PATH given, nothing will be logged\n".utf8))
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = SentinelDelegate(log: JSONLWriter(path: logPath), noActivate: noActivate)
        delegate.userActivity = userActivity
        delegate.frameTopLeft = frame
        app.delegate = delegate
        app.run()
    }
}
