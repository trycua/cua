// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// BenchSentinel: a tiny always-there window that stays frontmost and key while an
// agent works in other apps. It logs focus changes, leaked keys/clicks/scrolls and
// pointer position as JSONL so a run can be scored for foreground disturbance.
//
// Usage: BenchSentinel --log PATH [--no-activate] [--user-activity] [--mouse-user PATH] [--frame X,Y,W,H]
// --user-activity (Amendment 15): the window plays the user's own app. It is titled "User activity" and, while
// armed, "types" one character a second into its own field when it is active and key. A tick when it is not is
// logged as user_blocked; text that differs from what the user typed is logged as user_text_disrupted.
// --frame: outer frame in top-left screen coordinates (default 360x140 at the bottom right).
// --mouse-user PATH (shared-desktop tasks): the window is titled "User activity" and shows a canvas with numbered
// dots; while armed it plays a mouse user from the JSON script (see SHARED-DESKTOP.md): it moves the real pointer
// along straight legs, clicks each dot, checks the pointer against its own script, and logs untagged pointer events
// system-wide as mu_foreign. No typing user in this mode.
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

final class FlippedContentView: NSView {
    override var isFlipped: Bool { true }
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
    var mouseScript: MouseScript?
    var mouseUser: MouseUser?
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
        // Amendment 22 (multi-player background completion): the user's clipboard, whether the user's window is
        // on the active Space, and whether a sheet or modal window appeared on the user's app.
        out["clip"] = NSPasteboard.general.changeCount
        out["onspace"] = window?.isOnActiveSpace ?? false
        out["sheet"] = (window?.attachedSheet != nil) || (NSApp.modalWindow != nil)
            || NSApp.windows.contains { $0 !== window && $0.isVisible }
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
        if userActivity && mouseScript == nil { startUserActivity() }
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
        if let script = mouseScript {
            // Flipped content: the canvas sits near the top-left at (20, 20), the field below it.
            let content = FlippedContentView(frame: NSRect(x: 0, y: 0, width: w, height: h))
            window.contentView = content
            field.frame = NSRect(x: 20, y: 20 + script.canvas.h + 16, width: w - 40, height: 28)
            let canvas = MouseCanvasView(script: script, origin: NSPoint(x: 20, y: 20))
            content.addSubview(canvas)
            mouseUser = MouseUser(script: script, canvas: canvas) { [weak self] ev, extra in
                self?.emit(ev, extra)
            }
        }
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
        // Amendment 22: a desktop Space or full-screen switch.
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.emit("space", ["onspace": self?.window?.isOnActiveSpace ?? false])
        }
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
            // The mouse user's own tagged clicks are not leaks.
            if e.cgEvent?.getIntegerValueField(.eventSourceUserData) == muMagic { break }
            emit("mouseDown", ["button": e.buttonNumber, "loc": loc])
        case .leftMouseUp, .rightMouseUp, .otherMouseUp:
            if e.cgEvent?.getIntegerValueField(.eventSourceUserData) == muMagic { break }
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
        if armed { mouseUser?.stop() }
        armed.toggle()
        if userActivity && mouseUser == nil && !armed {
            emit(
                "user_text",
                ["expected_len": userExpected.count, "actual_len": field.stringValue.count,
                 "intact": field.stringValue == userExpected])
        }
        if userActivity && mouseUser == nil && armed { userExpected = field.stringValue }
        emit(armed ? "armed" : "disarmed", sampleFields())
        if armed { mouseUser?.start() }
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

// MARK: - Mouse user (shared-desktop tasks)

/// Tag on every event the mouse user posts (kCGEventSourceUserData); anything else is someone else's.
let muMagic: Int64 = 0x4344_4253_4D55_0001

/// One action of the mouse user: a click on a dot, or a drag (button held) from one dot to another.
enum MouseLeg {
    case click(Int)
    case drag(from: Int, to: Int)
}

struct MouseScript {
    var canvas: (w: Double, h: Double)
    var home: (x: Double, y: Double)
    var dots: [(x: Double, y: Double)]
    var radius: Double
    var sequence: [Int]
    /// Schema 2 `plan`; a schema-1 script has none and the plan is its `sequence` as click legs.
    var plan: [MouseLeg]
    var hasPlan = false
    var dragStepMs = 8.0, dragStepPx = 12.0, dragHoldMs = 40.0, dragTimeoutMs = 600.0
    var stepMs: Double, stepPx: Double, dwellBeforeMs: Double, pressMs: Double, dwellAfterMs: Double
    var tolerancePx: Double, clickTimeoutMs: Double

    init?(path: String) {
        guard let data = FileManager.default.contents(atPath: path),
              let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let canvas = (j["canvas"] as? [NSNumber])?.map({ $0.doubleValue }), canvas.count == 2,
              let home = (j["home"] as? [NSNumber])?.map({ $0.doubleValue }), home.count == 2,
              let rawDots = j["dots"] as? [[String: Any]],
              let radius = (j["radius"] as? NSNumber)?.doubleValue
        else { return nil }
        func num(_ k: String) -> Double? { (j[k] as? NSNumber)?.doubleValue }
        guard let stepMs = num("step_ms"), let stepPx = num("step_px"), stepPx > 0,
              let before = num("dwell_before_ms"), let press = num("press_ms"),
              let after = num("dwell_after_ms"), let tol = num("tolerance_px"),
              let timeout = num("click_timeout_ms")
        else { return nil }
        let rawPlan = j["plan"] as? [[String: Any]]
        let seq = (j["sequence"] as? [NSNumber])?.map({ $0.intValue }) ?? []
        guard rawPlan != nil || !seq.isEmpty else { return nil }
        var dots = [(x: Double, y: Double)](repeating: (0, 0), count: rawDots.count)
        for d in rawDots {
            guard let i = (d["index"] as? NSNumber)?.intValue, i >= 0, i < dots.count,
                  let x = (d["x"] as? NSNumber)?.doubleValue, let y = (d["y"] as? NSNumber)?.doubleValue
            else { return nil }
            dots[i] = (x, y)
        }
        guard seq.allSatisfy({ $0 >= 0 && $0 < dots.count }) else { return nil }
        var legs: [MouseLeg] = []
        if let rawPlan = rawPlan {
            for l in rawPlan {
                func idx(_ k: String) -> Int? {
                    guard let v = (l[k] as? NSNumber)?.intValue, v >= 0, v < dots.count else { return nil }
                    return v
                }
                switch l["kind"] as? String {
                case "click":
                    guard let d = idx("dot") else { return nil }
                    legs.append(.click(d))
                case "drag":
                    guard let a = idx("from"), let b = idx("to") else { return nil }
                    legs.append(.drag(from: a, to: b))
                default:
                    return nil
                }
            }
            guard !legs.isEmpty else { return nil }
            hasPlan = true
            if let v = num("drag_step_ms") { dragStepMs = v }
            if let v = num("drag_step_px"), v > 0 { dragStepPx = v }
            if let v = num("drag_hold_ms") { dragHoldMs = v }
            if let v = num("drag_timeout_ms") { dragTimeoutMs = v }
        } else {
            legs = seq.map { .click($0) }
        }
        self.plan = legs
        self.canvas = (canvas[0], canvas[1])
        self.home = (home[0], home[1])
        self.dots = dots
        self.radius = radius
        self.sequence = seq
        self.stepMs = stepMs
        self.stepPx = stepPx
        self.dwellBeforeMs = before
        self.pressMs = press
        self.dwellAfterMs = after
        self.tolerancePx = tol
        self.clickTimeoutMs = timeout
    }

    /// Index of the dot whose disc contains canvas point (x, y).
    func dotAt(_ x: Double, _ y: Double) -> Int? {
        for (i, d) in dots.enumerated() where (x - d.x) * (x - d.x) + (y - d.y) * (y - d.y) <= radius * radius {
            return i
        }
        return nil
    }

    /// Points of one leg from p to q, excluding p and ending exactly at q.
    func leg(from p: (x: Double, y: Double), to q: (x: Double, y: Double), stepPx: Double? = nil)
        -> [(x: Double, y: Double)]
    {
        let step = stepPx ?? self.stepPx
        let dist = hypot(q.x - p.x, q.y - p.y)
        let n = max(1, Int((dist / step).rounded(.up)))
        return (1...n).map { k in
            (floor(p.x + (q.x - p.x) * Double(k) / Double(n) + 0.5),
             floor(p.y + (q.y - p.y) * Double(k) / Double(n) + 0.5))
        }
    }
}

/// Drawing area of the user window: the numbered dots, a short flash on every button-down it receives.
final class MouseCanvasView: NSView {
    let script: MouseScript
    var onMouseDown: ((NSEvent) -> Void)?
    var onMouseUp: ((NSEvent) -> Void)?
    private var flashDot: Int?
    private var flashUntil = Date.distantPast

    init(script: MouseScript, origin: NSPoint) {
        self.script = script
        super.init(frame: NSRect(x: origin.x, y: origin.y, width: script.canvas.w, height: script.canvas.h))
        setAccessibilityLabel("User canvas")
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    override var isFlipped: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) { onMouseDown?(event) }
    override func mouseUp(with event: NSEvent) { onMouseUp?(event) }

    func flash(_ dot: Int?) {
        flashDot = dot
        flashUntil = Date().addingTimeInterval(0.15)
        needsDisplay = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in self?.needsDisplay = true }
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(srgbRed: 0.96, green: 0.96, blue: 0.97, alpha: 1).setFill()
        bounds.fill()
        let flashing = Date() < flashUntil
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.boldSystemFont(ofSize: 14), .foregroundColor: NSColor.white,
        ]
        for (i, d) in script.dots.enumerated() {
            let r = NSRect(x: d.x - script.radius, y: d.y - script.radius, width: script.radius * 2, height: script.radius * 2)
            let hit = flashing && flashDot == i
            (hit ? NSColor.systemOrange : NSColor.systemBlue).setFill()
            NSBezierPath(ovalIn: r).fill()
            let label = NSString(string: String(i + 1))
            let size = label.size(withAttributes: attrs)
            label.draw(at: NSPoint(x: d.x - size.width / 2, y: d.y - size.height / 2), withAttributes: attrs)
        }
        if flashing && flashDot == nil {
            NSColor.systemRed.setStroke()
            let p = NSBezierPath(rect: bounds.insetBy(dx: 1, dy: 1))
            p.lineWidth = 2
            p.stroke()
        }
    }
}

private func muMonoMs() -> Double { Double(DispatchTime.now().uptimeNanoseconds) / 1e6 }

private let muTapCallback: CGEventTapCallBack = { _, type, event, refcon in
    if let refcon = refcon {
        Unmanaged<MouseUser>.fromOpaque(refcon).takeUnretainedValue().foreign(type, event)
    }
    return Unmanaged.passUnretained(event)
}

/// The simulated mouse user: a state machine on a main-queue timer, so the clicks the canvas receives are matched
/// on the same queue that posts them.
final class MouseUser {
    private enum Phase { case idle, homing, moving, press, release, dwellAfter, dragMoving, dragHold, dragRelease }

    /// A drag whose result is not known yet: the canvas reports its button-down and button-up.
    private struct DragPending {
        var n: Int, from: Int, to: Int
        var downSeen = false
        var downWhy = "lost"
        var cut = false
        var releasedAt: Double?
        var upOk = false
        var upSeen = false
    }

    let script: MouseScript
    let canvas: MouseCanvasView
    let emit: (String, [String: Any]) -> Void
    private var phase = Phase.idle
    private var started = false
    private var nextAt = 0.0
    private var homeDeadline = 0.0
    private var timer: DispatchSourceTimer?
    private var tap: CFMachPort?
    private var tapAdded = false
    private(set) var tagging = false
    private var last = (x: 0.0, y: 0.0)
    private var prev = (x: 0.0, y: 0.0)
    private var points: [(x: Double, y: Double)] = []
    private var pointIdx = 0
    private var leg = 0
    private var target = 0
    private var checks = 0
    private var clicks = 0
    private var buttonDown = false
    private var pending: (n: Int, dot: Int, deadline: Double)?
    private var drag: DragPending?
    private var curLeg = MouseLeg.click(0)
    private var dragFrom = (x: 0.0, y: 0.0)
    private let source = CGEventSource(stateID: .hidSystemState)

    init(script: MouseScript, canvas: MouseCanvasView, emit: @escaping (String, [String: Any]) -> Void) {
        self.script = script
        self.canvas = canvas
        self.emit = emit
        canvas.onMouseDown = { [weak self] e in self?.canvasMouseDown(e) }
        canvas.onMouseUp = { [weak self] e in self?.canvasMouseUp(e) }
        let t = DispatchSource.makeTimerSource(queue: .main)
        t.schedule(deadline: .now() + 0.005, repeating: .milliseconds(2), leeway: .microseconds(500))
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        timer = t
    }

    // MARK: arm / disarm

    func start() {
        installTap()
        var startInfo: [String: Any] = ["tagging": tagging, "tolerance_px": script.tolerancePx, "platform": "macos"]
        if script.hasPlan { startInfo["drag_timeout_ms"] = script.dragTimeoutMs }  // schema 2 only
        emit("mu_start", startInfo)
        started = true
        leg = 0
        clicks = 0
        checks = 0
        pending = nil
        drag = nil
        last = script.home
        prev = script.home
        guard let g = globalPoint(script.home) else { fail(); return }
        post(.mouseMoved, g)
        phase = .homing
        homeDeadline = muMonoMs() + 200
        nextAt = 0
    }

    func stop() {
        guard started else { return }
        if buttonDown, let g = globalPoint(last) {
            post(.leftMouseUp, g)
            buttonDown = false
        }
        phase = .idle
        pending = nil
        drag = nil  // an unfinished drag is dropped
        started = false
        emit("mu_stop", [:])
    }

    private func fail() {
        emit("mu_error", ["why": "cannot move the pointer"])
        phase = .idle
    }

    // MARK: geometry and posting

    /// Top-left screen origin of the canvas, from its current on-screen rect.
    private func canvasOrigin() -> CGPoint? {
        guard let win = canvas.window else { return nil }
        let r = win.convertToScreen(canvas.convert(canvas.bounds, to: nil))
        let screenH = (NSScreen.screens.first ?? NSScreen.main)?.frame.height ?? 900
        return CGPoint(x: r.minX, y: screenH - r.maxY)
    }

    private func globalPoint(_ p: (x: Double, y: Double)) -> CGPoint? {
        guard let o = canvasOrigin() else { return nil }
        return CGPoint(x: o.x + p.x, y: o.y + p.y)
    }

    private func post(_ type: CGEventType, _ at: CGPoint) {
        guard let e = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: at, mouseButton: .left)
        else { return }
        if type != .mouseMoved { e.setIntegerValueField(.mouseEventClickState, value: 1) }
        e.setIntegerValueField(.eventSourceUserData, value: muMagic)
        e.post(tap: .cghidEventTap)
    }

    private func realPointer() -> CGPoint? { CGEvent(source: nil)?.location }

    /// Read the real pointer, log it against the last two commanded points.
    private func check() {
        guard let loc = realPointer(), let o = canvasOrigin() else { return }
        let got = (x: Double(loc.x - o.x), y: Double(loc.y - o.y))
        let d = min(hypot(got.x - last.x, got.y - last.y), hypot(got.x - prev.x, got.y - prev.y))
        if drag != nil && buttonDown && d > script.tolerancePx { drag?.cut = true }
        emit("mu", ["i": checks, "want": [last.x, last.y], "got": [got.x, got.y], "d": d])
        checks += 1
    }

    // MARK: state machine

    private func tick() {
        guard started else { return }
        let now = muMonoMs()
        if let p = pending, now >= p.deadline {
            pending = nil
            emit("mu_click_result", ["n": p.n, "ok": false, "why": "lost"])
        }
        if let d = drag, let rel = d.releasedAt, now >= rel + script.dragTimeoutMs { resolveDrag() }
        guard now >= nextAt else { return }
        switch phase {
        case .idle:
            break
        case .homing:
            if let g = globalPoint(script.home), let loc = realPointer(),
               hypot(Double(loc.x - g.x), Double(loc.y - g.y)) <= script.tolerancePx
            {
                startLeg(now)
            } else if now >= homeDeadline {
                fail()
            }
        case .moving:
            check()
            let pt = points[pointIdx]
            if let g = globalPoint(pt) { post(.mouseMoved, g) }
            prev = last
            last = pt
            pointIdx += 1
            if pointIdx >= points.count {
                phase = .press
                nextAt = now + script.stepMs + script.dwellBeforeMs
            } else {
                nextAt = now + script.stepMs
            }
        case .press:
            check()
            clicks += 1
            if case let .drag(_, to) = curLeg {
                resolveDrag()  // a stale unresolved drag
                drag = DragPending(n: clicks, from: target, to: to)
                dragFrom = last
                if let g = globalPoint(last) { post(.leftMouseDown, g) }
                buttonDown = true
                points = script.leg(from: last, to: script.dots[to], stepPx: script.dragStepPx)
                pointIdx = 0
                phase = .dragMoving
                nextAt = now + script.dragStepMs
            } else {
                if let g = globalPoint(last) { post(.leftMouseDown, g) }
                buttonDown = true
                pending = (clicks, target, now + script.clickTimeoutMs)
                phase = .release
                nextAt = now + script.pressMs
            }
        case .dragMoving:
            check()
            let pt = points[pointIdx]
            if let g = globalPoint(pt) { post(.leftMouseDragged, g) }
            prev = last
            last = pt
            pointIdx += 1
            if pointIdx >= points.count {
                phase = .dragHold
                nextAt = now + script.dragHoldMs
            } else {
                nextAt = now + script.dragStepMs
            }
        case .dragHold:
            phase = .dragRelease
        case .dragRelease:
            if drag != nil { drag?.releasedAt = now }
            if let g = globalPoint(last) { post(.leftMouseUp, g) }
            buttonDown = false
            if case let .drag(a, b) = curLeg {
                emit(
                    "mu_drag",
                    ["n": clicks, "from": a, "to": b, "at_from": [Int(dragFrom.x), Int(dragFrom.y)],
                     "at_to": [Int(last.x), Int(last.y)]])
            }
            phase = .dwellAfter
            nextAt = now + script.dwellAfterMs
        case .release:
            if let g = globalPoint(last) { post(.leftMouseUp, g) }
            buttonDown = false
            emit("mu_click", ["n": clicks, "dot": target, "at": [Int(last.x), Int(last.y)]])
            phase = .dwellAfter
            nextAt = now + script.dwellAfterMs
        case .dwellAfter:
            startLeg(now)
        }
    }

    private func startLeg(_ now: Double) {
        curLeg = script.plan[leg % script.plan.count]
        switch curLeg {
        case .click(let d): target = d
        case .drag(let a, _): target = a
        }
        leg += 1
        points = script.leg(from: last, to: script.dots[target])
        pointIdx = 0
        phase = .moving
        nextAt = now
    }

    // MARK: clicks received by the canvas

    private func canvasMouseDown(_ e: NSEvent) {
        let p = canvas.convert(e.locationInWindow, from: nil)
        let own = e.cgEvent?.getIntegerValueField(.eventSourceUserData) == muMagic
        let dot = script.dotAt(Double(p.x), Double(p.y))
        emit("mu_rx", ["dot": dot.map { $0 as Any } ?? NSNull(), "at": [Double(p.x), Double(p.y)], "own": own])
        canvas.flash(dot)
        if own, let d = drag, !d.downSeen {
            let c = script.dots[d.from]
            let near = hypot(Double(p.x) - c.x, Double(p.y) - c.y) <= script.tolerancePx
            drag?.downSeen = true
            if dot == d.from && near {
                drag?.downWhy = "landed"
            } else if let dot = dot, dot != d.from {
                drag?.downWhy = "wrong_dot"
            } else {
                drag?.downWhy = "off_target"
            }
            return
        }
        guard own, let pend = pending else { return }
        pending = nil
        let c = script.dots[pend.dot]
        let near = hypot(Double(p.x) - c.x, Double(p.y) - c.y) <= script.tolerancePx
        let why: String
        if dot == pend.dot && near {
            why = "landed"
        } else if let dot = dot, dot != pend.dot {
            why = "wrong_dot"
        } else {
            why = "off_target"
        }
        emit("mu_click_result", ["n": pend.n, "ok": why == "landed", "why": why])
    }

    /// The canvas received a button-up. Logs `mu_up`; settles the pending drag once its own up arrives.
    private func canvasMouseUp(_ e: NSEvent) {
        let p = canvas.convert(e.locationInWindow, from: nil)
        let tagged = e.cgEvent?.getIntegerValueField(.eventSourceUserData) == muMagic
        let dot = script.dotAt(Double(p.x), Double(p.y))
        var own = false
        if let d = drag, d.downSeen, d.downWhy == "landed", tagged, let rel = d.releasedAt {
            let c = script.dots[d.to]
            let near = hypot(Double(p.x) - c.x, Double(p.y) - c.y) <= script.tolerancePx
            own = near && muMonoMs() <= rel + script.dragTimeoutMs
        }
        emit("mu_up", ["dot": dot.map { $0 as Any } ?? NSNull(), "at": [Double(p.x), Double(p.y)], "own": own])
        guard drag != nil else { return }
        if !tagged {
            // Someone else's button-up (a stolen release) while the drag is pending.
            drag?.cut = true
            return
        }
        drag?.upSeen = true
        drag?.upOk = own
        if drag?.releasedAt != nil { resolveDrag() }
    }

    /// Emit the result of the pending drag (also on timeout, when the up never reached the canvas).
    private func resolveDrag() {
        guard let d = drag else { return }
        drag = nil
        let why: String
        if !d.downSeen {
            why = "lost"
        } else if d.downWhy != "landed" {
            why = d.downWhy
        } else if d.cut || !d.upOk {
            why = "cut"
        } else {
            why = "landed"
        }
        emit("mu_click_result", ["n": d.n, "ok": why == "landed", "why": why, "kind": "drag"])
    }

    // MARK: foreign pointer events

    private func installTap() {
        guard tap == nil else { return }
        let types: [CGEventType] = [
            .mouseMoved, .leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp,
            .otherMouseDown, .otherMouseUp, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged,
            .scrollWheel,
        ]
        let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << CGEventMask($1.rawValue)) }
        tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly,
            eventsOfInterest: mask, callback: muTapCallback,
            userInfo: Unmanaged.passUnretained(self).toOpaque())
        tagging = tap != nil
        if let tap = tap, !tapAdded {
            let src = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
            CFRunLoopAddSource(CFRunLoopGetMain(), src, .commonModes)
            tapAdded = true
        }
    }

    func foreign(_ type: CGEventType, _ event: CGEvent) {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            if let tap = tap { CGEvent.tapEnable(tap: tap, enable: true) }
            return
        }
        guard started, event.getIntegerValueField(.eventSourceUserData) != muMagic else { return }
        let kind: String
        switch type {
        case .mouseMoved: kind = "move"
        case .leftMouseDown, .rightMouseDown, .otherMouseDown: kind = "down"
        case .leftMouseUp, .rightMouseUp, .otherMouseUp: kind = "up"
        case .leftMouseDragged, .rightMouseDragged, .otherMouseDragged: kind = "drag"
        case .scrollWheel: kind = "scroll"
        default: return
        }
        if drag != nil && buttonDown { drag?.cut = true }
        emit("mu_foreign", ["kind": kind])
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
        var mousePath: String?
        var i = 1
        while i < args.count {
            switch args[i] {
            case "--log":
                if i + 1 < args.count { logPath = args[i + 1]; i += 1 }
            case "--no-activate":
                noActivate = true
            case "--user-activity":
                userActivity = true
            case "--mouse-user":
                if i + 1 < args.count { mousePath = args[i + 1]; i += 1 }
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
        var script: MouseScript?
        if let path = mousePath {
            guard let parsed = MouseScript(path: path) else {
                FileHandle.standardError.write(Data("BenchSentinel: cannot read the --mouse-user script\n".utf8))
                exit(2)
            }
            script = parsed
            userActivity = true  // the mouse user is the "User activity" window, without the typing user
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let delegate = SentinelDelegate(log: JSONLWriter(path: logPath), noActivate: noActivate)
        delegate.userActivity = userActivity
        delegate.mouseScript = script
        delegate.frameTopLeft = frame
        app.delegate = delegate
        app.run()
    }
}
