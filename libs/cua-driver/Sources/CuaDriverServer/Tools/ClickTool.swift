import AppKit
import ApplicationServices
import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

/// Unified left-click primitive. Two addressing modes, pid always
/// required:
///
/// - `element_index` from the last `get_window_state(pid, window_id)` →
///   wraps `AXPerformAction(action ?? "press")` against the cached
///   element in `FocusGuard.withFocusSuppressed`. Pure AX RPC, works
///   on backgrounded / hidden windows, no cursor move or focus
///   steal. This is the reliable path — prefer it whenever an AX
///   target exists.
///
/// - `x, y` window-local screenshot pixels → synthesizes mouse events
///   via `MouseInput.click` and delivers them to the pid. AX is never
///   consulted on this path — pixel coordinates always route through
///   CGEvent / SkyLight unconditionally. Supports `modifier` for
///   ctrl / cmd-click sequences.
///
/// Exactly one mode must be supplied. Missing pid, both modes, or
/// neither mode returns `isError` before any AX / event work
/// happens. `action` is only valid in the AX path; `count` and
/// `modifier` only take effect in the pixel path (AX actions have
/// no "double" concept and don't propagate modifier keys).
public enum ClickTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "click",
            description: """
                Left-click against a target pid. Two addressing modes:

                - `element_index` + `window_id` (from the last
                  `get_window_state` snapshot of that window) —
                  performs an AX action on the cached element. Default
                  `action` is `press` (left-click). Other values:
                  `show_menu` (right-click equivalent), `pick` (open a
                  menu-bar item's submenu), `confirm` (return on a
                  default button), `cancel` (escape on a dismiss
                  button), `open` (open a file/folder in a browser).
                  Pure AX RPC, works on backgrounded / hidden windows,
                  no cursor move or focus steal. Requires a prior
                  `get_window_state(pid, window_id)` in this turn; the
                  element_index cache is scoped per (pid, window_id) so
                  indices from one window do not resolve against another.

                - `x`, `y` (window-local screenshot pixels, top-left
                  origin of the PNG returned by `get_window_state`) —
                  synthesizes mouse events via CGEvent / SkyLight and
                  delivers them to the pid. AX is never consulted on
                  this path. The driver converts image-pixel →
                  screen-point internally using the target's window
                  origin and backing scale. `count: 2` posts two
                  down/up pairs ~80ms apart for a double-click.
                  `modifier` holds cmd/shift/option/ctrl during the
                  click (e.g. ["cmd"] for cmd-click).

                Exactly one of `element_index` or (`x` AND `y`) must be
                provided. `pid` is required in both modes. `window_id`
                is required when `element_index` is used (scopes the
                cache lookup). `action` is only valid with
                `element_index`; `count` and `modifier` are ignored in
                the AX path.
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
                            "Element index from the last get_window_state for the same (pid, window_id). Routes through the AX action path. Requires window_id.",
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
                    "action": [
                        "type": "string",
                        "enum": ["press", "show_menu", "pick", "confirm", "cancel", "open"],
                        "description":
                            "AX action name (element_index path only). Default: press.",
                    ],
                    "modifier": [
                        "type": "array",
                        "items": ["type": "string"],
                        "description":
                            "Modifier keys held during the click: cmd/shift/option/ctrl. Pixel path only.",
                    ],
                    "count": [
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 3,
                        "description":
                            "Click count — 1 (single), 2 (double), 3 (triple). Pixel path only. Default: 1.",
                    ],
                    "from_zoom": [
                        "type": "boolean",
                        "description":
                            "When true, x and y are pixel coordinates in the last `zoom` image for this pid. The driver automatically maps them back to the correct window coordinates. Use this after calling `zoom` to click on something you saw in the zoomed image.",
                    ],
                    "debug_image_out": [
                        "type": "string",
                        "description":
                            "Optional absolute path. When set on a pixel-addressed click (x, y present), the tool captures a fresh screenshot of the target window at the current max_image_dimension, draws a red crosshair at the received (x, y), and writes the PNG to this path. Use this to verify the tool received the coordinate you intended — if you drew your own blue crosshair on the PNG from get_window_state and saved it separately, the two crosshairs must land on the same pixel. Mismatch surfaces a coord-space bug. Pixel path only; requires window_id; incompatible with from_zoom.",
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
                    "Provide element_index or (x, y) to address the click target.")
            }
            if elementIndex != nil && rawWindowId == nil {
                return errorResult(
                    "window_id is required when element_index is used — the "
                    + "element_index cache is scoped per (pid, window_id). Pass "
                    + "the same window_id you used in `get_window_state`.")
            }

            let actionName = arguments?["action"]?.stringValue
            if actionName != nil && hasXY {
                return errorResult(
                    "action only applies with element_index, not (x, y).")
            }

            let modifiers = arguments?["modifier"]?.arrayValue?.compactMap {
                $0.stringValue
            } ?? []
            let count = arguments?["count"]?.intValue ?? 1
            let fromZoom = arguments?["from_zoom"]?.boolValue ?? false
            let debugImageOut = arguments?["debug_image_out"]?.stringValue

            if debugImageOut != nil {
                if elementIndex != nil {
                    return errorResult(
                        "debug_image_out only applies to pixel clicks (x, y); "
                        + "element_index clicks don't have a coordinate to verify.")
                }
                if fromZoom {
                    return errorResult(
                        "debug_image_out is incompatible with from_zoom — "
                        + "the received (x, y) would be in zoom-crop space, "
                        + "not window-local, so the crosshair would misplace.")
                }
                if rawWindowId == nil {
                    return errorResult(
                        "debug_image_out requires window_id — the tool needs a "
                        + "window to capture for the crosshair overlay.")
                }
            }

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
                return await performElementClick(
                    pid: pid,
                    windowId: windowId,
                    index: index,
                    actionName: actionName ?? "press")
            }
            return await performPixelClick(
                pid: pid,
                windowId: windowId,
                x: x!,
                y: y!,
                count: count,
                modifiers: modifiers,
                fromZoom: fromZoom,
                debugImageOut: debugImageOut
            )
        }
    )

    // MARK: - Element-indexed path

    private static func performElementClick(
        pid: Int32, windowId: UInt32, index: Int, actionName: String
    ) async -> CallTool.Result {
        guard let axAction = axActionByName[actionName] else {
            return errorResult("Unknown action: \(actionName).")
        }
        do {
            let element = try await AppStateRegistry.engine.lookup(
                pid: pid,
                windowId: windowId,
                elementIndex: index
            )
            // Capture the advertised AX actions BEFORE dispatching —
            // `AXUIElementPerformAction` returns `.success` even when
            // the element doesn't advertise the requested action, so
            // without this we have no way to detect a silent no-op.
            // See the post-dispatch warning assembled below.
            let advertisedActions = AXInput.advertisedActionNames(of: element)
            // Bail early if the element is disabled (AXEnabled = false).
            // AXUIElementPerformAction returns .success even on disabled
            // elements — the call reaches the app's AX layer but the app's
            // command-dispatch silently discards it (Chrome marks Developer
            // submenu items as disabled via commandDispatch when it isn't
            // frontmost). Surfacing this as an error rather than a silent
            // no-op lets the caller decide whether to activate the app first.
            if AXInput.boolAttribute("AXEnabled", of: element) == false {
                let target = AXInput.describe(element)
                let appName = NSRunningApplication(processIdentifier: pid)?
                    .localizedName ?? "the app"
                var msg = "❌ [\(index)] \(target.role ?? "element") \"\(target.title ?? "")\" is disabled (AXEnabled = false) — action would be a silent no-op."
                msg += "\n\nThis usually means the target app is not frontmost."
                msg += " Activate it first, then re-snapshot and retry:"
                msg += "\n  osascript -e 'tell application \"\(appName)\" to activate'"
                return errorResult(msg)
            }
            // Animate the visual agent cursor to the target before
            // firing the AX action. `animateAndWait` is a no-op
            // when the cursor is disabled (the default) or when
            // the element has no resolvable position (offscreen /
            // hidden menu items), so it's safe to call unconditionally.
            if let center = AXInput.screenCenter(of: element) {
                // Keep the overlay z-pinned to the target app so
                // unrelated windows stacked over the target
                // correctly occlude the cursor.
                await MainActor.run {
                    AgentCursor.shared.pinAbove(pid: pid)
                }
                await AgentCursor.shared.animateAndWait(to: center)
            }
            try await AppStateRegistry.focusGuard.withFocusSuppressed(
                pid: pid,
                element: element
            ) {
                try AXInput.performAction(axAction, on: element)
            }
            // For text fields (AXTextField / AXTextArea), WebKit establishes
            // DOM focus asynchronously after AXPress returns. Without a pause,
            // a follow-up type_text_chars call races with WebKit's focus setup
            // and sends chars before the input is active — chars are silently
            // dropped. This is especially pronounced for email/number inputs
            // when the app is backgrounded (Safari is non-frontmost).
            //
            // Empirically, 800 ms is reliably sufficient: a direct Python
            // integration test showed immediate click+type fails but click +
            // 2 s + type succeeds; 800 ms gives comfortable margin while
            // keeping the UX fast enough for interactive use.
            let target = AXInput.describe(element)
            if axAction == "AXPress",
               let role = target.role,
               role == "AXTextField" || role == "AXTextArea"
            {
                try? await Task.sleep(for: .milliseconds(800))
            }
            // AX-dispatched clicks can raise the target window to
            // the top of its level, leaving the overlay stranded
            // beneath it. Re-pin BEFORE the press-in / dwell so
            // the cursor is actually visible on top of the target
            // during both — otherwise the press animation plays
            // behind the app and looks like the cursor "dips
            // under" for the click duration.
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            // If the element has a bounding rect, show a glowing focus
            // highlight on the cursor overlay so the user can see which
            // element the agent is targeting.
            if let rect = AXInput.screenBoundingRect(of: element) {
                await MainActor.run {
                    AgentCursor.shared.showFocusRect(rect)
                }
            }
            // Press-in / release pulse on the cursor — purely
            // visual confirmation that the click fired.
            // No-op when disabled.
            await AgentCursor.shared.playClickPress()
            // Let the cursor rest on the target for the dwell
            // period and arm the idle-hide timer. No-op when
            // disabled.
            await AgentCursor.shared.finishClick(pid: pid)
            var summary =
                "✅ Performed \(axAction) on [\(index)] \(target.role ?? "?") \"\(target.title ?? "")\"."
            // For popup buttons (HTML <select> elements in Safari/WebKit):
            // the native macOS popup menu that AXPress opens immediately
            // closes when the app is non-frontmost, so the visible options
            // are never selectable from the keyboard. Instead, list the
            // available options from the AX children and direct the caller
            // to use set_value — which AX-presses the specific child option
            // directly, bypassing the native menu entirely.
            if target.role == "AXPopUpButton" {
                let children = AXInput.children(of: element)
                let options: [(title: String, value: String)] = children.compactMap { child in
                    let t = AXInput.stringAttribute("AXTitle", of: child) ?? ""
                    let v = AXInput.stringAttribute("AXValue", of: child) ?? ""
                    guard !t.isEmpty || !v.isEmpty else { return nil }
                    return (title: t, value: v)
                }
                if !options.isEmpty {
                    let optList = options.map { o in
                        o.value.isEmpty || o.value == o.title ? "\"\(o.title)\"" : "\"\(o.title)\" (value: \(o.value))"
                    }.joined(separator: ", ")
                    summary += "\n\n⚠️ This is a popup/select button. The native macOS menu"
                    summary += " closes immediately when the window is in the background."
                    summary += " Do NOT use click again — instead, use:"
                    summary += "\n  set_value(pid: \(pid), window_id: \(windowId), element_index: \(index),"
                    summary += " value: \"<option title>\")"
                    summary += "\nAvailable options: [\(optList)]"
                }
            }
            // If the element didn't advertise the action we just
            // dispatched, append a non-fatal warning. The AX call
            // returned `.success` but the element almost certainly
            // no-op'd — common with `AXLink`s that expose
            // `AXShowMenu` / `AXScrollToVisible` but not `AXPress`.
            // Stay a success (not isError) — callers can still
            // decide based on the re-snapshot.
            if !advertisedActions.contains(axAction) {
                let advertisedList = advertisedActions.isEmpty
                    ? "none"
                    : advertisedActions.joined(separator: ", ")
                summary += "\n⚠️ Element does not advertise \(axAction)"
                summary += " (actions: \(advertisedList))."
                summary += " Action may have been a no-op."
                summary += " Retry with a pixel click (`click(pid, x, y)`)"
                summary += " if the expected state change didn't happen."
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

    /// Build the error message surfaced when `lookup(pid:windowId:elementIndex:)`
    /// misses. If no daemon is listening on the UDS the caller almost
    /// certainly ran `cua-driver click` in-process, which starts with
    /// an empty `AppStateRegistry.engine` cache — prepend a hint to
    /// start the daemon before calling element-indexed actions, since
    /// the default `AppStateError.noCachedState` description assumes
    /// the cache is live and just needs a `get_window_state` refresh.
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

    private static func performPixelClick(
        pid: Int32,
        windowId: UInt32?,
        x: Double,
        y: Double,
        count: Int,
        modifiers: [String],
        fromZoom: Bool = false,
        debugImageOut: String? = nil
    ) async -> CallTool.Result {
        // Write the debug crosshair BEFORE any coordinate mangling —
        // we want the saved image to reflect the exact (x, y) the
        // caller handed us, in the same resized space the caller
        // was reasoning in. The crosshair lands on the received
        // pixel; the caller compares against their own "intent"
        // crosshair to spot coord-space mismatches.
        if let debugPath = debugImageOut, let windowId {
            let config = await ConfigStore.shared.load()
            do {
                try await DebugCrosshair.writeCrosshair(
                    windowID: windowId,
                    point: CGPoint(x: x, y: y),
                    maxImageDimension: config.maxImageDimension,
                    path: debugPath
                )
            } catch {
                // Don't abort the click just because the debug
                // artifact couldn't be written — the user asked for
                // a click, not a screenshot. Surface the error in
                // the response text so they can investigate, but
                // proceed with dispatch.
                return errorResult(
                    "debug_image_out write failed: \(error). "
                    + "Not dispatching click — fix the path and retry."
                )
            }
        }

        // Resolve x,y to native window-pixel coordinates.
        var actualX = x
        var actualY = y

        if fromZoom {
            guard let zoom = await ImageResizeRegistry.shared.zoom(forPid: pid) else {
                return errorResult(
                    "from_zoom=true but no zoom context for pid \(pid). Call `zoom` first.")
            }
            // x,y are in the zoom crop's pixel space; add crop origin to
            // get full-window native-pixel coordinates.
            actualX = Double(zoom.originX) + x
            actualY = Double(zoom.originY) + y
        } else if let ratio = await ImageResizeRegistry.shared.ratio(forPid: pid) {
            // x,y are in the resized image space; scale up to native pixels.
            actualX = x * ratio
            actualY = y * ratio
        }

        let screenPoint: CGPoint
        do {
            if let windowId {
                screenPoint = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: actualX, y: actualY),
                    forPid: pid,
                    windowId: windowId)
            } else {
                screenPoint = try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: actualX, y: actualY), forPid: pid)
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
                count: count,
                modifiers: modifiers
            )
            await MainActor.run {
                AgentCursor.shared.pinAbove(pid: pid)
            }
            await AgentCursor.shared.playClickPress()
            await AgentCursor.shared.finishClick(pid: pid)
            let clickWord = count == 2 ? "double-click" : (count == 3 ? "triple-click" : "click")
            let modSuffix = modifiers.isEmpty ? "" : " with \(modifiers.joined(separator: "+"))"
            return CallTool.Result(
                content: [.text(text: "✅ Posted \(clickWord)\(modSuffix) to pid \(pid).", annotations: nil, _meta: nil)]
            )
        } catch let error as MouseInputError {
            return errorResult(error.description)
        } catch {
            return errorResult("Unexpected error: \(error)")
        }
    }

    // MARK: - Helpers

    private static let axActionByName: [String: String] = [
        "press": "AXPress",
        "show_menu": "AXShowMenu",
        "pick": "AXPick",
        "confirm": "AXConfirm",
        "cancel": "AXCancel",
        "open": "AXOpen",
    ]

    /// JSON numbers without a decimal point parse as `.int` from the
    /// MCP Value type. Callers writing `{"x": 100}` would be silently
    /// rejected by `.doubleValue` alone, so promote `.int` here too.
    private static func coerceDouble(_ value: Value?) -> Double? {
        if let d = value?.doubleValue { return d }
        if let i = value?.intValue { return Double(i) }
        return nil
    }

    /// Check if all of the target pid's windows are off-screen
    /// (minimized or hidden). Uses CGWindowList to catch the case
    /// where Chrome's AX tree hides minimized windows entirely.
    private static func isWindowMinimized(pid: Int32) -> Bool {
        guard let onScreen = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements],
            kCGNullWindowID
        ) as? [[String: Any]] else { return false }

        let hasOnScreen = onScreen.contains {
            ($0[kCGWindowOwnerPID as String] as? Int32) == pid
        }
        if hasOnScreen { return false }

        // No on-screen window — check if there are any windows at all.
        guard let all = CGWindowListCopyWindowInfo(
            [.optionAll], kCGNullWindowID
        ) as? [[String: Any]] else { return false }

        let hasAny = all.contains {
            ($0[kCGWindowOwnerPID as String] as? Int32) == pid
            && ($0[kCGWindowLayer as String] as? Int32) == 0
        }
        return hasAny  // has off-screen windows but none on-screen → minimized
    }

    private static func errorResult(_ message: String) -> CallTool.Result {
        CallTool.Result(
            content: [.text(text: message, annotations: nil, _meta: nil)],
            isError: true
        )
    }
}
