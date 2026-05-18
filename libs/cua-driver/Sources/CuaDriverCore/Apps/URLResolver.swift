import Foundation

/// Resolves a caller-supplied string into a `URL` suitable for
/// `NSWorkspace.open(_:withApplicationAt:configuration:)`.
///
/// This is extracted from `LaunchAppTool` so it can be unit-tested
/// without spinning up the full MCP server stack.
///
/// ## Handling rules
///
/// - `http://` / `https://` — percent-encode non-ASCII before parsing.
/// - `file://` — strip scheme, decode percent-encoding, rebuild via
///   `URL(fileURLWithPath:)` so LaunchServices gets a clean path.
/// - Plain paths (absolute or `~`-prefixed) — expand tilde, then build
///   a `file://` URL via `URL(fileURLWithPath:)`.
///
/// ## Why `URL(fileURLWithPath:)` instead of `URL(string:)`
///
/// `URL(string:)` treats its argument as an already-encoded RFC 3986
/// string and returns `nil` for bare non-ASCII characters (e.g. CJK
/// paths like `/Users/me/器材控/file.key`).  `URL(fileURLWithPath:)`
/// accepts a raw filesystem path and applies the necessary
/// percent-encoding internally, producing a well-formed `file://` URL
/// that LaunchServices can open without crashing the daemon.
///
/// Fixes: https://github.com/trycua/cua/issues/1519
public func resolveLaunchURL(_ raw: String) -> URL? {
    guard !raw.isEmpty else { return nil }

    // Detect explicit URL schemes (http / https / file).
    if let schemeRange = raw.range(of: "://") {
        let scheme = raw[raw.startIndex..<schemeRange.lowerBound].lowercased()

        if scheme == "http" || scheme == "https" {
            // Percent-encode non-ASCII so URL(string:) doesn't return nil.
            if let encoded = raw.addingPercentEncoding(
                withAllowedCharacters: .urlQueryAllowed),
               let url = URL(string: encoded)
            {
                return url
            }
            // Already encoded by caller — try as-is.
            return URL(string: raw)
        }

        if scheme == "file" {
            // Strip "file://" prefix, decode any existing percent-encoding,
            // then re-encode via fileURLWithPath for a clean round-trip.
            let pathPart = String(raw[schemeRange.upperBound...])
            let decoded = pathPart.removingPercentEncoding ?? pathPart
            return URL(fileURLWithPath: decoded).standardizedFileURL
        }
    }

    // Plain path (absolute or tilde-prefixed).
    // URL(fileURLWithPath:) correctly percent-encodes non-ASCII characters
    // (CJK, Cyrillic, accented Latin, emoji, etc.).
    let expanded = (raw as NSString).expandingTildeInPath
    return URL(fileURLWithPath: expanded).standardizedFileURL
}
