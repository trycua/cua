import Foundation
import os

/// Shared Chrome DevTools Protocol (CDP) HTTP + WebSocket evaluator.
///
/// Extracted from `ElectronJS` so that `WebKitJS` (and future callers) can
/// reuse the same CDP plumbing without duplication.
public enum CDPClient {

    // MARK: - Errors

    public enum Error: Swift.Error, CustomStringConvertible {
        case connectionFailed(String)
        case evaluationFailed(String)

        public var description: String {
            switch self {
            case .connectionFailed(let d): return "CDP connection failed: \(d)"
            case .evaluationFailed(let d): return "CDP evaluation failed: \(d)"
            }
        }
    }

    // MARK: - Public API

    /// Returns `true` if the given port has a responsive CDP `/json` endpoint.
    public static func isAvailable(_ port: Int) async -> Bool {
        guard let url = URL(string: "http://127.0.0.1:\(port)/json") else { return false }
        var req = URLRequest(url: url)
        req.timeoutInterval = 0.5
        return (try? await URLSession.shared.data(for: req)) != nil
    }

    /// Scan `ports` for a CDP endpoint that exposes a `"page"` target (renderer
    /// with DOM access). Returns the first port that has one, or `nil`.
    public static func findPageTarget(ports: [Int]) async -> Int? {
        for port in ports {
            guard let url = URL(string: "http://127.0.0.1:\(port)/json") else { continue }
            var req = URLRequest(url: url)
            req.timeoutInterval = 0.5
            guard let (data, _) = try? await URLSession.shared.data(for: req),
                  let targets = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
            else { continue }
            if targets.contains(where: { ($0["type"] as? String) == "page" }) {
                return port
            }
        }
        return nil
    }

    /// Execute `javascript` via `Runtime.evaluate` on the CDP endpoint at `port`.
    /// Prefers a `"page"` target (renderer/DOM); falls back to the first target
    /// that exposes a `webSocketDebuggerUrl`.
    public static func evaluate(javascript: String, port: Int) async throws -> String {
        // 1. Fetch target list.
        guard let jsonURL = URL(string: "http://127.0.0.1:\(port)/json") else {
            throw Error.connectionFailed("bad port \(port)")
        }
        var req = URLRequest(url: jsonURL)
        req.timeoutInterval = 5
        let (data, _) = try await URLSession.shared.data(for: req)

        guard let targets = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            throw Error.connectionFailed("could not parse /json response from port \(port)")
        }
        let target = targets.first(where: { ($0["type"] as? String) == "page" })
            ?? targets.first(where: { $0["webSocketDebuggerUrl"] != nil })
        guard let wsURLStr = target?["webSocketDebuggerUrl"] as? String,
              let wsURL = URL(string: wsURLStr)
        else {
            throw Error.connectionFailed("no debuggable target found at port \(port)")
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

        let requestId = 1

        return try await withCheckedThrowingContinuation { continuation in
            let ws = URLSession.shared.webSocketTask(with: wsURL)
            ws.resume()

            // Single-resume guard: ensures the continuation is resumed exactly once
            // even if the timeout fires concurrently with a late-arriving frame.
            let once = OSAllocatedUnfairLock(initialState: false)
            let resumeOnce: @Sendable (() -> Void) -> Void = { action in
                let shouldRun = once.withLock { (fired: inout Bool) -> Bool in
                    guard !fired else { return false }
                    fired = true; return true
                }
                guard shouldRun else { return }
                action()
            }

            // 10-second timeout: close the socket and resume with an error if no
            // matching CDP response frame arrives in time.
            DispatchQueue.global().asyncAfter(deadline: .now() + 10) {
                resumeOnce {
                    ws.cancel()
                    continuation.resume(throwing: Error.connectionFailed("CDP response timed out after 10 seconds"))
                }
            }

            ws.send(.string(payloadStr)) { sendError in
                if let err = sendError {
                    resumeOnce {
                        ws.cancel()
                        continuation.resume(throwing: Error.connectionFailed(err.localizedDescription))
                    }
                    return
                }

                // Receive loop: discard CDP event frames (those with a "method" key)
                // and keep reading until a frame whose "id" matches requestId arrives.
                func receiveLoop() {
                    ws.receive { result in
                        switch result {
                        case .failure(let err):
                            resumeOnce {
                                ws.cancel()
                                continuation.resume(throwing: Error.connectionFailed(err.localizedDescription))
                            }
                        case .success(let message):
                            let text: String
                            switch message {
                            case .string(let s): text = s
                            case .data(let d): text = String(data: d, encoding: .utf8) ?? ""
                            @unknown default: receiveLoop(); return
                            }

                            guard let frameData = text.data(using: .utf8),
                                  let json = try? JSONSerialization.jsonObject(with: frameData) as? [String: Any]
                            else { receiveLoop(); return }

                            // Discard CDP events — they carry "method" but no "id".
                            if json["method"] != nil { receiveLoop(); return }

                            // Only act on the frame whose "id" matches our request.
                            guard let frameId = json["id"] as? Int, frameId == requestId else {
                                receiveLoop(); return
                            }

                            resumeOnce {
                                ws.cancel()
                                continuation.resume(with: Result { try Self.parseResult(text) })
                            }
                        }
                    }
                }
                receiveLoop()
            }
        }
    }

    // MARK: - Internals

    /// Extract the return value from a `Runtime.evaluate` CDP response.
    private static func parseResult(_ json: String) throws -> String {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return json }

        if let error = obj["error"] as? [String: Any] {
            let msg = error["message"] as? String ?? json
            throw Error.evaluationFailed(msg)
        }
        if let result = obj["result"] as? [String: Any],
           let inner = result["result"] as? [String: Any] {
            if let exDesc = (result["exceptionDetails"] as? [String: Any])?["text"] as? String {
                throw Error.evaluationFailed(exDesc)
            }
            if let value = inner["value"] {
                if let str = value as? String { return str }
                if let num = value as? NSNumber { return num.stringValue }
                if let d = try? JSONSerialization.data(withJSONObject: value),
                   let str = String(data: d, encoding: .utf8) { return str }
                return "\(value)"
            }
            return inner["description"] as? String ?? "undefined"
        }
        return json
    }
}
