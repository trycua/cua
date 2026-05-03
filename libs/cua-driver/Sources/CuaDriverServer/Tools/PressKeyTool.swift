import CuaDriverCore
import Foundation
import MCP

/// Unified key-press primitive. Always targets a specific pid —
/// previously there were two tools (`press_key` which went to the
/// frontmost app, and `press_key_in` which targeted a pid). The
/// frontmost-routed variant was a footgun because every driver-
/// backgrounded app produced keystrokes that landed in the user's
/// real foreground app. Merging them under a mandatory-pid shape
/// removes the footgun; the old `press_key_in` is gone.
///
/// `element_index` is optional. When present, the element is
/// focused via `AXSetAttribute(kAXFocused, true)` before the key is
/// posted — the canonical path for "type into this text field, then
/// press Return on it." When absent, the key is posted to the pid
/// directly with no focus change — the canonical path for scroll
/// keys (End / PageDown / Home) on the already-focused web area.
public enum PressKeyTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "press_key",
            description: """
                Press and release a single key, delivered directly to the
                target pid's event queue via `CGEvent.postToPid`. The target
                does NOT need to be frontmost — no focus steal.

                Optional `element_index` + `window_id` (from the last
                `get_window_state` snapshot of that window) pre-focuses
                that element (via `AXSetAttribute(kAXFocused, true)`)
                before the key fires; useful for "type into field →
                press Return on field." Without `element_index`, the
                key is posted to the pid with whatever element is
                already focused receiving it — useful for scroll keys
                on a backgrounded web view.

                The key is delivered via SkyLight's `SLEventPostToPid`
                with an attached `SLSEventAuthenticationMessage`,
                which is the path the renderer's keyboard pipeline
                accepts as authentic input. That's what lets Return
                commit a Chromium omnibox against a backgrounded
                window. Falls back to the public `CGEventPostToPid`
                when the SkyLight SPI isn't available.

                Key vocabulary: return, tab, escape, up/down/left/right,
                space, delete, home, end, pageup, pagedown, f1-f12, plus
                any letter or digit. Optional `modifiers` array takes
                cmd/shift/option/ctrl/fn. For true combinations (cmd+c),
                `hotkey` is a cleaner surface.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "key"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "key": [
                        "type": "string",
                        "description":
                            "Key name (return, tab, escape, up, down, left, right, space, delete, home, end, pageup, pagedown, f1-f12, letter, digit).",
                    ],
                    "modifiers": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Optional modifier names held while the key is pressed (cmd/shift/option/ctrl/fn).",
                    ],
                    "element_index": [
                        "type": "integer",
                        "description":
                            "Optional element_index from the last get_window_state for the same (pid, window_id). When present, the element is focused before the key fires. Requires window_id.",
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
                destructiveHint: true,
                idempotentHint: false,
                openWorldHint: true
            )
        ),
        invoke: { arguments in
            guard let rawPid = arguments?["pid"]?.intValue else {
                return errorResult("Missing required integer field pid.")
            }
            guard let key = arguments?["key"]?.stringValue else {
                return errorResult("Missing required string field key.")
            }
            let modifiers = (arguments?["modifiers"]?.arrayValue ?? [])
                .compactMap { $0.stringValue }
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

            do {
                if let index = elementIndex, let rawWindowId {
                    guard let windowId = UInt32(exactly: rawWindowId) else {
                        return errorResult(
                            "window_id \(rawWindowId) is outside the supported UInt32 range.")
                    }
                    let element = try await AppStateRegistry.engine.lookup(
                        pid: pid,
                        windowId: windowId,
                        elementIndex: index)

                    // Focus the element, then post the key via the
                    // auth-signed SkyLight path (handled inside
                    // KeyboardInput.press → sendKey). Earlier revisions
                    // mapped return → AXConfirm here as a workaround for
                    // the broken keystroke path; now that auth-signed
                    // SLEventPostToPid reaches the renderer's keyboard
                    // pipeline (Chrome omnibox commits correctly), the
                    // action mapping is a foot-gun — AXConfirm silently
                    // no-ops on Chromium and other web targets. Uniform
                    // focus-then-keystroke works everywhere.
                    try await AppStateRegistry.focusGuard.withFocusSuppressed(
                        pid: pid, element: element
                    ) {
                        try? AXInput.setAttribute(
                            "AXFocused",
                            on: element,
                            value: kCFBooleanTrue as CFTypeRef
                        )
                        try KeyboardInput.press(
                            key, modifiers: modifiers, toPid: pid)
                    }
                    let target = AXInput.describe(element)
                    return CallTool.Result(
                        content: [
                            .text(
                                text:
                                    "✅ Focused [\(index)] \(target.role ?? "?") and pressed \(key) on pid \(rawPid).",
                                annotations: nil, _meta: nil)
                        ]
                    )
                } else {
                    try KeyboardInput.press(
                        key, modifiers: modifiers, toPid: pid)
                    return CallTool.Result(
                        content: [
                            .text(
                                text:
                                    "✅ Pressed \(key) on pid \(rawPid).",
                                annotations: nil, _meta: nil)
                        ]
                    )
                }
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

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
