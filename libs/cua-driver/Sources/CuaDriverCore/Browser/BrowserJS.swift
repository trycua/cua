import CoreGraphics
import Foundation
import os

/// Execute JavaScript in a browser tab via Apple Events.
///
/// Supports the Chromium family (Chrome, Brave, Edge) which share the same
/// `execute javascript` AppleScript verb, and Safari which uses `do JavaScript`.
/// Firefox and Arc are not supported — see WEB_APPS.md for the full matrix.
///
/// **Prerequisite:** "Allow JavaScript from Apple Events" must be enabled in
/// the target browser. See WEB_APPS.md → "Enable Allow JavaScript from Apple
/// Events" for the programmatic setup path.
public enum BrowserJS {

    // MARK: - Public API

    public enum Error: Swift.Error, CustomStringConvertible {
        case unsupportedBrowser(String)
        case windowNotFound(UInt32)
        case javascriptNotEnabled(String)
        case executionFailed(String)

        public var description: String {
            switch self {
            case .unsupportedBrowser(let id):
                return "Browser '\(id)' does not support JavaScript via Apple Events. "
                    + "Supported: Chrome (com.google.Chrome), Brave (com.brave.Browser), "
                    + "Edge (com.microsoft.edgemac), Safari (com.apple.Safari)."
            case .windowNotFound(let wid):
                return "Could not find a browser window matching window_id \(wid). "
                    + "Ensure the browser is running and the window is on the current Space."
            case .javascriptNotEnabled(let appName):
                return "'\(appName)' rejected the JavaScript execution — "
                    + "'Allow JavaScript from Apple Events' is not enabled. "
                    + "See WEB_APPS.md → 'Enable Allow JavaScript from Apple Events' "
                    + "for the programmatic setup path (quit browser → edit Preferences JSON → relaunch)."
            case .executionFailed(let detail):
                return "JavaScript execution failed: \(detail)"
            }
        }
    }

    /// Run `javascript` in the active tab of the browser window identified by
    /// `windowId` (CGWindowID) and return the result as a string.
    public static func execute(
        javascript: String,
        bundleId: String,
        windowId: UInt32
    ) async throws -> String {
        guard let spec = browserSpec(for: bundleId) else {
            throw Error.unsupportedBrowser(bundleId)
        }

        // Find the CGWindow entry for this window_id to get its title,
        // which we use to locate the matching AppleScript window.
        guard let windowTitle = cgWindowTitle(for: windowId) else {
            throw Error.windowNotFound(windowId)
        }

        let script = spec.buildScript(javascript, windowTitle)
        return try await runAppleScript(script, appName: spec.appName)
    }

    /// True when this bundle ID is handled via Apple Events (Chrome/Brave/Edge/Safari).
    public static func supports(bundleId: String) -> Bool {
        browserSpec(for: bundleId) != nil
    }

    // MARK: - Browser specs

    private struct BrowserSpec {
        let appName: String
        let buildScript: (_ javascript: String, _ windowTitle: String) -> String
    }

    private static func browserSpec(for bundleId: String) -> BrowserSpec? {
        switch bundleId {
        case "com.google.Chrome":
            return chromiumSpec(appName: "Google Chrome")
        case "com.brave.Browser":
            return chromiumSpec(appName: "Brave Browser")
        case "com.microsoft.edgemac":
            return chromiumSpec(appName: "Microsoft Edge")
        case "com.apple.Safari":
            return safariSpec()
        default:
            return nil
        }
    }

    private static func chromiumSpec(appName: String) -> BrowserSpec {
        BrowserSpec(appName: appName) { javascript, windowTitle in
            // Escape the window title for AppleScript string comparison.
            // We use `whose name contains` to handle truncated titles.
            let escapedTitle = windowTitle
                .replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "\"", with: "\\\"")
            return """
            tell application "\(appName)"
              set matchedWindow to missing value
              repeat with w in windows
                if name of w contains "\(escapedTitle)" then
                  set matchedWindow to w
                  exit repeat
                end if
              end repeat
              if matchedWindow is missing value then
                set matchedWindow to front window
              end if
              tell active tab of matchedWindow
                execute javascript \(appleScriptString(javascript))
              end tell
            end tell
            """
        }
    }

    private static func safariSpec() -> BrowserSpec {
        BrowserSpec(appName: "Safari") { javascript, windowTitle in
            let escapedTitle = windowTitle
                .replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "\"", with: "\\\"")
            return """
            tell application "Safari"
              set matchedDoc to missing value
              repeat with d in documents
                if name of d contains "\(escapedTitle)" then
                  set matchedDoc to d
                  exit repeat
                end if
              end repeat
              if matchedDoc is missing value then
                set matchedDoc to document 1
              end if
              do JavaScript \(appleScriptString(javascript)) in matchedDoc
            end tell
            """
        }
    }

    // MARK: - AppleScript runner

    private static func runAppleScript(
        _ script: String,
        appName: String
    ) async throws -> String {
        // Write to a temp file to avoid shell-quoting the script entirely.
        let tmpURL = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent(UUID().uuidString + ".applescript")
        try script.write(to: tmpURL, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: tmpURL) }

        return try await withCheckedThrowingContinuation { continuation in
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
            proc.arguments = [tmpURL.path]
            let stdout = Pipe()
            let stderr = Pipe()
            proc.standardOutput = stdout
            proc.standardError = stderr

            let once = OSAllocatedUnfairLock(initialState: false)
            let resumeOnce: @Sendable (Result<String, Swift.Error>) -> Void = { result in
                let shouldResume = once.withLock { (fired: inout Bool) -> Bool in
                    guard !fired else { return false }
                    fired = true; return true
                }
                guard shouldResume else { return }
                switch result {
                case .success(let s): continuation.resume(returning: s)
                case .failure(let e): continuation.resume(throwing: e)
                }
            }

            let timeoutItem = DispatchWorkItem {
                proc.terminate()
                resumeOnce(.failure(Error.executionFailed(
                    "osascript timed out after 15 s — browser may be showing a permission dialog")))
            }
            DispatchQueue.global().asyncAfter(deadline: .now() + 15, execute: timeoutItem)

            proc.terminationHandler = { p in
                timeoutItem.cancel()
                let out = String(
                    data: stdout.fileHandleForReading.readDataToEndOfFile(),
                    encoding: .utf8
                )?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                let err = String(
                    data: stderr.fileHandleForReading.readDataToEndOfFile(),
                    encoding: .utf8
                )?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                if p.terminationStatus == 0 {
                    resumeOnce(.success(out))
                } else if err.contains("turned off") || err.contains("AppleScript is turned off") {
                    resumeOnce(.failure(Error.javascriptNotEnabled(appName)))
                } else {
                    resumeOnce(.failure(Error.executionFailed(err.isEmpty ? out : err)))
                }
            }
            do {
                try proc.run()
            } catch {
                timeoutItem.cancel()
                resumeOnce(.failure(error))
            }
        }
    }

    // MARK: - CGWindow helpers

    private static func cgWindowTitle(for windowId: UInt32) -> String? {
        // Enumerate all on-screen windows and find the one with matching ID.
        // CGWindowListCopyWindowInfo([.optionIncludingWindow], id) is unreliable
        // for windows not yet in the compositing list — enumerating all is safer.
        //
        // Return "" (empty string) when the window exists but has no title
        // (new tab, about:blank, freshly-opened window). The AppleScript
        // `name contains ""` expression matches every string, so an empty
        // title falls through to the `front window` fallback — correct behavior.
        // Only return nil when the window ID is genuinely not in any list.
        let list = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
        ) as? [[String: Any]] ?? []
        for entry in list {
            guard let id = entry[kCGWindowNumber as String] as? Int,
                  UInt32(id) == windowId else { continue }
            return entry[kCGWindowName as String] as? String ?? ""
        }
        // Fall back to off-screen windows (minimized, hidden).
        let all = CGWindowListCopyWindowInfo([], kCGNullWindowID) as? [[String: Any]] ?? []
        for entry in all {
            guard let id = entry[kCGWindowNumber as String] as? Int,
                  UInt32(id) == windowId else { continue }
            return entry[kCGWindowName as String] as? String ?? ""
        }
        return nil
    }

    // MARK: - AppleScript string literal builder

    /// Wraps `value` in a multi-line AppleScript string literal using
    /// quoted form syntax to avoid any quoting/escaping issues with the
    /// JavaScript payload.
    private static func appleScriptString(_ value: String) -> String {
        // AppleScript doesn't support heredocs; we use string concatenation
        // for newlines and escape only the characters AppleScript requires.
        if !value.contains("\n") && !value.contains("\"") && !value.contains("\\") {
            return "\"\(value)\""
        }
        // Build the string by concatenating segments split on newlines,
        // joined with (ASCII character 10) for newlines.
        let lines = value.split(separator: "\n", omittingEmptySubsequences: false)
        let escaped = lines.map { line -> String in
            let s = String(line)
                .replacingOccurrences(of: "\\", with: "\\\\")
                .replacingOccurrences(of: "\"", with: "\\\"")
            return "\"\(s)\""
        }
        if escaped.count == 1 { return escaped[0] }
        return escaped.joined(separator: " & (ASCII character 10) & ")
    }
}
