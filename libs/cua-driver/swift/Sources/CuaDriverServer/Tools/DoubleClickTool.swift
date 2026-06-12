import ApplicationServices
import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

/// Unified double-click primitive. Two addressing modes, pid always
/// required:
///
/// - `element_index` from the last `get_window_state(pid, window_id)` →
///   performs `AXOpen` when the element advertises it (Finder items,
///   openable list rows, document cells that expose an open action);
///   otherwise resolves the element's on-screen center and falls
///   back to a stamped pixel double-click. This mirrors `click`'s
///   AX-first policy: when a semantic action exists, use it; when it
///   doesn't, fall back to the event-synthesis recipe that actually
///   triggers `dblclick` on the target.
///
/// - `x, y` window-local screenshot pixels → routes directly through
///   `MouseInput.click(count: 2)` — the primer-gated auth-signed
///   recipe that lands backgrounded double-clicks on Chromium web
///   content (YouTube fullscreen toggle, Finder Column View open,
///   list-row open-on-double-click). `modifier` holds cmd/shift/
///   option/ctrl during the gesture.
///
/// Exactly one mode must be supplied. Missing pid, both modes, or
/// neither mode returns `isError` before any AX / event work happens.
public enum DoubleClickTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "double_click",
            description: """
                Double-click against a target pid. Two addressing modes:

                - `element_index` + `window_id` (from the last
                  `get_window_state` snapshot of that window) —
                  performs `AXOpen` when the element advertises it
                  (Finder items, list rows, openable cells); otherwise
                  falls back to a stamped pixel double-click at the
                  element's on-screen center. Pure AX RPC in the
                  AXOpen branch; pixel recipe when no AX open action
                  exists. Requires a prior
                  `get_window_state(pid, window_id)` in this turn.

                - `x`, `y` (window-local screenshot pixels, top-left
                  origin of the PNG returned by `get_window_state`) —
                  synthesizes a stamped double-click (two down/up
                  pairs with clickState 1→2) and delivers to the pid.
                  The primer-gated auth-signed recipe lands on
                  backgrounded Chromium web content (dblclick events
                  that trigger YouTube fullscreen toggle, Finder
                  open-on-double-click, etc.). `modifier` holds
                  cmd/shift/option/ctrl during the gesture.

                Exactly one of `element_index` or (`x` AND `y`) must be
                provided. `pid` is required in both modes. `window_id`
                is required when `element_index` is used. `modifier`
                only takes effect in the pixel path (AXOpen doesn't
                propagate modifier keys).
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "element_index": [
                        "type": "integer",
                        "description":
                            "Element index from the last get_window_state for the same (pid, window_id). Routes through AXOpen when advertised, else pixel double-click at the element's center. Requires window_id.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID for the window whose get_window_state produced the element_index. Required when element_index is used; ignored in the pixel path.",
                    ],
                    "x": [
                        "type": "number",
                        "description":
                            "X in window-local screenshot pixels — same space as the PNG get_window_state returns. Top-left origin of the target's window. Must be provided together with y.",
                    ],
                    "y": [
                        "type": "number",
                        "description":
                            "Y in window-local screenshot pixels — same space as the PNG get_window_state returns. Top-left origin of the target's window. Must be provided together with x.",
                    ],
                    "modifier": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Modifier keys held during the double-click: cmd/shift/option/ctrl. Pixel path only.",
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
            guard let pid = Int32(exactly: rawPid) else {
                return errorResult(
                    "pid \(rawPid) is outside the supported Int32 range.")
            }

            let elementIndex = arguments?["element_index"]?.intValue
            let rawWindowId = arguments?["window_id"]?.intValue
            let x = coerceDouble(arguments?["x"])
            let y = coerceDouble(arguments?["y"])
            let hasXY = x != nil && y != nil
            let hasPartialXY = (x != nil) != (y != nil)

            if hasPartialXY {
                return errorResult(
                    "Provide both x and y together, not just one.")
            }
            if elementIndex != nil && hasXY {
                return errorResult(
                    "Provide either element_index or (x, y), not both.")
            }
            if elementIndex == nil && !hasXY {
                return errorResult(
                    "Provide element_index or (x, y) to address the double-click target.")
            }
            if elementIndex != nil && rawWindowId == nil {
                return errorResult(
                    "window_id is required when element_index is used — the "
                    + "element_index cache is scoped per (pid, window_id). Pass "
                    + "the same window_id you used in `get_window_state`.")
            }

            let modifiers = arguments?["modifier"]?.arrayValue?.compactMap {
                $0.stringValue
            } ?? []

            let windowId: UInt32?
            if let rawWindowId {
                guard let checked = UInt32(exactly: rawWindowId) else {
                    return errorResult(
                        "window_id \(rawWindowId) is outside the supported UInt32 range.")
                }
                windowId = checked
            } else {
                windowId = nil
            }

            if let index = elementIndex, let windowId {
                return await performElementDoubleClick(
                    pid: pid, windowId: windowId, index: index)
            }
            return await performPixelDoubleClick(
                pid: pid,
                windowId: windowId,
                x: x!, y: y!, modifiers: modifiers)
        }
    )

    // MARK: - Element-indexed path

    private static func performElementDoubleClick(
        pid: Int32, windowId: UInt32, index: Int
    ) async -> CallTool.Result {
        do {
            let element = try await AppStateRegistry.engine.lookup(
                pid: pid,
                windowId: windowId,
                elementIndex: index
            )

            if let center = AXInput.screenCenter(of: element) {
                await MainActor.run {
                    AgentCursor.shared.pinAbove(pid: pid)
                }
                await AgentCursor.shared.animateAndWait(to: center)
            }

            // AXOpen if the element advertises it — the semantic
            // equivalent of a double-click on an openable item, with
            // no synthesized events. Everything else (buttons, cells
            // without an open action, canvas-backed rows) falls
            // through to the pixel recipe at the element's center,
            // which produces a real dblclick the target receives as
            // a gesture, not two discrete AX press calls.
            let advertisesOpen = advertisedActions(of: element).contains("AXOpen")
            let target = AXInput.describe(element)

            if advertisesOpen {
                try await AppStateRegistry.focusGuard.withFocusSuppressed(
                    pid: pid,
                    element: element
                ) {
                    try AXInput.performAction("AXOpen", on: element)
                }
                await MainActor.run {
                    AgentCursor.shared.pinAbove(pid: pid)
                }
                await AgentCursor.shared.playClickPress()
                await AgentCursor.shared.finishClick(pid: pid)
                let summary =
                    "✅ Performed AXOpen on [\(index)] \(target.role ?? "?") \"\(target.title ?? "")\"."
                return CallTool.Result(
                    content: [.text(text: summary, annotations: nil, _meta: nil)]
                )
            }

            guard let center = AXInput.screenCenter(of: element) else {
                return errorResult(
                    "Element [\(index)] has no on-screen position; cannot double-click without AXOpen.")
            }

            try MouseInput.click(
                at: center,
                toPid: pid,
                button: .left,
                count: 2,
                modifiers: []
            )
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            await AgentCursor.shared.playClickPress()
            await AgentCursor.shared.finishClick(pid: pid)
            let summary =
                "✅ Posted double-click to [\(index)] \(target.role ?? "?") \"\(target.title ?? "")\" at screen-point (\(Int(center.x)), \(Int(center.y)))."
            return CallTool.Result(
                content: [.text(text: summary, annotations: nil, _meta: nil)]
            )
        } catch let error as AppStateError {
            return errorResult(error.description)
        } catch let error as AXInputError {
            return errorResult(error.description)
        } catch let error as MouseInputError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error: \(error)")
        }
    }

    // MARK: - Pixel-addressed path

    private static func performPixelDoubleClick(
        pid: Int32, windowId: UInt32?,
        x: Double, y: Double, modifiers: [String]
    ) async -> CallTool.Result {
        let screenPoint: CGPoint
        do {
            if let windowId {
                screenPoint = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: x, y: y),
                    forPid: pid,
                    windowId: windowId)
            } else {
                screenPoint = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: x, y: y), forPid: pid)
            }
        } catch let error as WindowCoordinateSpaceError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error resolving window: \(error)")
        }

        await MainActor.run {
            AgentCursor.shared.pinAbove(pid: pid)
        }
        await AgentCursor.shared.animateAndWait(to: screenPoint)

        do {
            try MouseInput.click(
                at: screenPoint,
                toPid: pid,
                button: .left,
                count: 2,
                modifiers: modifiers
            )
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            await AgentCursor.shared.playClickPress()
            await AgentCursor.shared.finishClick(pid: pid)
            let modSuffix =
                modifiers.isEmpty ? "" : " with \(modifiers.joined(separator: "+"))"
            let summary =
                "Posted double-click\(modSuffix) to pid \(pid) at window-pixel (\(Int(x)), \(Int(y))) "
                + "→ screen-point (\(Int(screenPoint.x)), \(Int(screenPoint.y)))."
            return CallTool.Result(
                content: [.text(text: "✅ \(summary)", annotations: nil, _meta: nil)]
            )
        } catch let error as MouseInputError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error: \(error)")
        }
    }

    // MARK: - Helpers

    private static func advertisedActions(of element: AXUIElement) -> [String] {
        var names: CFArray?
        let result = AXUIElementCopyActionNames(element, &names)
        guard result == .success, let names = names as? [String] else { return [] }
        return names
    }

    /// JSON numbers without a decimal point parse as `.int` from the
    /// MCP Value type. Callers writing `{"x": 100}` would be silently
    /// rejected by `.doubleValue` alone, so promote `.int` here too.
    private static func coerceDouble(_ value: Value?) -> Double? {
        if let d = value?.doubleValue { return d }
        if let i = value?.intValue { return Double(i) }
        return nil
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
