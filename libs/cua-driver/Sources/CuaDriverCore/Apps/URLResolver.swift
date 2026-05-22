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

    // Detect any URI scheme by looking for the first ':' that appears
    // before any '/' — this correctly handles both "://" schemes
    // (http, https, file) and single-colon schemes (about:, mailto:, etc.).
    if let colon = raw.firstIndex(of: ":") {
        let beforeColon = raw[raw.startIndex..<colon]
        // RFC 3986: scheme = ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
        let isValidScheme = !beforeColon.isEmpty &&
            beforeColon.range(of: "^[A-Za-z][A-Za-z0-9+.\\-]*$",
                              options: .regularExpression) != nil

        if isValidScheme {
            let scheme = beforeColon.lowercased()

            if scheme == "http" || scheme == "https" {
                // Percent-encode non-ASCII so URL(string:) doesn't return nil.
                if let encoded = raw.addingPercentEncoding(
                    withAllowedCharacters: .urlQueryAllowed),
                   let url = URL(string: encoded)
                {
                    return url
                }
                return URL(string: raw)
            }

            if scheme == "file", let schemeRange = raw.range(of: "://") {
                // Strip "file://" prefix, decode any existing percent-encoding,
                // then re-encode via fileURLWithPath for a clean round-trip.
                let pathPart = String(raw[schemeRange.upperBound...])
                let decoded = pathPart.removingPercentEncoding ?? pathPart
                return URL(fileURLWithPath: decoded).standardizedFileURL
            }

            // All other schemes (about:, mailto:, data:, etc.) — pass through
            // as-is via URL(string:). NSWorkspace handles about:blank natively
            // for browsers; other schemes are passed verbatim to LaunchServices.
            return URL(string: raw)
        }
    }

    // Plain path (absolute or tilde-prefixed).
    // URL(fileURLWithPath:) correctly percent-encodes non-ASCII characters
    // (CJK, Cyrillic, accented Latin, emoji, etc.).
    let expanded = (raw as NSString).expandingTildeInPath
    return URL(fileURLWithPath: expanded).standardizedFileURL
}
