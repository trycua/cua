import Darwin
import Foundation
import Vision

@MainActor protocol RecoveryVNCDriving {
    func waitForListening(deadlineSeconds: Double, heartbeat: () throws -> Void) async throws
    func waitForFramebuffer(minSize: Int, deadlineSeconds: Double, label: String, heartbeat: () throws -> Void)
        async throws
    func waitForText(
        containing expectedPhrases: [String], rejecting failurePhrases: [String], deadlineSeconds: Double,
        label: String, heartbeat: () throws -> Void
    ) async throws -> String
    func type(_ text: String) throws
    func typeSensitive(_ text: String) throws
    func key(_ name: String) throws
    func click(x: Int, y: Int) throws
    func snapshot(_ tag: String) throws
}

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
@MainActor struct VNCDriver: RecoveryVNCDriving {
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
        for name in ["vncdo", "vncdotool"] { if let hit = whichPath(name) { return hit } }
        let home = NSHomeDirectory()
        let pythonBase = "\(home)/Library/Python"
        if let dirs = try? FileManager.default.contentsOfDirectory(atPath: pythonBase) {
            for dir in dirs {
                for name in ["vncdo", "vncdotool"] {
                    let candidate = "\(pythonBase)/\(dir)/bin/\(name)"
                    if FileManager.default.isExecutableFile(atPath: candidate) { return candidate }
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
        guard p.terminationStatus == 0, let data = try? out.fileHandleForReading.readToEnd(),
            let str = String(data: data, encoding: .utf8)
        else { return nil }
        let trimmed = str.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    // MARK: - Public API

    /// Wait until the VNC server is accepting TCP connections.
    func waitForListening(deadlineSeconds: Double, heartbeat: () throws -> Void = {}) async throws {
        let deadline = Date().addingTimeInterval(deadlineSeconds)
        while Date() < deadline {
            try heartbeat()
            if canConnect() { return }
            try await sleep(seconds: 1)
        }
        throw VNCDriverError.timeout("VNC server never became reachable on \(target)")
    }

    /// Poll `capture` until the returned PNG exceeds `minSize` bytes — a
    /// heuristic that reliably distinguishes "framebuffer still black / boot
    /// splash" from "actual UI rendered."
    func waitForFramebuffer(minSize: Int, deadlineSeconds: Double, label: String, heartbeat: () throws -> Void = {})
        async throws
    {
        let deadline = Date().addingTimeInterval(deadlineSeconds)
        let tempPath = "/tmp/lume-sip-fb-\(UUID().uuidString.prefix(8)).png"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }
        var lastSize = 0
        var lastError: Error?
        while Date() < deadline {
            try heartbeat()
            do {
                try run(["capture", tempPath])
                lastError = nil
            } catch { lastError = error }
            let attrs = try? FileManager.default.attributesOfItem(atPath: tempPath)
            let size = (attrs?[.size] as? NSNumber)?.intValue ?? 0
            lastSize = size
            if size >= minSize {
                Logger.info("Framebuffer ready", metadata: ["label": label, "png_bytes": "\(size)"])
                return
            }
            try await sleep(seconds: 4)
        }
        let errorSuffix = lastError.map { "; last capture error: \($0)" } ?? ""
        throw VNCDriverError.timeout(
            "Framebuffer for '\(label)' never reached \(minSize) bytes " + "(last: \(lastSize))\(errorSuffix)")
    }

    /// Poll screenshots until OCR finds an expected terminal result. Failure
    /// phrases abort immediately so a bad password is reported before the
    /// workflow accidentally types the shutdown command into another prompt.
    func waitForText(
        containing expectedPhrases: [String], rejecting failurePhrases: [String], deadlineSeconds: Double,
        label: String, heartbeat: () throws -> Void = {}
    ) async throws -> String {
        let deadline = Date().addingTimeInterval(deadlineSeconds)
        let tempPath = "/tmp/lume-sip-ocr-\(UUID().uuidString.prefix(8)).png"
        defer { try? FileManager.default.removeItem(atPath: tempPath) }
        let expected = expectedPhrases.map(Self.normalizeText)
        let failures = failurePhrases.map(Self.normalizeText)
        var lastText = ""
        var lastError: Error?

        while Date() < deadline {
            try heartbeat()
            do {
                try run(["capture", tempPath])
                lastText = try Self.recognizeText(at: tempPath)
                lastError = nil
                let normalized = Self.normalizeText(lastText)

                if let phrase = failures.first(where: normalized.contains) {
                    throw VNCDriverError.unexpectedText("Framebuffer for '\(label)' contained failure text '\(phrase)'")
                }
                if expected.contains(where: normalized.contains) { return lastText }
            } catch let error as VNCDriverError {
                if case .unexpectedText = error { throw error }
                lastError = error
            } catch { lastError = error }
            try await sleep(seconds: 2)
        }

        let recognized = Self.normalizeText(lastText)
        let textSuffix = recognized.isEmpty ? "" : "; last OCR text: \(recognized)"
        let errorSuffix = lastError.map { "; last OCR error: \($0)" } ?? ""
        throw VNCDriverError.timeout(
            "Framebuffer for '\(label)' did not contain expected text" + "\(textSuffix)\(errorSuffix)")
    }

    /// Type a plain string. Unshifted characters only — see the type doc.
    func type(_ text: String) throws { try run(["type", text]) }

    /// Type a secret without putting it in the vncdotool process arguments.
    /// `typefile -` reads the value from the child process's standard input.
    func typeSensitive(_ text: String) throws {
        guard let data = text.data(using: .utf8) else { throw VNCDriverError.invalidSensitiveText }
        try run(["typefile", "-"], standardInput: data)
    }

    /// Send a single named key (e.g. `enter`, `right`, `y`).
    func key(_ name: String) throws { try run(["key", name]) }

    /// Send a keyboard chord. `modifiers` come from vncdotool's named-key
    /// vocabulary (`shift`, `ctrl`, `alt`, `super`/`cmd`).
    /// NOTE: VZ's VNC server does not reliably route Cmd chords; prefer
    /// menu-bar clicks + arrow navigation for menu shortcuts.
    func chord(modifiers: [String], key: String) throws {
        let chord = (modifiers + [key]).joined(separator: "-")
        try run(["key", chord])
    }

    /// Move the pointer to (x, y) and left-click.
    func click(x: Int, y: Int) throws { try run(["move", "\(x)", "\(y)", "click", "1"]) }

    /// Snapshot the current framebuffer to the screenshot dir if configured.
    /// Silent no-op when the dir isn't set.
    func snapshot(_ tag: String) throws {
        guard let dir = screenshotDir else { return }
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let path = "\(dir)/\(tag).png"
        try run(["capture", path])
    }

    // MARK: - Internals

    private func canConnect() -> Bool {
        let host = CFHostCreateWithName(nil, self.host as CFString).takeRetainedValue()
        var readStream: Unmanaged<CFReadStream>?
        var writeStream: Unmanaged<CFWriteStream>?
        CFStreamCreatePairWithSocketToCFHost(nil, host, Int32(port), &readStream, &writeStream)
        defer {
            readStream?.release()
            writeStream?.release()
        }
        guard let r = readStream?.takeUnretainedValue() else { return false }
        let ok = CFReadStreamOpen(r)
        if ok { CFReadStreamClose(r) }
        return ok
    }

    private func run(_ args: [String], standardInput: Data? = nil) throws {
        let outputURL = FileManager.default.temporaryDirectory.appendingPathComponent(
            "lume-vncdo-\(UUID().uuidString).log"
        )
        guard FileManager.default.createFile(
            atPath: outputURL.path,
            contents: nil,
            attributes: [.posixPermissions: 0o600]
        ) else {
            throw VNCDriverError.outputCaptureFailed
        }
        let outputHandle: FileHandle
        do {
            outputHandle = try FileHandle(forWritingTo: outputURL)
        } catch {
            try? FileManager.default.removeItem(at: outputURL)
            throw VNCDriverError.outputCaptureFailed
        }
        defer {
            try? outputHandle.close()
            try? FileManager.default.removeItem(at: outputURL)
        }

        let p = Process()
        p.executableURL = URL(fileURLWithPath: vncdo)
        p.arguments = ["-s", target, "-p", password, "-t", "30"] + args
        p.standardOutput = outputHandle
        p.standardError = outputHandle
        let input = standardInput.map { _ in Pipe() }
        p.standardInput = input
        try p.run()
        if let standardInput, let input {
            input.fileHandleForWriting.write(standardInput)
            try? input.fileHandleForWriting.close()
        }

        let deadline = Date().addingTimeInterval(35)
        while p.isRunning && Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        if p.isRunning {
            p.terminate()
            Thread.sleep(forTimeInterval: 0.2)
            if p.isRunning {
                Darwin.kill(p.processIdentifier, SIGKILL)
            }
            p.waitUntilExit()
            throw VNCDriverError.timeout("vncdotool command exceeded 35 seconds")
        }

        try? outputHandle.synchronize()
        let outputData = (try? Data(contentsOf: outputURL)) ?? Data()
        let outputText =
            String(data: outputData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if p.terminationStatus != 0 { throw VNCDriverError.commandFailed(args, p.terminationStatus, outputText) }
    }

    private static func recognizeText(at path: String) throws -> String {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = false
        request.recognitionLanguages = ["en-US"]
        let handler = VNImageRequestHandler(url: URL(fileURLWithPath: path))
        try handler.perform([request])
        return (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
    }

    private static func normalizeText(_ text: String) -> String {
        text.lowercased().split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }

    private func sleep(seconds: Double) async throws {
        try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }
}

enum VNCDriverError: Error, CustomStringConvertible {
    case timeout(String)
    case commandFailed([String], Int32, String)
    case unexpectedText(String)
    case invalidSensitiveText
    case outputCaptureFailed

    var description: String {
        switch self {
        case .timeout(let msg): return "VNC driver timeout: \(msg)"
        case .commandFailed(let args, let code, let output):
            let suffix = output.isEmpty ? "" : ": \(output)"
            return "vncdotool \(args.joined(separator: " ")) exited with \(code)\(suffix)"
        case .unexpectedText(let msg): return "VNC terminal check failed: \(msg)"
        case .invalidSensitiveText: return "The sensitive VNC input could not be encoded as UTF-8"
        case .outputCaptureFailed: return "Could not create a private vncdotool output log"
        }
    }
}
