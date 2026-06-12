import ApplicationServices
import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

/// Unified right-click primitive. Two addressing modes, pid always
/// required:
///
/// - `element_index` from the last `get_window_state(pid, window_id)` →
///   wraps `AXShowMenu` against the cached element in
///   `FocusGuard.withFocusSuppressed`. This is the former
///   `secondary_action` tool, collapsed in. Works against
///   backgrounded / hidden windows as long as the element advertises
///   `AXShowMenu`.
///
/// - `x, y` window-local screenshot pixels (same space as the PNG
///   returned by `get_window_state`) →
///   synthesizes a `rightMouseDown` / `rightMouseUp` CGEvent pair
///   and delivers them to the pid via `SkyLightEventPost.postToPid`
///   (auth-signed). Driver converts image-pixel → screen-point
///   internally. Known caveat: Chromium web content coerces the
///   event back to a left-click — see `MouseInput` for the full note.
///
/// Exactly one mode must be supplied. Missing pid, both modes, or
/// neither mode returns `isError` before any AX / event work happens.
public enum RightClickTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "right_click",
            description: """
                Right-click against a target pid. Two addressing modes:

                - `element_index` + `window_id` (from the last
                  `get_window_state` snapshot of that window) —
                  performs `AXShowMenu` on the cached element. This is the
                  reliable path: pure AX RPC, works on backgrounded /
                  hidden windows, no cursor move or focus steal. Requires
                  a prior `get_window_state(pid, window_id)` in this turn.

                - `x`, `y` (window-local screenshot pixels, top-left
                  origin of the PNG returned by `get_window_state`) —
                  internally routed through AX (`AXShowMenu`) when the
                  pixel resolves to an actionable element (same
                  reliability as `element_index`); falls back to a
                  synthesized right-mouse-down / right-mouse-up
                  CGEvent pair posted via auth-signed
                  `SLEventPostToPid` for non-AX surfaces (canvas /
                  WebView / games). Driver converts image-pixel →
                  screen-point internally. Experimental on Chromium
                  web content in the CGEvent fallback: the event is
                  observed to land as a left-click there (a known
                  limitation matching third-party Computer Use
                  implementations). `modifier` forces the CGEvent
                  path (AX doesn't propagate modifier keys).

                Exactly one of `element_index` or (`x` AND `y`) must be
                provided. `pid` is required in both modes. `window_id`
                is required when `element_index` is used. `modifier`
                only takes effect in the pixel path (AX actions don't
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
                            "Element index from the last get_window_state for the same (pid, window_id). Routes through AXShowMenu. Requires window_id.",
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
                            "Modifier keys held during the right-click: cmd/shift/option/ctrl. Pixel path only.",
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
                    "Provide element_index or (x, y) to address the right-click target.")
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
                return await performElementRightClick(
                    pid: pid, windowId: windowId, index: index)
            }
            return await performPixelRightClick(
                pid: pid,
                windowId: windowId,
                x: x!, y: y!, modifiers: modifiers)
        }
    )

    // MARK: - Element-indexed path

    private static func performElementRightClick(
        pid: Int32, windowId: UInt32, index: Int
    ) async -> CallTool.Result {
        do {
            let element = try await AppStateRegistry.engine.lookup(
                pid: pid,
                windowId: windowId,
                elementIndex: index
            )
            // Capture advertised actions before dispatch — see the
            // same pattern in `ClickTool.performElementClick`.
            // `AXShowMenu` on a non-advertising element returns
            // `.success` but does nothing; callers deserve the hint.
            let advertisedActions = AXInput.advertisedActionNames(of: element)
            if let center = AXInput.screenCenter(of: element) {
                await MainActor.run {
                    AgentCursor.shared.pinAbove(pid: pid)
                }
                await AgentCursor.shared.animateAndWait(to: center)
            }
            try await AppStateRegistry.focusGuard.withFocusSuppressed(
                pid: pid,
                element: element
            ) {
                try AXInput.performAction("AXShowMenu", on: element)
            }
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            await AgentCursor.shared.playClickPress()
            await AgentCursor.shared.finishClick(pid: pid)
            let target = AXInput.describe(element)
            var summary =
                "✅ Shown menu for [\(index)] \(target.role ?? "?") \"\(target.title ?? "")\"."
            if !advertisedActions.contains("AXShowMenu") {
                let advertisedList = advertisedActions.isEmpty
                    ? "none"
                    : advertisedActions.joined(separator: ", ")
                summary += "\n⚠️ Element does not advertise AXShowMenu"
                summary += " (actions: \(advertisedList))."
                summary += " Action may have been a no-op."
                summary += " Retry with a pixel right-click (`right_click(pid, x, y)`)"
                summary += " if no menu appeared."
            }
            return CallTool.Result(
                content: [.text(text: summary, annotations: nil, _meta: nil)]
            )
        } catch AppStateError.noCachedState(let pid, let windowId) {
            return errorResult(noCachedStateMessage(pid: pid, windowId: windowId))
        } catch let error as AppStateError {
            return errorResult(error.description)
        } catch let error as AXInputError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error: \(error)")
        }
    }

    /// Build the error message surfaced when the element-index cache
    /// misses. If no daemon is listening on the UDS the caller is
    /// running `cua-driver right_click` in-process, which starts
    /// with an empty cache. Prepend a hint to start the daemon first
    /// since the default description assumes the cache is live and
    /// just needs a `get_window_state` refresh. Mirrors
    /// `ClickTool.noCachedStateMessage`.
    private static func noCachedStateMessage(pid: Int32, windowId: UInt32) -> String {
        let socketPath = DaemonPaths.defaultSocketPath()
        if !DaemonClient.isDaemonListening(socketPath: socketPath) {
            return
                "No cached AX state for pid \(pid). Start the daemon first: "
                + "`open -n -g -a CuaDriver --args serve` "
                + "(or `cua-driver serve &` — the CLI auto-relaunches via "
                + "`open` if your shell's TCC context is wrong). "
                + "Element-indexed clicks read a cache populated by "
                + "`get_window_state`, which only persists across CLI "
                + "calls when a daemon is running."
        }
        return AppStateError.noCachedState(pid: pid, windowId: windowId).description
    }

    // MARK: - Pixel-addressed path

    private static func performPixelRightClick(
        pid: Int32, windowId: UInt32?,
        x: Double, y: Double, modifiers: [String]
    ) async -> CallTool.Result {
        // `x, y` are window-local screenshot pixels. Convert to screen
        // points before injecting. See ClickTool.performPixelClick for
        // the same pattern and rationale.
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

        // Glide the overlay to the target before dispatching — same
        // visible pacing as element-indexed clicks. No-op when
        // disabled.
        await MainActor.run {
            AgentCursor.shared.pinAbove(pid: pid)
        }
        await AgentCursor.shared.animateAndWait(to: screenPoint)

        do {
            try MouseInput.rightClick(
                at: screenPoint, toPid: pid, modifiers: modifiers)
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            await AgentCursor.shared.playClickPress()
            await AgentCursor.shared.finishClick(pid: pid)
            let modSuffix =
                modifiers.isEmpty ? "" : " with \(modifiers.joined(separator: "+"))"
            let summary =
                "Posted right-click\(modSuffix) to pid \(pid) at window-pixel (\(Int(x)), \(Int(y))) "
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
