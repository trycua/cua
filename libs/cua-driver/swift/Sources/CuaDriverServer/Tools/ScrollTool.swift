import ApplicationServices
import CuaDriverCore
import Foundation
import MCP

/// Scroll a pid's focused region via synthesized keystrokes.
///
/// We originally tried posting `CGEventCreateScrollWheelEvent2` via
/// SkyLight's auth-signed `SLEventPostToPid` — same path keyboard uses
/// successfully. It doesn't work against Chromium: probe tests showed
/// wheel events posted via that path are silently dropped no matter
/// what wheelCount / wheel-delta / auth-subclass combination we send.
/// SkyLight exposes dedicated auth subclasses for Key / Mouse /
/// Gesture but none for Scroll, so the factory falls back to the bare
/// parent class, and Chromium apparently rejects parent-class-authed
/// wheel events outright.
///
/// Keyboard scroll works everywhere keys work (which is now the
/// canonical "reaches Chromium" path via
/// `SLSSkyLightKeyEventAuthenticationMessage`): PageUp/PageDown
/// scrolls browsers, text views, and every standard Cocoa scroll
/// view; arrow keys give finer-grained scroll in most targets.
public enum ScrollTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "scroll",
            description: """
                Scroll the target pid's focused region by synthesized
                keystrokes.

                Mapping: `by: "page"` → PageDown/PageUp × amount;
                `by: "line"` → DownArrow/UpArrow × amount. Horizontal
                variants use Left/Right arrow keys. Keys are posted via
                the same auth-signed `SLEventPostToPid` path as
                `press_key`, so backgrounded Chromium / WebKit windows
                scroll correctly.

                Why keyboard instead of wheel events: probe tests
                showed `CGEventCreateScrollWheelEvent2` posted via
                SkyLight's per-pid path is silently dropped by
                Chromium (no Scroll-specific auth subclass exists;
                the factory falls back to the bare parent class and
                renderers reject it). Keystrokes don't have that
                problem — that's why PageDown works when the wheel
                event didn't.

                Optional `element_index` + `window_id` (from the last
                `get_window_state` snapshot of that window) focuses the
                element (`AXFocused=true`) before the keystroke, so the
                keys land in the scrollable element rather than whatever
                previously had focus. Skip it if focus was already
                established by a prior click.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "direction"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "direction": [
                        "type": "string",
                        "enum": ["up", "down", "left", "right"],
                    ],
                    "amount": [
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "description":
                            "Number of keystroke repetitions. Default: 3.",
                    ],
                    "by": [
                        "type": "string",
                        "enum": ["line", "page"],
                        "description": "Scroll granularity. Default: line.",
                    ],
                    "element_index": [
                        "type": "integer",
                        "description":
                            "Optional element_index from the last get_window_state for the same (pid, window_id) — focuses the element before the keystrokes fire. Requires window_id.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID for the window whose get_window_state produced the element_index. Required when element_index is used.",
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: false,
                openWorldHint: true
            )
        ),
        invoke: { arguments in
            guard let rawPid = arguments?["pid"]?.intValue else {
                return errorResult("Missing required integer field pid.")
            }
            guard let direction = arguments?["direction"]?.stringValue else {
                return errorResult("Missing required string field direction.")
            }
            let amount = arguments?["amount"]?.intValue ?? 3
            let by = arguments?["by"]?.stringValue ?? "line"
            let elementIndex = arguments?["element_index"]?.intValue
            let rawWindowId = arguments?["window_id"]?.intValue
            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }
            if elementIndex != nil && rawWindowId == nil {
                return errorResult(
                    "window_id is required when element_index is used — the "
                    + "element_index cache is scoped per (pid, window_id). Pass "
                    + "the same window_id you used in `get_window_state`.")
            }
            guard let key = scrollKey(direction: direction, by: by) else {
                return errorResult(
                    "Invalid direction/by combination: \(direction)/\(by).")
            }

            do {
                var target = TargetedElement(
                    role: nil, subrole: nil, title: nil, description: nil)
                var focusedElement: AXUIElement?
                if let index = elementIndex, let rawWindowId {
                    guard let windowId = UInt32(exactly: rawWindowId) else {
                        return errorResult(
                            "window_id \(rawWindowId) is outside the supported UInt32 range.")
                    }
                    let element = try await AppStateRegistry.engine.lookup(
                        pid: pid,
                        windowId: windowId,
                        elementIndex: index)
                    target = AXInput.describe(element)
                    focusedElement = element
                }

                if let element = focusedElement {
                    try await AppStateRegistry.focusGuard.withFocusSuppressed(
                        pid: pid, element: element
                    ) {
                        try? AXInput.setAttribute(
                            "AXFocused",
                            on: element,
                            value: kCFBooleanTrue as CFTypeRef
                        )
                        for _ in 0..<amount {
                            try KeyboardInput.press(
                                key, modifiers: [], toPid: pid)
                        }
                    }
                } else {
                    for _ in 0..<amount {
                        try KeyboardInput.press(
                            key, modifiers: [], toPid: pid)
                    }
                }

                let elementDesc = elementIndex.map {
                    " (target [\($0)] \(target.role ?? "?"))"
                } ?? ""
                let summary =
                    "✅ Scrolled pid \(rawPid) \(direction) via \(amount)× "
                    + "\(key) keystroke(s)\(elementDesc)."
                return CallTool.Result(
                    content: [.text(text: summary, annotations: nil, _meta: nil)]
                )
            } catch let error as AppStateError {
                return errorResult(error.description)
            } catch let error as AXInputError {
                return errorResult(error.description)
            } catch let error as KeyboardError {
                return errorResult(error.description)
            } catch {
                return errorResult("Unexpected error: \(error)")
            }
        }
    )

    /// Map (direction, by) to a keyboard key name. "Line" granularity
    /// uses arrow keys; "page" uses Page / Home / End equivalents.
    /// Horizontal "page" scroll isn't a standard shortcut — we fall
    /// back to arrow keys for those (the amount loop still applies).
    private static func scrollKey(direction: String, by: String) -> String? {
        switch (direction, by) {
        case ("up", "line"):    return "up"
        case ("down", "line"):  return "down"
        case ("left", "line"):  return "left"
        case ("right", "line"): return "right"
        case ("up", "page"):    return "pageup"
        case ("down", "page"):  return "pagedown"
        // Horizontal page scroll isn't a standard Cocoa/Chromium
        // shortcut — fall back to arrow keys and let amount do the
        // work. Callers wanting a bigger horizontal step can raise
        // amount.
        case ("left", "page"):  return "left"
        case ("right", "page"): return "right"
        default: return nil
        }
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
