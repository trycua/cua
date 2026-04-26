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
                        "enum": ["execute_javascript", "get_text", "query_dom"],
                        "description": "Which page primitive to run.",
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
                do {
                    let result = try await executeJS(
                        "document.body.innerText",
                        bundleId: bundleId, pid: pid, windowId: windowId)
                    return okResult(result)
                } catch {
                    return errorResult("\(error)")
                }

            case "query_dom":
                guard let selector = arguments?["css_selector"]?.stringValue,
                      !selector.isEmpty
                else {
                    return errorResult(
                        "action=query_dom requires a non-empty css_selector field.")
                }
                let attrs: [String]
                if let rawAttrs = arguments?["attributes"]?.arrayValue {
                    attrs = rawAttrs.compactMap { $0.stringValue }
                } else {
                    attrs = []
                }
                let attrJS = attrs.isEmpty
                    ? "[]"
                    : "[\(attrs.map { "\"\($0)\"" }.joined(separator: ", "))]"
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
                    return errorResult("\(error)")
                }

            default:
                return errorResult(
                    "Unknown action '\(action)'. Valid values: execute_javascript, get_text, query_dom.")
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
    /// 2. Electron CDP (ElectronJS) — any Electron app detected by bundle presence.
    /// 3. Error — unsupported app type.
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
        throw BrowserJS.Error.unsupportedBrowser(bundleId)
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
