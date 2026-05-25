import CuaDriverCore
import Foundation
import MCP

/// Unified text-insertion primitive — always targets a specific pid.
///
/// Tries `AXSetAttribute(kAXSelectedText)` first (fast, bulk insert).
/// If the target element rejects the AX write (Chromium / Electron inputs
/// that don't expose `kAXSelectedText`), automatically falls back to
/// `KeyboardInput.typeCharacters` — character-by-character CGEvent synthesis
/// via `CGEvent.postToPid`. The fallback path is noted in the response summary
/// so callers can tell which path was taken.
public enum TypeTextTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "type_text",
            description: """
                Insert text into the target pid. Tries
                `AXSetAttribute(kAXSelectedText)` first (fast bulk insert —
                works for standard Cocoa text fields). If the target element
                rejects the AX write, automatically falls back to
                character-by-character `CGEvent.postToPid` synthesis —
                reaches Chromium / Electron inputs and any surface that
                doesn't implement `kAXSelectedText`.

                Special keys (Return, Escape, arrows, Tab) go through
                `press_key` / `hotkey` — they are not text.

                Optional `element_index` + `window_id` (from the last
                `get_window_state` snapshot of that window) pre-focuses
                that element before the write; useful for "fill this
                specific field." Without `element_index`, the write
                targets the pid's currently-focused element — useful
                after a prior click already set focus.

                `delay_ms` (0–200, default 30) spaces successive characters
                in the CGEvent fallback path so autocomplete and IME can keep
                up; ignored when the AX path succeeds.

                Requires Accessibility.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "text"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "text": [
                        "type": "string",
                        "description": "Text to insert at the target's cursor.",
                    ],
                    "element_index": [
                        "type": "integer",
                        "description":
                            "Optional element_index from the last get_window_state for the same (pid, window_id). When present, the element is focused before the write. Requires window_id.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID for the window whose get_window_state produced the element_index. Required when element_index is used.",
                    ],
                    "delay_ms": [
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 200,
                        "description":
                            "Milliseconds between characters in the CGEvent fallback path. Default 30. Ignored when the AX path succeeds.",
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
            guard let text = arguments?["text"]?.stringValue else {
                return errorResult("Missing required string field text.")
            }
            let elementIndex = arguments?["element_index"]?.intValue
            let rawWindowId = arguments?["window_id"]?.intValue
            let delayMs = arguments?["delay_ms"]?.intValue ?? 30
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

            // Attempt AX bulk-insert. On any AXInputError — or when a
            // silent-accept-but-no-DOM-update is detected (Chromium web
            // inputs accept kAXSelectedText writes without error but never
            // propagate the value to the HTML DOM) — fall back to CGEvent
            // character synthesis.
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
                    // Animate the agent cursor to the target element before
                    // writing — same visual feedback as click. No-op when the
                    // cursor is disabled or the element has no screen position.
                    if let center = AXInput.screenCenter(of: element) {
                        await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                        await AgentCursor.shared.animateAndWait(to: center)
                    }
                    do {
                        try await AppStateRegistry.focusGuard.withFocusSuppressed(
                            pid: pid, element: element
                        ) {
                            try AXInput.setAttribute(
                                "AXSelectedText",
                                on: element,
                                value: text as CFTypeRef
                            )
                        }
                        // Verify the write was actually applied — Chromium
                        // web inputs silently accept the call but never
                        // update their DOM value. Read AXValue back; if it's
                        // still empty when we just wrote non-empty text the
                        // write was a no-op and we must use CGEvent instead.
                        if !text.isEmpty,
                           let axValue = AXInput.stringAttribute("AXValue", of: element),
                           axValue.isEmpty
                        {
                            // Silent-accept detected — fall through.
                        } else {
                            let target = AXInput.describe(element)
                            let summary =
                                "✅ Inserted \(text.count) char(s) into [\(index)] \(target.role ?? "?") \"\(target.title ?? "")\" on pid \(rawPid) via AX."
                            await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                            await AgentCursor.shared.playClickPress()
                            await AgentCursor.shared.finishClick(pid: pid)
                            return CallTool.Result(
                                content: [.text(text: summary, annotations: nil, _meta: nil)]
                            )
                        }
                    } catch is AXInputError {
                        // AX rejected — fall through to CGEvent synthesis.
                    }
                } else {
                    do {
                        let element = try AXInput.focusedElement(pid: pid)
                        // Animate the cursor to the focused element before writing.
                        if let center = AXInput.screenCenter(of: element) {
                            await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                            await AgentCursor.shared.animateAndWait(to: center)
                        }
                        try AXInput.setAttribute(
                            "AXSelectedText",
                            on: element,
                            value: text as CFTypeRef
                        )
                        // Same silent-accept check as the element_index path.
                        if !text.isEmpty,
                           let axValue = AXInput.stringAttribute("AXValue", of: element),
                           axValue.isEmpty
                        {
                            // Silent-accept detected — fall through.
                        } else {
                            let target = AXInput.describe(element)
                            let summary =
                                "✅ Inserted \(text.count) char(s) into focused \(target.role ?? "?") \"\(target.title ?? "")\" on pid \(rawPid) via AX."
                            await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                            await AgentCursor.shared.playClickPress()
                            await AgentCursor.shared.finishClick(pid: pid)
                            return CallTool.Result(
                                content: [.text(text: summary, annotations: nil, _meta: nil)]
                            )
                        }
                    } catch is AXInputError {
                        // AX rejected — fall through to CGEvent synthesis.
                    }
                }

                // CGEvent fallback path — reached when AX write was rejected
                // (AXInputError) or silently accepted without updating the
                // DOM value (Chromium web inputs).
                try KeyboardInput.typeCharacters(
                    text,
                    delayMilliseconds: delayMs,
                    toPid: pid
                )
                let summary =
                    "✅ Typed \(text.count) char(s) on pid \(rawPid) via CGEvent (\(delayMs)ms delay)."
                await MainActor.run { AgentCursor.shared.pinAbove(pid: pid) }
                await AgentCursor.shared.playClickPress()
                await AgentCursor.shared.finishClick(pid: pid)
                return CallTool.Result(
                    content: [.text(text: summary, annotations: nil, _meta: nil)]
                )
            } catch let error as AppStateError {
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
