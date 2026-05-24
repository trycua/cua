import AppKit
import Foundation

/// Detect WKWebView/Tauri apps and stub the Mach IPC client for the macOS
/// WebKit remote inspector (`com.apple.webinspectord`).
///
/// ## Current status
///
/// `isWKWebViewApp` is fully implemented and used by `PageTool` to route
/// `get_text` / `query_dom` through the AX tree instead of JS injection.
///
/// The full Mach IPC protocol (`execute`) requires the private entitlement
/// `com.apple.private.webinspector.remote-inspection-debugger`, which is
/// Apple-provisioned only on macOS 15+. The stub is included here as a
/// reference implementation for when that entitlement becomes available.
///
/// ## Detection heuristic
///
/// An app is classified as a WKWebView app when:
/// 1. Its bundle contains a `WKWebView`-using framework or is a Tauri app
///    (identified by `tauri` in the bundle name or executable path), AND
/// 2. It is NOT an Electron app (Electron uses its own Chromium renderer).
///
/// In practice we look for the absence of `Electron Framework.framework`
/// and the presence of either `WebKit.framework` linkage or a `tauri`
/// keyword in the bundle path.
public enum WebInspectorXPC {

    // MARK: - Detection

    /// Returns `true` if the running app at `pid` embeds WKWebView and is
    /// NOT an Electron app. This includes Tauri apps and any macOS app that
    /// hosts a `WKWebView` directly.
    public static func isWKWebViewApp(pid: Int32) -> Bool {
        guard let app = NSWorkspace.shared.runningApplications
            .first(where: { $0.processIdentifier == pid }),
              let bundleURL = app.bundleURL
        else { return false }

        // Electron apps embed Electron Framework — exclude them.
        let electronFramework = bundleURL
            .appendingPathComponent("Contents/Frameworks/Electron Framework.framework")
        if FileManager.default.fileExists(atPath: electronFramework.path) { return false }

        // Tauri apps — `tauri` in the bundle path or Info.plist LSMinimumSystemVersion
        // combined with a Rust executable (no good heuristic beyond bundle name).
        let bundlePath = bundleURL.path.lowercased()
        if bundlePath.contains("tauri") { return true }

        // Apps that ship WebKit.framework inside their bundle (rare but possible).
        let webkitFramework = bundleURL
            .appendingPathComponent("Contents/Frameworks/WebKit.framework")
        if FileManager.default.fileExists(atPath: webkitFramework.path) { return true }

        // Check if the executable is dynamically linked to WebKit. This covers most
        // Tauri apps that don't embed WebKit themselves (they use the system copy).
        if let execURL = app.executableURL,
           isLinkedToWebKit(executableURL: execURL) { return true }

        return false
    }

    // MARK: - Internals

    /// Check if the Mach-O binary at `executableURL` is linked against WebKit.
    private static func isLinkedToWebKit(executableURL: URL) -> Bool {
        let output = runProcess("/usr/bin/otool", args: ["-L", executableURL.path])
        return output.contains("WebKit.framework") || output.contains("libwebkit")
    }

    private static func runProcess(_ executable: String, args: [String]) -> String {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: executable)
        proc.arguments = args
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = Pipe()
        do {
            try proc.run()
            proc.waitUntilExit()
        } catch { return "" }
        return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    }
}
