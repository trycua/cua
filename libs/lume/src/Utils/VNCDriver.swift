import Foundation

/// Thin wrapper around the `vncdotool` CLI (`pip3 install vncdotool`).
///
/// Used only by `lume sip` to drive the recoveryOS UI. The tool is invoked
/// per action via `Process`, and all args go through argv (never a shell).
///
/// Known limitations of vncdotool against macOS VNC servers:
///   - `type` doesn't shift correctly: uppercase, `_`, `&`, etc. get
///     corrupted. Only send unshifted characters through `type`.
///   - Cmd-key chords (⇧⌘T etc.) do NOT reliably route through VZ's VNC
///     server. Use menu-bar clicks + arrow-key navigation instead.
///
/// A future commit can swap this for a native Swift RFB client without
/// changing call sites. The in-tree `VNCClient` actor is a candidate, but
/// has an outstanding NWConnection stall when invoked from certain
/// `@MainActor` call chains that needs investigation before use here.
struct VNCDriver {
    let vncdo: String
    let host: String
    let port: Int
    let password: String
    let screenshotDir: String?

    private var target: String { "\(host)::\(port)" }

    /// Find `vncdo` (or `vncdotool`) on PATH. Also checks the pip3
    /// user-scripts directory (`~/Library/Python/<ver>/bin`) since
    /// `pip3 install --user vncdotool` puts it there and that's often not
    /// on PATH by default.
    static func locateVncdo() -> String? {
        for name in ["vncdo", "vncdotool"] {
            if let hit = whichPath(name) { return hit }
        }
        let home = NSHomeDirectory()
        let pythonBase = "\(home)/Library/Python"
        if let dirs = try? FileManager.default.contentsOfDirectory(atPath: pythonBase) {
            for dir in dirs {
                for name in ["vncdo", "vncdotool"] {
                    let candidate = "\(pythonBase)/\(dir)/bin/\(name)"
                    if FileManager.default.isExecutableFile(atPath: candidate) {
                        return candidate
                    }
                }
            }
        }
        return nil
    }

    private static func whichPath(_ name: String) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/which")
        p.arguments = [name]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = Pipe()
        try? p.run()
        p.waitUntilExit()
        guard p.terminationStatus == 0,
            let data = try? out.fileHandleForReading.readToEnd(),
            let str = String(data: data, encoding: .utf8)
        else { return nil }
        let trimmed = str.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    // MARK: - Public API

    /// Wait until the VNC server is accepting TCP connections.
    func waitForListening(deadlineSeconds: Double) async throws {
        let deadline = Date().addingTimeInterval(deadlineSeconds)
        while Date() < deadline {
            if canConnect() { return }
            try await sleep(seconds: 1)
        }
        throw VNCDriverError.timeout(
            "VNC server never became reachable on \(target)")
    }

    /// Poll `capture` until the returned PNG exceeds `minSize` bytes — a
    /// heuristic that reliably distinguishes "framebuffer still black / boot
    /// splash" from "actual UI rendered."
    func waitForFramebuffer(minSize: Int, deadlineSeconds: Double, label: String) async throws {
        let deadline = Date().addingTimeInterval(deadlineSeconds)
        let tempPath = "/tmp/lume-sip-fb-\(UUID().uuidString.prefix(8)).png"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }
        var lastSize = 0
        while Date() < deadline {
            do { try run(["capture", tempPath]) } catch { /* transient */ }
            let attrs = try? FileManager.default.attributesOfItem(atPath: tempPath)
            let size = (attrs?[.size] as? NSNumber)?.intValue ?? 0
            lastSize = size
            if size >= minSize {
                Logger.info(
                    "Framebuffer ready",
                    metadata: ["label": label, "png_bytes": "\(size)"])
                return
            }
            try await sleep(seconds: 4)
        }
        throw VNCDriverError.timeout(
            "Framebuffer for '\(label)' never reached \(minSize) bytes (last: \(lastSize))")
    }

    /// Type a plain string. Unshifted characters only — see the type doc.
    func type(_ text: String) throws {
        try run(["type", text])
    }

    /// Send a single named key (e.g. `enter`, `right`, `y`).
    func key(_ name: String) throws {
        try run(["key", name])
    }

    /// Send a keyboard chord. `modifiers` come from vncdotool's named-key
    /// vocabulary (`shift`, `ctrl`, `alt`, `super`/`cmd`).
    /// NOTE: VZ's VNC server does not reliably route Cmd chords; prefer
    /// menu-bar clicks + arrow navigation for menu shortcuts.
    func chord(modifiers: [String], key: String) throws {
        let chord = (modifiers + [key]).joined(separator: "-")
        try run(["key", chord])
    }

    /// Move the pointer to (x, y) and left-click.
    func click(x: Int, y: Int) throws {
        try run(["move", "\(x)", "\(y)", "click", "1"])
    }

    /// Snapshot the current framebuffer to the screenshot dir if configured.
    /// Silent no-op when the dir isn't set.
    func snapshot(_ tag: String) throws {
        guard let dir = screenshotDir else { return }
        try FileManager.default.createDirectory(
            atPath: dir, withIntermediateDirectories: true)
        let path = "\(dir)/\(tag).png"
        try run(["capture", path])
    }

    // MARK: - Internals

    private func canConnect() -> Bool {
        let host = CFHostCreateWithName(nil, self.host as CFString).takeRetainedValue()
        var readStream: Unmanaged<CFReadStream>?
        var writeStream: Unmanaged<CFWriteStream>?
        CFStreamCreatePairWithSocketToCFHost(
            nil, host, Int32(port), &readStream, &writeStream)
        defer {
            readStream?.release()
            writeStream?.release()
        }
        guard let r = readStream?.takeUnretainedValue() else { return false }
        let ok = CFReadStreamOpen(r)
        if ok { CFReadStreamClose(r) }
        return ok
    }

    private func run(_ args: [String]) throws {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: vncdo)
        p.arguments = ["-s", target, "-p", password] + args
        p.standardOutput = Pipe()
        p.standardError = Pipe()
        try p.run()
        p.waitUntilExit()
        if p.terminationStatus != 0 {
            throw VNCDriverError.commandFailed(args, p.terminationStatus)
        }
    }

    private func sleep(seconds: Double) async throws {
        try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }
}

enum VNCDriverError: Error, CustomStringConvertible {
    case timeout(String)
    case commandFailed([String], Int32)

    var description: String {
        switch self {
        case .timeout(let msg):
            return "VNC driver timeout: \(msg)"
        case .commandFailed(let args, let code):
            return "vncdotool \(args.joined(separator: " ")) exited with \(code)"
        }
    }
}
