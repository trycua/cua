import AppKit
import Foundation

/// Execute JavaScript in a running Electron app's renderer via the V8 inspector
/// (SIGUSR1 → CDP WebSocket → Runtime.evaluate).
///
/// **How it works:**
/// 1. Send SIGUSR1 to the Electron main process — Node activates the V8 inspector
///    on a random localhost port (same as `node --inspect`).
/// 2. Discover the new port by diffing the pid's TCP LISTEN sockets before/after.
/// 3. GET `http://localhost:{port}/json` for the CDP WebSocket URL.
/// 4. Open a WebSocket, send `Runtime.evaluate`, return the result.
///
/// **Requirement:** The app must have the `EnableNodeCliInspectArguments` Electron
/// fuse enabled (the default in development builds; some production apps disable it
/// post-CVE-2024-23738). Check with:
///   `npx @electron/fuses read --app /Applications/MyApp.app`
///
/// VS Code and Cursor both have this fuse ON. Slack/Discord likely have it OFF.
public enum ElectronJS {

    // MARK: - Public API

    public enum Error: Swift.Error, CustomStringConvertible {
        case notElectron(String)
        case inspectorNotAvailable(String)
        case cdpConnectionFailed(String)
        case evaluationFailed(String)

        public var description: String {
            switch self {
            case .notElectron(let detail):
                return "Not an Electron app: \(detail). "
                    + "Use the `page` tool only with Electron apps "
                    + "(bundle must contain Electron Framework.framework)."
            case .inspectorNotAvailable(let detail):
                return "V8 inspector did not activate after SIGUSR1: \(detail)\n"
                    + "The app may have the EnableNodeCliInspectArguments fuse disabled. "
                    + "Verify with: npx @electron/fuses read --app /path/to/App.app"
            case .cdpConnectionFailed(let detail):
                return "CDP connection failed: \(detail)"
            case .evaluationFailed(let detail):
                return "JavaScript evaluation failed: \(detail)"
            }
        }
    }

    /// True if the app running at `pid` is Electron-based — detected by the
    /// presence of `Electron Framework.framework` inside its bundle.
    public static func isElectron(pid: Int32) -> Bool {
        guard let app = NSWorkspace.shared.runningApplications
            .first(where: { $0.processIdentifier == pid }),
              let bundleURL = app.bundleURL else { return false }
        let frameworkPath = bundleURL
            .appendingPathComponent("Contents/Frameworks/Electron Framework.framework")
        return FileManager.default.fileExists(atPath: frameworkPath.path)
    }

    /// Execute `javascript` in the Electron app running at `pid`.
    /// Returns the result serialised as a string.
    ///
    /// **Context available depends on how the inspector was activated:**
    ///
    /// - `--remote-debugging-port=N` at launch → prefers "page" targets (full DOM,
    ///   `window`, `document`, renderer globals). Best for UI automation.
    /// - SIGUSR1 (no relaunch) → activates the Node.js main process inspector.
    ///   `process`, `Buffer`, `console` available; `require`, `document`, and
    ///   Electron APIs are NOT available in sandboxed apps (VS Code, Cursor).
    ///   Useful for process-level reads: `process.env`, `process.versions`,
    ///   `process.cwd()`, `process.pid`.
    public static func execute(javascript: String, pid: Int32) async throws -> String {
        guard isElectron(pid: pid) else {
            let name = NSWorkspace.shared.runningApplications
                .first(where: { $0.processIdentifier == pid })?.localizedName ?? "pid \(pid)"
            throw Error.notElectron(name)
        }

        // Prefer a "page" target (renderer with DOM) if the app was launched with
        // --remote-debugging-port. Page targets expose document/window/DOM APIs.
        if let pagePort = await pageTarget(pid: pid) {
            return try await cdpEvaluate(port: pagePort, javascript: javascript)
        }

        // If the inspector is already active (from a prior call in this session),
        // skip SIGUSR1 and reuse the port directly.
        if let existingPort = await activeInspectorPort(pid: pid) {
            return try await cdpEvaluate(port: existingPort, javascript: javascript)
        }

        // Snapshot TCP LISTEN ports before signalling so we can diff afterwards.
        let portsBefore = await listeningPorts(pid: pid)

        // Activate the V8 inspector — same mechanism as `node --inspect`.
        kill(pid, SIGUSR1)

        // Poll up to 2 s for a new LISTEN port to appear.
        var inspectorPort: Int?
        for _ in 0..<10 {
            try await Task.sleep(for: .milliseconds(200))
            let portsAfter = await listeningPorts(pid: pid)
            let newPorts = portsAfter.subtracting(portsBefore)
            for port in newPorts {
                if await isInspectorPort(port) {
                    inspectorPort = port
                    break
                }
            }
            if inspectorPort != nil { break }

            // Fallback: scan the common Node inspector range in case lsof
            // didn't resolve the pid association in time.
            if let port = await scanInspectorPorts() {
                inspectorPort = port
                break
            }
        }

        guard let port = inspectorPort else {
            throw Error.inspectorNotAvailable(
                "no CDP endpoint appeared on localhost within 2 s after SIGUSR1"
            )
        }

        return try await cdpEvaluate(port: port, javascript: javascript)
    }

    // MARK: - Port discovery

    /// Scan common remote-debugging ports for a CDP "page" target (renderer with
    /// DOM access). Returns the port if found. Apps launched with
    /// `--remote-debugging-port=N` expose page targets here; apps activated via
    /// SIGUSR1 only expose a "node" main-process target with no DOM.
    ///
    /// Only probes ports that `lsof` confirms are owned by `pid`, so JS is never
    /// executed in a different Electron/Chromium app that happens to be listening
    /// on the same well-known port. Falls back to all candidate ports when
    /// `lsof` returns empty (race window before the inspector has started).
    private static func pageTarget(pid: Int32) async -> Int? {
        let candidatePorts = [9222, 9223, 9224, 9225, 9230]
        let ownedPorts = await listeningPorts(pid: pid)
        // Only probe ports owned by this pid; fall back to all candidates when
        // lsof returns empty (inspector hasn't started yet).
        let portsToProbe = ownedPorts.isEmpty
            ? candidatePorts
            : candidatePorts.filter { ownedPorts.contains($0) }
        for port in portsToProbe {
            guard let url = URL(string: "http://127.0.0.1:\(port)/json") else { continue }
            var req = URLRequest(url: url); req.timeoutInterval = 0.3
            guard let (data, _) = try? await URLSession.shared.data(for: req),
                  let targets = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]],
                  targets.contains(where: { ($0["type"] as? String) == "page" })
            else { continue }
            return port
        }
        return nil
    }

    /// Check if the pid already has an active V8 inspector port.
    private static func activeInspectorPort(pid: Int32) async -> Int? {
        let ports = await listeningPorts(pid: pid)
        for port in ports {
            if await isInspectorPort(port) { return port }
        }
        return nil
    }

    /// TCP ports the pid is currently listening on, via `lsof -p <pid> -iTCP:LISTEN`.
    private static func listeningPorts(pid: Int32) async -> Set<Int> {
        let output = await runProcess(
            "/usr/sbin/lsof",
            args: ["-p", "\(pid)", "-iTCP", "-sTCP:LISTEN", "-Fn", "-P"]
        )
        var ports = Set<Int>()
        for line in output.split(separator: "\n") {
            // lsof -Fn lines: "n*:9229" or "n127.0.0.1:9229"
            guard line.hasPrefix("n"), let colon = line.lastIndex(of: ":") else { continue }
            let portStr = String(line[line.index(after: colon)...])
            if let port = Int(portStr) { ports.insert(port) }
        }
        return ports
    }

    /// Returns true if `port` responds to the CDP `/json` HTTP endpoint.
    private static func isInspectorPort(_ port: Int) async -> Bool {
        guard let url = URL(string: "http://127.0.0.1:\(port)/json") else { return false }
        var req = URLRequest(url: url)
        req.timeoutInterval = 0.5
        do {
            let (_, resp) = try await URLSession.shared.data(for: req)
            return (resp as? HTTPURLResponse)?.statusCode == 200
        } catch { return false }
    }

    /// Scan the standard Node inspector port range (9229-9249) for an active endpoint.
    private static func scanInspectorPorts() async -> Int? {
        for port in 9229...9249 {
            if await isInspectorPort(port) { return port }
        }
        return nil
    }

    // MARK: - CDP evaluation

    private static func cdpEvaluate(port: Int, javascript: String) async throws -> String {
        // 1. Fetch the list of debuggable targets.
        guard let jsonURL = URL(string: "http://127.0.0.1:\(port)/json") else {
            throw Error.cdpConnectionFailed("bad port \(port)")
        }
        var req = URLRequest(url: jsonURL)
        req.timeoutInterval = 5
        let (data, _) = try await URLSession.shared.data(for: req)

        guard let targets = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            throw Error.cdpConnectionFailed("could not parse /json response from port \(port)")
        }
        // Prefer the page target; fall back to the first target with a WS URL.
        let target = targets.first(where: { ($0["type"] as? String) == "page" })
            ?? targets.first(where: { $0["webSocketDebuggerUrl"] != nil })
        guard let wsURLStr = target?["webSocketDebuggerUrl"] as? String,
              let wsURL = URL(string: wsURLStr) else {
            throw Error.cdpConnectionFailed("no debuggable target found at port \(port)")
        }

        // 2. Send Runtime.evaluate over WebSocket.
        let payload: [String: Any] = [
            "id": 1,
            "method": "Runtime.evaluate",
            "params": [
                "expression": javascript,
                "returnByValue": true,
                "awaitPromise": true,
            ],
        ]
        let payloadStr = String(
            data: try JSONSerialization.data(withJSONObject: payload),
            encoding: .utf8
        )!

        return try await withCheckedThrowingContinuation { continuation in
            let ws = URLSession.shared.webSocketTask(with: wsURL)
            ws.resume()
            ws.send(.string(payloadStr)) { sendError in
                if let err = sendError {
                    ws.cancel()
                    continuation.resume(throwing: Error.cdpConnectionFailed(err.localizedDescription))
                    return
                }
                ws.receive { result in
                    ws.cancel()
                    switch result {
                    case .failure(let err):
                        continuation.resume(throwing: Error.cdpConnectionFailed(err.localizedDescription))
                    case .success(let message):
                        guard case .string(let str) = message else {
                            continuation.resume(throwing: Error.cdpConnectionFailed("binary WebSocket frame"))
                            return
                        }
                        continuation.resume(with: Result { try Self.parseCDPResult(str) })
                    }
                }
            }
        }
    }

    /// Extract the return value from a `Runtime.evaluate` CDP response.
    private static func parseCDPResult(_ json: String) throws -> String {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return json
        }
        if let error = obj["error"] as? [String: Any] {
            let msg = error["message"] as? String ?? json
            throw Error.evaluationFailed(msg)
        }
        if let result = obj["result"] as? [String: Any],
           let inner = result["result"] as? [String: Any] {
            // Exception thrown inside JS
            if let exDesc = (result["exceptionDetails"] as? [String: Any])?["text"] as? String {
                throw Error.evaluationFailed(exDesc)
            }
            // Serialise the return value
            if let value = inner["value"] {
                if let str = value as? String { return str }
                if let num = value as? NSNumber { return num.stringValue }
                if let data = try? JSONSerialization.data(withJSONObject: value),
                   let str = String(data: data, encoding: .utf8) { return str }
                return "\(value)"
            }
            return inner["description"] as? String ?? "undefined"
        }
        return json
    }

    // MARK: - Subprocess helper

    private static func runProcess(_ executable: String, args: [String]) async -> String {
        await withCheckedContinuation { continuation in
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: executable)
            proc.arguments = args
            let pipe = Pipe()
            proc.standardOutput = pipe
            proc.standardError = Pipe()
            proc.terminationHandler = { _ in
                let out = String(
                    data: pipe.fileHandleForReading.readDataToEndOfFile(),
                    encoding: .utf8
                ) ?? ""
                continuation.resume(returning: out)
            }
            do { try proc.run() } catch { continuation.resume(returning: "") }
        }
    }
}
