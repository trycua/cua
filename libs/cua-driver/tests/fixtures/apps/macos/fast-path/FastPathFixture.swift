// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.
//
// Fast-path race fixture (CUA-3102). A test, not a timer, decides when the
// UI changes: write one command per line to $DIR/cmd; the fixture applies it
// within 20 ms and appends "ack <n>" to $DIR/ack. Every press is logged to
// $DIR/log so a harness can tell a right click from a wrong one.
//
// Build: swiftc -O FastPathFixture.swift -o FastPathFixture
// Run:   ./FastPathFixture /tmp/fpfix
//
// Commands: move_alpha <dx> | swap | relabel_alpha <name> | remove_beta |
//           canvas_move <dx> | canvas_shape | reset

import AppKit

let dir = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/fpfix")
try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

func log(_ line: String) {
    let url = dir.appendingPathComponent("log")
    let data = (line + "\n").data(using: .utf8)!
    if let h = try? FileHandle(forWritingTo: url) {
        h.seekToEndOfFile(); h.write(data); try? h.close()
    } else {
        try? data.write(to: url)
    }
}

/// A canvas with no accessibility children: draws one tile.
final class Canvas: NSView {
    var tile = NSRect(x: 40, y: 40, width: 48, height: 28)
    var circle = false
    override var isFlipped: Bool { true }
    override func isAccessibilityElement() -> Bool { false }
    override func accessibilityChildren() -> [Any]? { [] }
    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 0.85, alpha: 1).setFill()
        bounds.fill()
        NSColor(white: 0.75, alpha: 1).setFill()
        for i in stride(from: 0, to: Int(bounds.width), by: 32) {
            NSRect(x: CGFloat(i), y: 0, width: 16, height: bounds.height).fill()
        }
        if circle {
            NSColor.systemOrange.setFill()
            NSBezierPath(ovalIn: tile.insetBy(dx: 8, dy: 0)).fill()
        } else {
            NSColor(white: 0.15, alpha: 1).setFill()
            tile.fill()
            NSColor.white.setFill()
            NSRect(x: tile.minX + 6, y: tile.minY + 10, width: 18, height: 6).fill()
            NSColor.systemBlue.setFill()
            NSRect(x: tile.minX + 30, y: tile.minY + 6, width: 10, height: 16).fill()
        }
    }
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if tile.contains(p) && !circle {
            log("canvas hit")
        } else {
            log("canvas miss \(Int(p.x)),\(Int(p.y))")
        }
    }
}

final class Fixture: NSObject, NSApplicationDelegate {
    let window = NSWindow(contentRect: NSRect(x: 120, y: 120, width: 720, height: 520),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 720, height: 520))
    var alpha: NSButton!
    var beta: NSButton!
    var fields: [NSTextField] = []
    var checks: [NSButton] = []
    let canvas = Canvas(frame: NSRect(x: 380, y: 200, width: 320, height: 280))
    var seen = 0

    func button(_ title: String, _ frame: NSRect) -> NSButton {
        let b = NSButton(title: title, target: self, action: #selector(pressed(_:)))
        b.frame = frame
        content.addSubview(b)
        return b
    }

    @objc func pressed(_ sender: NSButton) {
        if sender.title == "Submit" {
            log("submit " + fields.map { $0.stringValue }.joined(separator: "|"))
        } else {
            log("pressed \(sender.title)")
        }
    }

    @objc func toggled(_ sender: NSButton) {
        log("toggled \(sender.title) \(sender.state == .on ? 1 : 0)")
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        window.title = "FastPath Fixture"
        window.contentView = content
        for (i, name) in ["Name", "Email", "City"].enumerated() {
            let label = NSTextField(labelWithString: name)
            label.frame = NSRect(x: 20, y: 470 - i * 40, width: 60, height: 22)
            content.addSubview(label)
            let field = NSTextField(frame: NSRect(x: 90, y: 470 - i * 40, width: 240, height: 24))
            field.placeholderString = name
            field.setAccessibilityLabel(name)
            content.addSubview(field)
            fields.append(field)
        }
        _ = button("Submit", NSRect(x: 90, y: 340, width: 100, height: 30))
        for i in 1...6 {
            let c = NSButton(checkboxWithTitle: "Option \(i)", target: self, action: #selector(toggled(_:)))
            c.frame = NSRect(x: 20, y: 290 - (i - 1) * 28, width: 160, height: 22)
            content.addSubview(c)
            checks.append(c)
        }
        alpha = button("Alpha", NSRect(x: 200, y: 260, width: 100, height: 30))
        beta = button("Beta", NSRect(x: 200, y: 200, width: 100, height: 30))
        content.addSubview(canvas)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { [weak self] _ in self?.poll() }
    }

    func poll() {
        guard let text = try? String(contentsOf: dir.appendingPathComponent("cmd"), encoding: .utf8) else { return }
        let lines = text.split(separator: "\n").map(String.init)
        while seen < lines.count {
            apply(lines[seen])
            seen += 1
            let ack = dir.appendingPathComponent("ack")
            try? "ack \(seen)\n".data(using: .utf8)!.write(to: ack)
        }
    }

    func apply(_ line: String) {
        let parts = line.split(separator: " ").map(String.init)
        guard let cmd = parts.first else { return }
        let arg = parts.count > 1 ? parts[1] : ""
        switch cmd {
        case "move_alpha": alpha.frame.origin.x += CGFloat(Double(arg) ?? 80)
        case "swap":
            let f = alpha.frame; alpha.frame = beta.frame; beta.frame = f
        case "relabel_alpha": alpha.title = arg.isEmpty ? "Gamma" : arg
        case "remove_beta": beta.removeFromSuperview()
        case "canvas_move": canvas.tile.origin.x += CGFloat(Double(arg) ?? 40); canvas.needsDisplay = true
        case "canvas_shape": canvas.circle = true; canvas.needsDisplay = true
        case "reset":
            alpha.title = "Alpha"; alpha.frame = NSRect(x: 200, y: 260, width: 100, height: 30)
            if beta.superview == nil { content.addSubview(beta) }
            beta.frame = NSRect(x: 200, y: 200, width: 100, height: 30)
            canvas.tile = NSRect(x: 40, y: 40, width: 48, height: 28); canvas.circle = false
            canvas.needsDisplay = true
            fields.forEach { $0.stringValue = "" }
            checks.forEach { $0.state = .off }
        default: log("unknown command \(line)")
        }
        log("applied \(line)")
    }
}

let app = NSApplication.shared
let delegate = Fixture()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
