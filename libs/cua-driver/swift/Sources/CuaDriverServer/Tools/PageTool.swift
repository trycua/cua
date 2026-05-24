import AppKit
import CuaDriverCore
import Foundation
import MCP

/// Browser page primitives — execute JavaScript, extract page text, query DOM.
///
/// Use `get_window_state` with its `javascript` param for **read-only** queries
/// co-located with a snapshot. Use `page` when you need a standalone JS call
/// (mutations, navigation side-effects, or results without a full AX walk).
///
/// Requires "Allow JavaScript from Apple Events" to be enabled in the target
/// browser — see WEB_APPS.md for the setup path.
public enum PageTool {

    public static let handler = ToolHandler(
        tool: Tool(
            name: "page",
            description: """
                Browser page primitives — execute JavaScript, extract page \
                text, or query DOM elements via CSS selector.

                Three actions:

                • `execute_javascript` — run arbitrary JS in the active tab \
                and return the result. Wrap in an IIFE with try-catch for \
                safety. Don't use this for elements already indexed by \
                `get_window_state` — prefer `click` / `set_value` there.

                • `get_text` — returns `document.body.innerText`. Faster than \
                walking the AX tree for plain-text content (prices, article \
                body, table values) that the AX tree drops or truncates.

                • `query_dom` — runs `querySelectorAll(css_selector)` and \
                returns each matching element's tag, innerText, and any \
                requested attributes as a JSON array. Useful for structured \
                data (table rows, link hrefs, data-* attributes) that the AX \
                tree flattens.

                Supported browsers: Chrome (com.google.Chrome), Brave \
                (com.brave.Browser), Edge (com.microsoft.edgemac), Safari \
                (com.apple.Safari). Requires "Allow JavaScript from Apple \
                Events" enabled — see WEB_APPS.md.

                `pid` and `window_id` identify the target window. The browser \
                does NOT need to be frontmost for read actions. For mutations \
                that trigger UI changes (form submits, navigation) the browser \
                should be frontmost so the results are visible.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "window_id", "action"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Browser process ID.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description": "CGWindowID of the target browser window.",
                    ],
                    "action": [
                        "type": "string",
                        "enum": ["execute_javascript", "get_text", "query_dom",
                                 "enable_javascript_apple_events"],
                        "description": """
                            Which page primitive to run.

                            • execute_javascript / get_text / query_dom — see above.
                            • enable_javascript_apple_events — enable 'Allow JavaScript \
                            from Apple Events' in Chrome/Brave/Edge. Quits the browser, \
                            patches each profile's Preferences JSON, then relaunches. \
                            Requires user_has_confirmed_enabling=true — you MUST ask \
                            the user for explicit permission before calling this action.
                            """,
                    ],
                    "javascript": [
                        "type": "string",
                        "description": "JS to execute (action=execute_javascript only). Should return a serialisable value. Wrap in IIFE: `(() => { … })()`.",
                    ],
                    "css_selector": [
                        "type": "string",
                        "description": "CSS selector (action=query_dom only). Passed to querySelectorAll — standard CSS syntax.",
                    ],
                    "attributes": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description": "Element attributes to include per match (action=query_dom only). E.g. [\"href\", \"src\", \"data-id\"]. tag and innerText are always included.",
                    ],
                    "bundle_id": [
                        "type": "string",
                        "description": "Browser bundle ID (action=enable_javascript_apple_events only). E.g. com.google.Chrome.",
                    ],
                    "user_has_confirmed_enabling": [
                        "type": "boolean",
                        "description": "Must be true (action=enable_javascript_apple_events only). Set this only after the user has explicitly said yes to enabling JavaScript from Apple Events.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: false,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            guard let rawPid = arguments?["pid"]?.intValue,
                  let pid = Int32(exactly: rawPid)
            else {
                return errorResult("Missing or invalid pid.")
            }
            guard let rawWindowId = arguments?["window_id"]?.intValue,
                  let windowId = UInt32(exactly: rawWindowId)
            else {
                return errorResult("Missing or invalid window_id.")
            }
            guard let action = arguments?["action"]?.stringValue else {
                return errorResult("Missing required field action.")
            }

            // Resolve bundle ID from the running app (NSWorkspace requires main thread).
            let bundleId = await MainActor.run {
                NSWorkspace.shared.runningApplications
                    .first(where: { $0.processIdentifier == pid })?
                    .bundleIdentifier ?? ""
            }

            switch action {

            case "enable_javascript_apple_events":
                guard arguments?["user_has_confirmed_enabling"]?.boolValue == true else {
                    return errorResult(
                        "action=enable_javascript_apple_events requires user_has_confirmed_enabling=true. "
                        + "You MUST ask the user for explicit permission before calling this action. "
                        + "Do not set this flag unless the user has said yes.")
                }
                guard let targetBundleId = arguments?["bundle_id"]?.stringValue, !targetBundleId.isEmpty else {
                    return errorResult(
                        "action=enable_javascript_apple_events requires a bundle_id "
                        + "(e.g. com.google.Chrome).")
                }
                do {
                    try await BrowserJS.enableJavaScriptAppleEvents(bundleId: targetBundleId)
                    return okResult(
                        "'Allow JavaScript from Apple Events' has been enabled in \(targetBundleId). "
                        + "The browser has been relaunched. You can now use execute_javascript.")
                } catch {
                    return errorResult("\(error)")
                }

            case "execute_javascript":
                guard let js = arguments?["javascript"]?.stringValue, !js.isEmpty else {
                    return errorResult(
                        "action=execute_javascript requires a non-empty javascript field.")
                }
                do {
                    let result = try await executeJS(js, bundleId: bundleId, pid: pid, windowId: windowId)
                    return okResult("## Result\n\n```\n\(result)\n```")
                } catch {
                    return errorResult("\(error)")
                }

            case "get_text":
                // For WKWebView/Tauri apps, skip JS injection and read the AX tree
                // directly. Check BrowserJS.supports() first so Safari is NOT
                // misrouted to the AX fallback (Safari links WebKit but uses Apple Events).
                if !BrowserJS.supports(bundleId: bundleId) && WebInspectorXPC.isWKWebViewApp(pid: pid) {
                    let axText = await axGetText(pid: pid, windowId: windowId)
                    return axText.map { okResult("## Page text (via AX tree)\n\n\($0)") }
                        ?? errorResult(
                            "No accessible text found in \(bundleId). "
                            + "The app's AX tree may not expose web content. "
                            + "Try get_window_state to inspect the full accessibility tree.")
                }
                do {
                    let result = try await executeJS(
                        "document.body.innerText",
                        bundleId: bundleId, pid: pid, windowId: windowId)
                    return okResult(result)
                } catch {
                    if let axText = await axGetText(pid: pid, windowId: windowId) {
                        return okResult("## Page text (via AX tree)\n\n\(axText)")
                    }
                    return errorResult("\(error)")
                }

            case "query_dom":
                guard let selector = arguments?["css_selector"]?.stringValue,
                      !selector.isEmpty
                else {
                    return errorResult(
                        "action=query_dom requires a non-empty css_selector field.")
                }
                // For WKWebView/Tauri apps, go straight to AX role query.
                // Check BrowserJS.supports() first so Safari is NOT misrouted.
                if !BrowserJS.supports(bundleId: bundleId) && WebInspectorXPC.isWKWebViewApp(pid: pid) {
                    let axResult = await axQueryDom(selector: selector, pid: pid, windowId: windowId)
                    return axResult.map {
                        okResult("## AX query: `\(selector)` (via accessibility tree)\n\n```json\n\($0)\n```")
                    } ?? errorResult(
                        "No elements matching '\(selector)' found in the AX tree of \(bundleId).")
                }
                let attrs: [String]
                if let rawAttrs = arguments?["attributes"]?.arrayValue {
                    attrs = rawAttrs.compactMap { $0.stringValue }
                } else {
                    attrs = []
                }
                let attrJS = attrs.isEmpty
                    ? "[]"
                    : "[\(attrs.map { jsonString($0) }.joined(separator: ", "))]"
                let js = """
                (() => {
                  const attrs = \(attrJS);
                  return JSON.stringify(
                    Array.from(document.querySelectorAll(\(jsonString(selector)))).map(el => {
                      const obj = { tag: el.tagName.toLowerCase(), text: el.innerText?.trim() };
                      for (const a of attrs) obj[a] = el.getAttribute(a);
                      return obj;
                    })
                  );
                })()
                """
                do {
                    let result = try await executeJS(js, bundleId: bundleId, pid: pid, windowId: windowId)
                    return okResult("## DOM query: `\(selector)`\n\n```json\n\(result)\n```")
                } catch {
                    if let axResult = await axQueryDom(
                        selector: selector, pid: pid, windowId: windowId) {
                        return okResult(
                            "## AX query: `\(selector)` (via accessibility tree)\n\n```json\n\(axResult)\n```")
                    }
                    return errorResult("\(error)")
                }

            default:
                return errorResult(
                    "Unknown action '\(action)'. Valid values: execute_javascript, get_text, "
                    + "query_dom, enable_javascript_apple_events.")
            }
        }
    )

    private static func okResult(_ text: String) -> CallTool.Result {
        CallTool.Result(content: [.text(text: text, annotations: nil, _meta: nil)])
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }

    /// Route JS execution to the right backend:
    /// 1. Apple Events (BrowserJS) — Chrome, Brave, Edge, Safari by bundle ID.
    /// 2. Electron CDP (ElectronJS) — Electron apps (SIGUSR1 or --remote-debugging-port).
    /// 3. WebKit TCP (WebKitJS) — GTK/WPE WebKit builds (Linux); probes ports 9226–9228.
    /// 4. WebKit Mach IPC (WebInspectorXPC) — macOS WKWebView / Tauri apps via webinspectord.
    /// 5. Error — unsupported app type.
    private static func executeJS(
        _ javascript: String,
        bundleId: String,
        pid: Int32,
        windowId: UInt32
    ) async throws -> String {
        // Apple Events path — covers all Chromium browsers and Safari.
        if BrowserJS.supports(bundleId: bundleId) {
            return try await BrowserJS.execute(
                javascript: javascript, bundleId: bundleId, windowId: windowId)
        }
        // Electron CDP path — SIGUSR1 → V8 inspector → Runtime.evaluate.
        if ElectronJS.isElectron(pid: pid) {
            return try await ElectronJS.execute(javascript: javascript, pid: pid)
        }
        // GTK/WPE WebKit TCP path (Linux; probes WEBKIT_INSPECTOR_SERVER ports).
        // On macOS this always comes back empty so falls through quickly.
        if await WebKitJS.isAvailable() {
            return try await WebKitJS.execute(javascript: javascript)
        }
        // macOS WKWebView / Tauri apps: execute_javascript is not available
        // without com.apple.private.webinspector.remote-inspection-debugger
        // (Apple-provisioned, not grantable to third parties on macOS 15+).
        // Use get_text or query_dom — those route through the AX tree and work
        // without any special entitlements. See WebInspectorXPC.swift for the
        // full protocol implementation to enable when the entitlement is available.
        if WebInspectorXPC.isWKWebViewApp(pid: pid) {
            throw WKWebViewJSUnavailableError(bundleId: bundleId)
        }
        throw BrowserJS.Error.unsupportedBrowser(bundleId)
    }

    // MARK: - WKWebView JS unavailable error

    private struct WKWebViewJSUnavailableError: Error, CustomStringConvertible {
        let bundleId: String
        var description: String {
            "execute_javascript is not available for WKWebView/Tauri apps (\(bundleId)) — "
            + "the macOS webinspectord inspector requires "
            + "com.apple.private.webinspector.remote-inspection-debugger (Apple-provisioned only).\n"
            + "Use get_text or query_dom instead — both work via the AX tree without any entitlement."
        }
    }

    // MARK: - AX fallbacks for WKWebView/Tauri apps

    /// Extract body text from the AX tree — fallback for apps where JS
    /// injection is unavailable (Tauri/WKWebView without inspector).
    private static func axGetText(pid: Int32, windowId: UInt32) async -> String? {
        guard let snapshot = try? await AppStateRegistry.engine.snapshot(
            pid: pid, windowId: windowId)
        else { return nil }
        let text = AXPageReader.extractText(from: snapshot.treeMarkdown)
        if !text.isEmpty { return text }
        return snapshot.treeMarkdown.isEmpty ? nil : snapshot.treeMarkdown
    }

    /// Query the AX tree by CSS selector (role-mapped) — fallback for
    /// apps where JS injection is unavailable.
    private static func axQueryDom(
        selector: String,
        pid: Int32,
        windowId: UInt32
    ) async -> String? {
        guard let snapshot = try? await AppStateRegistry.engine.snapshot(
            pid: pid, windowId: windowId)
        else { return nil }
        let elements = AXPageReader.query(selector: selector, from: snapshot.treeMarkdown)
        guard !elements.isEmpty else { return nil }
        let items: [[String: Any]] = elements.map { el in
            var obj: [String: Any] = [
                "role": el.role,
                "text": el.title.isEmpty ? el.value : el.title,
            ]
            if let idx = el.index { obj["element_index"] = idx }
            if !el.description.isEmpty { obj["description"] = el.description }
            return obj
        }
        guard let data = try? JSONSerialization.data(
            withJSONObject: items, options: [.prettyPrinted]),
              let str = String(data: data, encoding: .utf8)
        else { return nil }
        return str
    }

    /// JSON-safe string literal for embedding in JS source.
    private static func jsonString(_ value: String) -> String {
        let escaped = value
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n")
            .replacingOccurrences(of: "\r", with: "\\r")
            .replacingOccurrences(of: "\t", with: "\\t")
        return "\"\(escaped)\""
    }
}
