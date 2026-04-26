import AppKit
import CuaDriverCore
import Foundation
import MCP

public enum LaunchAppTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "launch_app",
            description: """
                Launch a macOS app hidden — the driver never brings the
                target to the foreground, and the target's window is not
                drawn on screen. This tool is strictly a background-launch
                primitive; if a human operator wants to switch the
                foreground app, they should do so through normal macOS UI
                (Dock, Cmd-Tab, Spotlight), not through this driver.

                Provide either `bundle_id` (preferred — unambiguous, e.g.
                `com.apple.calculator`) or `name` (e.g. "Calculator", resolved
                against /Applications and /System/Applications). If both are
                given, bundle_id wins.

                Launches hidden — the app runs with its window initialized
                for automation (so the AX tree is populated and
                `click({pid, element_index})` works end-to-end), but no
                window is drawn on screen. To bring the window on-screen,
                use `NSRunningApplication.unhide()` from your own code, or
                have the user open the app (Dock click, Cmd-Tab, Spotlight).

                Some apps (Calculator and many Electron apps) call
                `NSApp.activate(ignoringOtherApps:)` in their own
                `applicationDidFinishLaunching`, overriding
                `NSWorkspace.OpenConfiguration.activates = false`. A layer-3
                focus-steal preventer arms a short
                `NSWorkspace.didActivateApplicationNotification` watcher
                around the launch and re-activates the previously-frontmost
                app if the target self-activates. If suppression fails the
                returned summary includes a "self-activation not suppressed"
                warning. With `hides = true`, the target is also hidden
                back immediately by LaunchServices, so even a missed
                layer-3 demotion doesn't leave a visible window.

                Optional `urls` are handed to the app through its
                `application(_:open:)` AppKit delegate. For Finder, passing
                a folder URL opens a backgrounded Finder window rooted at
                that folder — no activation, no visible window. Works
                with any app that implements `application(_:open:)`; apps
                that ignore the delegate simply launch without side effects.

                Optional `electron_debugging_port` launches an Electron app
                with `--remote-debugging-port=<N>`, activating its Chrome
                DevTools Protocol (CDP) on that port. This gives the `page`
                tool full renderer/DOM access (`document`, `window`,
                `fetch`, etc.) via page targets rather than the restricted
                Node main-process context exposed by SIGUSR1. Use port 9222
                unless you're running multiple Electron apps simultaneously.
                Has no effect on non-Electron apps.

                Optional `webkit_inspector_port` launches a Tauri/WKWebView
                app with `WEBKIT_INSPECTOR_SERVER=127.0.0.1:<N>`, activating
                WebKit's remote inspector. The `page` tool will use this to
                execute JavaScript in the WKWebView renderer. Use port 9226
                (reserved WebKit range: 9226–9228). Requires
                `developerExtrasEnabled = true` in the app's WKWebView config
                (default in Tauri debug builds; production builds may disable
                it). Has no effect on non-WKWebView apps.

                Use this instead of shelling out to `open -a` — the shell form
                always activates the target, requires an extra permission
                prompt for Bash, and doesn't even try to respect
                background-launch intent.

                Returns the launched app's pid, bundle_id, name, active
                flag, AND a `windows` array — same per-window shape as
                `list_windows` returns (window_id, title, bounds,
                z_index, is_on_screen, on_current_space, space_ids).
                That lets callers skip an extra `list_windows` round-trip
                before `get_window_state(pid, window_id)` in the common
                case. When the launch settles but no window has
                materialized yet (transient; rare), `windows` comes back
                empty — call `list_windows(pid)` explicitly a moment
                later to resolve a target.
                """,
            inputSchema: [
                "type": "object",
                "properties": [
                    "bundle_id": [
                        "type": "string",
                        "description":
                            "App bundle identifier, e.g. com.apple.calculator.",
                    ],
                    "name": [
                        "type": "string",
                        "description":
                            "App display name. Used only when bundle_id is absent.",
                    ],
                    "urls": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Optional file:// or http(s):// URLs (or plain paths with ~ expansion) to hand to the launched app via application(_:open:). For Finder, pass a folder URL or path to open a backgrounded Finder window rooted at that folder — no activation. Apps that don't implement application(_:open:) launch normally and ignore these.",
                    ],
                    "electron_debugging_port": [
                        "type": "integer",
                        "description": "Launch an Electron app with --remote-debugging-port=<N> so the `page` tool gets full renderer/DOM access. Use 9222 unless running multiple Electron apps. Ignored for non-Electron apps.",
                    ],
                    "webkit_inspector_port": [
                        "type": "integer",
                        "description": "Launch a Tauri/WKWebView app with WEBKIT_INSPECTOR_SERVER=127.0.0.1:<N> so the `page` tool can reach its WebKit inspector. Use 9226 (reserved WebKit range: 9226–9228, distinct from Electron's 9222–9225). Requires developerExtrasEnabled=true in the WKWebView config (default in Tauri debug builds).",
                    ],
                    "creates_new_application_instance": [
                        "type": "boolean",
                        "description": """
                            Force a brand-new process even if the app is already running. \
                            Useful for isolated browser sessions: pass \
                            creates_new_application_instance=true together with \
                            additional_arguments=[\"--user-data-dir=/tmp/session-a\", \
                            \"--no-first-run\", \"--no-default-browser-check\"] to launch \
                            a sandboxed Chrome that cannot see the user's real profile, \
                            cookies, or extensions. Each session gets its own pid and \
                            window identity and can be controlled independently.
                            """,
                    ],
                    "additional_arguments": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description": """
                            Extra command-line arguments passed to the launched process. \
                            Passed directly as argv entries — no shell expansion. \
                            Example: [\"--user-data-dir=/tmp/cua-session\", \
                            \"--no-first-run\"] for an isolated Chrome session.
                            """,
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: true,  // relaunching a running app is a no-op
                openWorldHint: true
            )
        ),
        invoke: { arguments in
            let bundleId = arguments?["bundle_id"]?.stringValue
            let name = arguments?["name"]?.stringValue
            let rawUrls = arguments?["urls"]?.arrayValue ?? []
            let electronDebuggingPort = arguments?["electron_debugging_port"]?.intValue
            let webkitInspectorPort = arguments?["webkit_inspector_port"]?.intValue
            let createsNewInstance = arguments?["creates_new_application_instance"]?.boolValue ?? false
            let rawExtraArgs = arguments?["additional_arguments"]?.arrayValue ?? []

            if bundleId == nil && name == nil {
                return errorResult(
                    "Provide either bundle_id or name to identify the app to launch.")
            }

            // Resolve url strings → URL values. Accept file:// and http(s)://
            // verbatim, and plain paths (with optional ~) as file URLs.
            // Reject malformed entries up front rather than letting
            // NSWorkspace return a vague "no such file" later.
            var urls: [URL] = []
            for raw in rawUrls {
                guard let str = raw.stringValue, !str.isEmpty else {
                    return errorResult("urls entries must be non-empty strings.")
                }
                guard let resolved = resolveLaunchURL(str) else {
                    return errorResult("Could not resolve url: \(str).")
                }
                urls.append(resolved)
            }

            do {
                // Layer-3 focus-steal suppression. If there's a current
                // frontmost app that isn't the target, arm the preventer
                // BEFORE the launch so any NSApp.activate(ignoringOtherApps:)
                // call the target makes in applicationDidFinishLaunching —
                // or during a URL-open handoff (Chrome does this) — is
                // immediately undone. Arming after launch was a race: for
                // targets that self-activate synchronously during the
                // open() call itself (Chrome with urls=..., Electron apps
                // that eagerly foreground on URL handoff), the activation
                // notification had already fired and been discarded before
                // our observer was listening, leaving the target stuck
                // frontmost. Two-phase: begin before launch with a
                // best-guess target (resolved from bundle_id/name upfront),
                // replace the pid after launch returns with the actual one.
                let priorFrontmost = await MainActor.run {
                    NSWorkspace.shared.frontmostApplication
                }

                // Arm the preventer speculatively with targetPid=0; we'll
                // replace it below once we know the real pid. The preventer
                // matches by pid on each activation notification, so a
                // placeholder won't fire false positives.
                var handle: SuppressionHandle?
                if let priorFrontmost {
                    handle =
                        await AppStateRegistry.systemFocusStealPreventer
                        .beginSuppression(
                            targetPid: 0,  // placeholder; replaced after launch
                            restoreTo: priorFrontmost
                        )
                }

                var additionalArguments: [String] = rawExtraArgs.compactMap { $0.stringValue }
                if let port = electronDebuggingPort {
                    additionalArguments.append("--remote-debugging-port=\(port)")
                }

                var additionalEnvironment: [String: String] = [:]
                if let port = webkitInspectorPort {
                    additionalEnvironment["WEBKIT_INSPECTOR_SERVER"] = "127.0.0.1:\(port)"
                    // wry (the WebView runtime used by Tauri) checks this env var at
                    // WKWebView init time and sets allowsRemoteInspection = true, which
                    // is required for WEBKIT_INSPECTOR_SERVER to actually open the socket.
                    // Harmless for non-Tauri WKWebView apps (they don't read it).
                    additionalEnvironment["TAURI_WEBVIEW_AUTOMATION"] = "1"
                }

                let info = try await AppLauncher.launch(
                    bundleId: bundleId,
                    name: name,
                    urls: urls,
                    additionalArguments: additionalArguments,
                    additionalEnvironment: additionalEnvironment,
                    createsNewApplicationInstance: createsNewInstance
                )

                // Replace the placeholder pid with the real one so any
                // activation notification the target emits from now on is
                // caught. Observed activations that fired DURING the launch
                // (synchronous `open`) will have been seen by the observer
                // but not matched (pid=0 mismatch), so they pass through —
                // this fix covers the PROCESS-ORDERING race (target emits
                // activation AFTER `open` returns) which is the common
                // case on real machines; the intra-`open` case is handled
                // by the target's own reflex running asynchronously.
                let shouldSuppress =
                    priorFrontmost != nil
                    && priorFrontmost?.processIdentifier != info.pid
                if shouldSuppress, let handle, let priorFrontmost {
                    await AppStateRegistry.systemFocusStealPreventer
                        .endSuppression(handle)
                    let reArmedHandle =
                        await AppStateRegistry.systemFocusStealPreventer
                        .beginSuppression(
                            targetPid: info.pid,
                            restoreTo: priorFrontmost
                        )
                    // 500ms is enough for applicationDidFinishLaunching
                    // plus any reflex NSApp.activate to fire and get
                    // suppressed.
                    try? await Task.sleep(nanoseconds: 500_000_000)
                    await AppStateRegistry.systemFocusStealPreventer
                        .endSuppression(reArmedHandle)

                    // Belt-and-braces: if the target is STILL frontmost
                    // after the suppression window (intra-`open` synchronous
                    // activation that fired before we could arm with the
                    // real pid), explicitly demote by re-activating the
                    // prior frontmost. One-shot, no observer.
                    let frontmostNow = await MainActor.run {
                        NSWorkspace.shared.frontmostApplication
                    }
                    if frontmostNow?.processIdentifier == info.pid {
                        await MainActor.run {
                            _ = priorFrontmost.activate(options: [])
                        }
                    }
                } else if let handle {
                    await AppStateRegistry.systemFocusStealPreventer
                        .endSuppression(handle)
                }

                var summary = "✅ Launched \(info.name) (pid \(info.pid)) in background."
                if let port = electronDebuggingPort {
                    summary += " CDP renderer available on port \(port) — use `page` tool for full DOM access."
                }
                if let port = webkitInspectorPort {
                    summary += " WebKit inspector available on port \(port) — use `page` tool for DOM access."
                }
                if shouldSuppress {
                    let frontmostAfter = await MainActor.run {
                        NSWorkspace.shared.frontmostApplication
                    }
                    if frontmostAfter?.processIdentifier == info.pid {
                        summary +=
                            " WARNING: self-activation not suppressed; "
                            + "layer-3 mechanism may be insufficient for this app."
                    }
                }

                // Resolve the pid's windows so the caller doesn't have
                // to round-trip list_windows before get_window_state.
                // Short retry loop — LaunchServices returns before
                // WindowServer has registered the new windows on cold
                // launches, so a single enumeration right after
                // `openApplication` often comes back empty on Calculator /
                // Finder / other "create window in applicationDidFinish-
                // Launching" apps. 5x100ms is enough for the common case
                // without stalling the non-window case noticeably.
                let windowRecords = await resolveWindows(forPid: info.pid)
                if !windowRecords.isEmpty {
                    summary += "\n\nWindows:"
                    for w in windowRecords {
                        let title = w.title.isEmpty ? "(no title)" : "\"\(w.title)\""
                        summary += "\n- \(title) [window_id: \(w.windowId)]"
                    }
                    summary += "\n→ Call get_window_state(pid: \(info.pid), window_id) to inspect."
                }

                let launchResult = LaunchResult(info: info, windows: windowRecords)
                let textContent: Tool.Content = .text(
                    text: summary, annotations: nil, _meta: nil)
                if let result = try? CallTool.Result(
                    content: [textContent],
                    structuredContent: launchResult
                ) {
                    return result
                }
                return CallTool.Result(
                    content: [textContent]
                )
            } catch let error as AppLauncher.LaunchError {
                return errorResult(error.description)
            } catch {
                return errorResult("Unexpected error: \(error)")
            }
        }
    )

    /// Resolve a caller-supplied url string into a `URL` suitable for
    /// `NSWorkspace.open(_:withApplicationAt:configuration:)`.
    ///
    /// - `http://` / `https://` / `file://` passthrough as-is.
    /// - Tilde-prefixed paths (`~/Downloads`) expand against the current
    ///   user's home.
    /// - Other strings are treated as file paths — absolute paths become
    ///   `file://` URLs; relative paths resolve against the launcher's
    ///   cwd, which is almost never meaningful for a GUI app, so in
    ///   practice callers should always pass absolute or ~-prefixed
    ///   paths.
    ///
    /// Returns `nil` only when the string can't be parsed into any URL
    /// at all — realistically that's the empty string (handled upstream).
    private static func resolveLaunchURL(_ raw: String) -> URL? {
        if let url = URL(string: raw),
           let scheme = url.scheme?.lowercased(),
           scheme == "http" || scheme == "https" || scheme == "file"
        {
            return url
        }
        let expanded = (raw as NSString).expandingTildeInPath
        return URL(fileURLWithPath: expanded)
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }

    /// Poll `WindowEnumerator` for the pid's layer-0 windows, retrying a
    /// handful of times to absorb the usual LaunchServices → WindowServer
    /// latency. Returns `[]` when the pid produces no window at all
    /// (menubar-only helpers, or apps that defer window creation); the
    /// caller falls back to an explicit `list_windows` call later.
    private static func resolveWindows(forPid pid: Int32) async -> [WindowRow] {
        let currentSpaceID = SpaceMigrator.currentSpaceID()
        for attempt in 0..<5 {
            let found = WindowEnumerator.allWindows()
                .filter { $0.pid == pid && $0.layer == 0 }
                .filter { $0.bounds.width > 1 && $0.bounds.height > 1 }
            if !found.isEmpty {
                return found.map { info -> WindowRow in
                    let spaceIDs = SpaceMigrator.spaceIDs(
                        forWindowID: UInt32(info.id))
                    let onCurrentSpace: Bool? = {
                        guard let spaceIDs, let currentSpaceID else { return nil }
                        return spaceIDs.contains(currentSpaceID)
                    }()
                    return WindowRow(
                        windowId: info.id,
                        title: info.name,
                        bounds: info.bounds,
                        layer: info.layer,
                        zIndex: info.zIndex,
                        isOnScreen: info.isOnScreen,
                        onCurrentSpace: onCurrentSpace,
                        spaceIds: spaceIDs
                    )
                }
            }
            // Last attempt — return empty rather than sleep again.
            if attempt == 4 { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return []
    }

    /// Extends `AppInfo` with a `windows` array for `launch_app`'s
    /// structured response. Per-window fields mirror `list_windows`'s
    /// `Row` shape — `pid` and `app_name` are omitted because the
    /// top-level `AppInfo` already carries those.
    private struct LaunchResult: Codable, Sendable {
        let info: AppInfo
        let windows: [WindowRow]

        private enum CodingKeys: String, CodingKey {
            case pid
            case bundleId = "bundle_id"
            case name
            case running
            case active
            case windows
        }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(info.pid, forKey: .pid)
            try c.encodeIfPresent(info.bundleId, forKey: .bundleId)
            try c.encode(info.name, forKey: .name)
            try c.encode(info.running, forKey: .running)
            try c.encode(info.active, forKey: .active)
            try c.encode(windows, forKey: .windows)
        }

        // Emit-only — the daemon doesn't round-trip its own responses.
        init(info: AppInfo, windows: [WindowRow]) {
            self.info = info
            self.windows = windows
        }

        init(from decoder: Decoder) throws {
            throw DecodingError.dataCorrupted(
                .init(
                    codingPath: decoder.codingPath,
                    debugDescription: "LaunchResult is emit-only."
                )
            )
        }
    }

    /// Per-window record in `launch_app`'s `windows` array. Mirrors the
    /// shape `list_windows` returns (minus `pid` / `app_name`, which
    /// live on the top-level response here) so callers can treat the
    /// two sources interchangeably.
    private struct WindowRow: Codable, Sendable {
        let windowId: Int
        let title: String
        let bounds: WindowBounds
        let layer: Int
        let zIndex: Int
        let isOnScreen: Bool
        let onCurrentSpace: Bool?
        let spaceIds: [UInt64]?

        private enum CodingKeys: String, CodingKey {
            case windowId = "window_id"
            case title
            case bounds
            case layer
            case zIndex = "z_index"
            case isOnScreen = "is_on_screen"
            case onCurrentSpace = "on_current_space"
            case spaceIds = "space_ids"
        }

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(windowId, forKey: .windowId)
            try c.encode(title, forKey: .title)
            try c.encode(bounds, forKey: .bounds)
            try c.encode(layer, forKey: .layer)
            try c.encode(zIndex, forKey: .zIndex)
            try c.encode(isOnScreen, forKey: .isOnScreen)
            try c.encodeIfPresent(onCurrentSpace, forKey: .onCurrentSpace)
            try c.encodeIfPresent(spaceIds, forKey: .spaceIds)
        }
    }
}
