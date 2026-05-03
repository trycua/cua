import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

/// Pixel-addressed drag primitive. Two endpoints, both in window-local
/// screenshot pixels (the same space `get_window_state` returns), plus
/// the target pid.
///
/// macOS AX has no semantic "drag" action — every drag-and-drop, marquee
/// selection, slider scrub, and resize handle is event-synthesis. So
/// drag is pixel-only by design: there is no `element_index` mode the
/// way `click` / `double_click` have. Address one element with
/// `get_window_state`, read its `bounds`, and pass the pixel coordinates
/// you want to drag from / to.
///
/// The handler delegates to `MouseInput.drag`, which posts via
/// `.cghidEventTap` when the target is frontmost (real-cursor gesture,
/// reaches AppKit drag sources / canvas viewports) and via the
/// auth-signed pid-routed path when backgrounded (cursor-neutral).
public enum DragTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "drag",
            description: """
                Press-drag-release gesture from (`from_x`, `from_y`) to
                (`to_x`, `to_y`) in window-local screenshot pixels —
                the same space the PNG `get_window_state` returns.
                Top-left origin of the target's window.

                Use this for: marquee/lasso selection, drag-and-drop
                between source and destination, resizing via a handle,
                scrubbing a slider, repositioning a panel. macOS AX
                has no semantic drag action, so drag is pixel-only —
                there is no element-indexed mode. Address an element
                with `get_window_state`, read its `bounds`, and pass
                the pixel coordinates you want.

                `duration_ms` (default 500) is the wall-clock budget
                for the path between mouse-down and mouse-up; `steps`
                (default 20) is the number of intermediate `mouseDragged`
                events linearly interpolated along the path. Increase
                both for slower, more "human" drags; decrease for snap
                gestures. `modifier` keys (cmd/shift/option/ctrl) are
                held across the entire gesture — option-drag duplicates,
                shift-drag constrains the axis on most surfaces.

                Frontmost target: posts via `.cghidEventTap`. The real
                cursor visibly traces the drag path — unavoidable for
                AppKit-style drag sources, which accept only HID-origin
                events.

                Backgrounded target: posts via the auth-signed pid-routed
                path. Cursor-neutral, but some surfaces (OpenGL canvases,
                Blender, Unity) filter pid-routed dragged events at the
                event-source level — those targets must be frontmost.

                When `from_zoom` is true, both endpoints are pixel
                coordinates in the last `zoom` image for this pid; the
                driver maps them back to window coordinates before
                dispatching.
                """,
            inputSchema: [
                "type": "object",
                "required": ["pid", "from_x", "from_y", "to_x", "to_y"],
                "properties": [
                    "pid": [
                        "type": "integer",
                        "description": "Target process ID.",
                    ],
                    "window_id": [
                        "type": "integer",
                        "description":
                            "CGWindowID for the window the pixel coordinates were measured against. Optional — when omitted the driver picks the frontmost window of `pid`. Pass when the target has multiple windows and you measured against a specific one.",
                    ],
                    "from_x": [
                        "type": "number",
                        "description":
                            "Drag-start X in window-local screenshot pixels. Top-left origin.",
                    ],
                    "from_y": [
                        "type": "number",
                        "description":
                            "Drag-start Y in window-local screenshot pixels. Top-left origin.",
                    ],
                    "to_x": [
                        "type": "number",
                        "description":
                            "Drag-end X in window-local screenshot pixels.",
                    ],
                    "to_y": [
                        "type": "number",
                        "description":
                            "Drag-end Y in window-local screenshot pixels.",
                    ],
                    "duration_ms": [
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 10_000,
                        "description":
                            "Wall-clock duration of the drag path between mouseDown and mouseUp. Default: 500.",
                    ],
                    "steps": [
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description":
                            "Number of intermediate mouseDragged events linearly interpolated along the path. Default: 20.",
                    ],
                    "modifier": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Modifier keys held across the entire gesture: cmd/shift/option/ctrl.",
                    ],
                    "button": [
                        "type": "string",
                        "enum": ["left", "right", "middle"],
                        "description":
                            "Mouse button used for the drag. Default: left.",
                    ],
                    "from_zoom": [
                        "type": "boolean",
                        "description":
                            "When true, from_x/from_y/to_x/to_y are pixel coordinates in the last `zoom` image for this pid. The driver maps them back to window coordinates before dispatching.",
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
            guard
                let fromX = coerceDouble(arguments?["from_x"]),
                let fromY = coerceDouble(arguments?["from_y"]),
                let toX = coerceDouble(arguments?["to_x"]),
                let toY = coerceDouble(arguments?["to_y"])
            else {
                return errorResult(
                    "from_x, from_y, to_x, and to_y are all required (window-local pixels).")
            }

            let durationMs = arguments?["duration_ms"]?.intValue ?? 500
            let steps = arguments?["steps"]?.intValue ?? 20
            let modifiers = arguments?["modifier"]?.arrayValue?.compactMap {
                $0.stringValue
            } ?? []
            let buttonString = arguments?["button"]?.stringValue ?? "left"
            let fromZoom = arguments?["from_zoom"]?.boolValue ?? false

            let button: MouseInput.Button
            switch buttonString.lowercased() {
            case "left": button = .left
            case "right": button = .right
            case "middle": button = .middle
            default:
                return errorResult(
                    "Unknown button \"\(buttonString)\" — expected left, right, or middle.")
            }

            let rawWindowId = arguments?["window_id"]?.intValue
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

            return await performPixelDrag(
                pid: pid,
                windowId: windowId,
                fromX: fromX, fromY: fromY,
                toX: toX, toY: toY,
                durationMs: durationMs,
                steps: steps,
                modifiers: modifiers,
                button: button,
                fromZoom: fromZoom
            )
        }
    )

    private static func performPixelDrag(
        pid: Int32,
        windowId: UInt32?,
        fromX: Double, fromY: Double,
        toX: Double, toY: Double,
        durationMs: Int,
        steps: Int,
        modifiers: [String],
        button: MouseInput.Button,
        fromZoom: Bool
    ) async -> CallTool.Result {
        var actualFromX = fromX
        var actualFromY = fromY
        var actualToX = toX
        var actualToY = toY

        if fromZoom {
            guard let zoom = await ImageResizeRegistry.shared.zoom(forPid: pid) else {
                return errorResult(
                    "from_zoom=true but no zoom context for pid \(pid). Call `zoom` first.")
            }
            actualFromX = Double(zoom.originX) + fromX
            actualFromY = Double(zoom.originY) + fromY
            actualToX = Double(zoom.originX) + toX
            actualToY = Double(zoom.originY) + toY
        } else if let ratio = await ImageResizeRegistry.shared.ratio(forPid: pid) {
            actualFromX = fromX * ratio
            actualFromY = fromY * ratio
            actualToX = toX * ratio
            actualToY = toY * ratio
        }

        let startScreen: CGPoint
        let endScreen: CGPoint
        do {
            if let windowId {
                startScreen = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: actualFromX, y: actualFromY),
                    forPid: pid,
                    windowId: windowId)
                endScreen = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: actualToX, y: actualToY),
                    forPid: pid,
                    windowId: windowId)
            } else {
                startScreen = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: actualFromX, y: actualFromY),
                    forPid: pid)
                endScreen = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: actualToX, y: actualToY),
                    forPid: pid)
            }
        } catch let error as WindowCoordinateSpaceError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error resolving window: \(error)")
        }

        // Animate the agent cursor along the drag path so the visual
        // overlay (when enabled) matches the synthesized gesture. The
        // pin-above call keeps the overlay z-stacked over the target
        // app for the whole gesture rather than only at the endpoints.
        await MainActor.run {
            AgentCursor.shared.pinAbove(pid: pid)
        }
        await AgentCursor.shared.animateAndWait(to: startScreen)

        do {
            // Wrap the dispatch in FocusGuard so an app that
            // self-activates mid-gesture (Electron / Calculator-style
            // reflex `NSApp.activate`) gets demoted back to whatever
            // was frontmost before the drag. Drags hold mouseDown for
            // `duration_ms` (default 500) — far longer than a click —
            // so the self-activation race is materially more likely
            // here than for click. Pixel drags have no AX element, so
            // layers 1+2 (enablement / synthetic focus) no-op; only
            // layer 3 (SystemFocusStealPreventer) arms.
            try await AppStateRegistry.focusGuard.withFocusSuppressed(
                pid: pid,
                element: nil
            ) {
                try MouseInput.drag(
                    from: startScreen,
                    to: endScreen,
                    toPid: pid,
                    button: button,
                    durationMs: durationMs,
                    steps: steps,
                    modifiers: modifiers
                )
            }
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            await AgentCursor.shared.animateAndWait(to: endScreen)
            await AgentCursor.shared.finishClick(pid: pid)

            let modSuffix = modifiers.isEmpty ? "" : " with \(modifiers.joined(separator: "+"))"
            let buttonSuffix = button == .left ? "" : " (\(button.rawValue) button)"
            let summary =
                "Posted drag\(buttonSuffix)\(modSuffix) to pid \(pid) "
                + "from window-pixel (\(Int(fromX)), \(Int(fromY))) "
                + "→ (\(Int(toX)), \(Int(toY))), "
                + "screen (\(Int(startScreen.x)), \(Int(startScreen.y))) "
                + "→ (\(Int(endScreen.x)), \(Int(endScreen.y))) "
                + "in \(durationMs)ms / \(steps) steps."
            return CallTool.Result(
                content: [.text(text: "✅ \(summary)", annotations: nil, _meta: nil)]
            )
        } catch let error as MouseInputError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error: \(error)")
        }
    }

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
