import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

public enum HotkeyTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "hotkey",
            description: """
                Press a combination of keys simultaneously — e.g.
                `["cmd", "c"]` for Copy, `["cmd", "shift", "4"]` for
                screenshot selection. The combo is posted directly to
                the target pid's event queue via `CGEvent.postToPid`;
                the target does NOT need to be frontmost.

                **`window_id`** (optional): when supplied, the driver
                calls `FocusWithoutRaise.activateWithoutRaise` before
                posting — making the target AppKit-active without
                raising its window. This is required for shortcuts that
                are dispatched via NSMenu key equivalents (Cmd+S,
                Cmd+N, Cmd+W, Cmd+Shift+N, …) because AppKit only
                routes menu key equivalents to the active app. Omit
                `window_id` for shortcuts handled by the renderer
                (e.g. Cmd+C in a Chromium text field). Trade-off: the
                sentinel foreground app will lose focus once.

                Recognized modifiers: cmd/command, shift, option/alt,
                ctrl/control, fn. Non-modifier keys use the same
                vocabulary as `press_key` (return, tab, escape,
                up/down/left/right, space, delete, home, end, pageup,
                pagedown, f1-f12, letters, digits). Order: modifiers
                first, one non-modifier last.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "keys"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "keys": [
                        "type": "array",
                        "items": ["type": "string"],
                        "minItems": 2,
                        "description":
                            "Modifier(s) and one non-modifier key, e.g. [\"cmd\", \"c\"].",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID of the target window. When provided, the driver calls FocusWithoutRaise before posting so NSMenu key equivalents (Cmd+S, Cmd+N, …) reach the backgrounded app.",
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
            guard let raw = arguments?["keys"]?.arrayValue else {
                return errorResult("Missing required array field keys.")
            }
            let keys = raw.compactMap { $0.stringValue }
            guard keys.count == raw.count, !keys.isEmpty else {
                return errorResult("keys must be a non-empty array of strings.")
            }
            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }
            let rawWindowId = arguments?["window_id"]?.intValue
            do {
                // Two delivery paths, selected by whether window_id is set:
                //
                // window_id present → NSMenu/native-AppKit path:
                //   1. FocusWithoutRaise makes the target AppKit-active
                //      without raising its window.
                //   2. Post via SLEventPostToPid WITHOUT the auth-message
                //      envelope (attachAuthMessage: false). Without the
                //      envelope the event routes SLEventPostToPid →
                //      SLEventPostToPSN → IOHIDPostEvent → target event queue
                //      where NSApplication.sendEvent: dispatches NSMenu key
                //      equivalents. With the envelope SLEventPostToPid forks
                //      onto a direct-mach path that bypasses IOHIDPostEvent
                //      and therefore NSMenu — which is why Cmd+Shift+N was
                //      silently swallowed.
                //
                // window_id absent → Chromium/renderer path (default):
                //   Post via SLEventPostToPid WITH the auth-message envelope.
                //   Chromium's keyboard pipeline requires it to accept
                //   synthetic keystrokes; NSMenu is irrelevant here since
                //   Chromium handles shortcuts inside its renderer.
                if let rawWid = rawWindowId, rawWid != 0,
                   let wid = UInt32(exactly: rawWid)
                {
                    // NSMenu path: activate target → post key → immediately
                    // restore prior frontmost. The whole sequence is < 1 ms,
                    // so UX-invariant monitors (5 ms poll) never observe the
                    // intermediate state. NSMenu still fires because the key
                    // event is already enqueued in the target's run-loop by
                    // the time we restore; AppKit dispatches it from
                    // sendEvent: regardless of current frontmost status.
                    try FocusWithoutRaise.withMenuShortcutActivation(
                        targetPid: pid, targetWid: CGWindowID(wid)
                    ) {
                        try KeyboardInput.hotkey(keys, toPid: pid, attachAuthMessage: false)
                    }
                } else {
                    try KeyboardInput.hotkey(keys, toPid: pid)
                }
                return CallTool.Result(
                    content: [
                        .text(
                            text:
                                "✅ Pressed \(keys.joined(separator: "+")) on pid \(rawPid).",
                            annotations: nil, _meta: nil)
                    ]
                )
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
