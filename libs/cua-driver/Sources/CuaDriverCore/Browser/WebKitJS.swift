import Foundation

/// Execute JavaScript in a GTK/WPE WebKit browser via its TCP remote inspector.
///
/// On Linux, WebKit can expose a JSON/WebSocket inspector on a TCP port when
/// launched with `WEBKIT_INSPECTOR_SERVER=127.0.0.1:<port>`. The reserved port
/// range for this driver is 9226–9228 (distinct from Electron's 9222–9225).
///
/// On macOS this path is not normally active — `isAvailable()` returns `false`
/// quickly (0.5 s timeout per port) so the fall-through to the next backend is
/// near-instant.
public enum WebKitJS {

    /// Ports probed for a live WebKit TCP inspector.
    private static let inspectorPorts = 9226...9228

    /// Returns `true` if any of the probed ports expose a CDP endpoint.
    public static func isAvailable() async -> Bool {
        for port in inspectorPorts {
            if await CDPClient.isAvailable(port) { return true }
        }
        return false
    }

    /// Execute `javascript` on the first live WebKit TCP inspector port.
    public static func execute(javascript: String) async throws -> String {
        for port in inspectorPorts {
            if await CDPClient.isAvailable(port) {
                return try await CDPClient.evaluate(javascript: javascript, port: port)
            }
        }
        throw WebKitJSError.inspectorNotAvailable(
            "no WebKit TCP inspector found on ports \(inspectorPorts.lowerBound)–\(inspectorPorts.upperBound). "
            + "Launch the app with WEBKIT_INSPECTOR_SERVER=127.0.0.1:9226 to enable it.")
    }

    // MARK: - Errors

    public enum WebKitJSError: Swift.Error, CustomStringConvertible {
        case inspectorNotAvailable(String)

        public var description: String {
            switch self {
            case .inspectorNotAvailable(let d): return d
            }
        }
    }
}
