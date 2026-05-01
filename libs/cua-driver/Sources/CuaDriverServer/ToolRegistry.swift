import CoreGraphics
import CuaDriverCore
import Foundation
import MCP

public struct ToolHandler: Sendable {
    public let tool: Tool
    public let invoke: @Sendable ([String: Value]?) async throws -> CallTool.Result

    public init(
        tool: Tool,
        invoke: @escaping @Sendable ([String: Value]?) async throws -> CallTool.Result
    ) {
        self.tool = tool
        self.invoke = invoke
    }
}

public struct ToolRegistry: Sendable {
    public let handlers: [String: ToolHandler]

    public init(handlers: [ToolHandler]) {
        self.handlers = Dictionary(uniqueKeysWithValues: handlers.map { ($0.tool.name, $0) })
    }

    public var allTools: [Tool] {
        handlers.values.map(\.tool).sorted { $0.name < $1.name }
    }

    /// Tool names that mutate UI state and should be recorded when
    /// `RecordingSession.shared` is enabled. Excludes read-only tools
    /// (snapshots, enumerators, permission probes) and the recording
    /// toggle itself.
    public static let actionToolNames: Set<String> = [
        "click",
        "right_click",
        "drag",
        "scroll",
        "type_text",
        "press_key",
        "hotkey",
        "set_value",
        "page",
    ]

    /// Click-family tools — their argument set carries a click point we
    /// can render as `click.png`. Other action tools don't have a
    /// natural "dot" location and skip the marker file.
    public static let clickFamilyToolNames: Set<String> = [
        "click",
        "right_click",
    ]

    public func call(_ name: String, arguments: [String: Value]?) async throws -> CallTool.Result {
        guard let handler = handlers[name] else {
            throw MCPError.invalidParams("Unknown tool: \(name)")
        }
        // Capture monotonic start time before any animation or side-effect
        // so the recorded span brackets the full action duration.
        let actionStartNs: UInt64 = Self.actionToolNames.contains(name)
            ? clock_gettime_nsec_np(CLOCK_UPTIME_RAW) : 0

        let result = try await handler.invoke(arguments)

        // Recording hook — runs AFTER the tool's invoke. Errors inside
        // the recorder are swallowed by the actor; the tool caller
        // never sees a recording-path failure.
        if Self.actionToolNames.contains(name),
           await RecordingSession.shared.isEnabled()
        {
            // Bind the shared engine lazily. `bindAppStateEngine` just
            // assigns, so repeat binds are harmless.
            await RecordingSession.shared.bindAppStateEngine(
                AppStateRegistry.engine
            )
            let pid = extractPid(arguments)
            let clickPoint: CGPoint?
            if Self.clickFamilyToolNames.contains(name) {
                clickPoint = await resolveClickPoint(
                    toolName: name, arguments: arguments
                )
            } else {
                clickPoint = nil
            }
            await RecordingSession.shared.record(
                toolName: name,
                arguments: snapshotArguments(arguments),
                pid: pid,
                clickPoint: clickPoint,
                resultSummary: firstTextContent(of: result),
                actionStartNs: actionStartNs
            )
        }

        return result
    }

    // MARK: - Argument extraction helpers

    private func extractPid(_ arguments: [String: Value]?) -> pid_t? {
        guard let raw = arguments?["pid"]?.intValue else { return nil }
        return pid_t(raw)
    }

    /// Pick up the click point for click-family tools, normalized to
    /// screen-absolute points (the space `ClickMarkerRenderer` expects).
    ///
    /// Pixel path: `{x, y}` arrive as window-local screenshot pixels
    /// — same space as the PNG `get_window_state` returns, per the tool
    /// schema. We convert to screen points via the same helper
    /// `ClickTool.performPixelClick` uses, so the recorded dot lines
    /// up with the real click location.
    ///
    /// Element-indexed path: look the element up on the shared engine
    /// scoped to `(pid, window_id)` and take its on-screen center.
    ///
    /// Returns nil when neither is resolvable (the tool handler itself
    /// would have errored in that case, but we shouldn't crash the
    /// recording hook).
    private func resolveClickPoint(
        toolName: String,
        arguments: [String: Value]?
    ) async -> CGPoint? {
        let x = coerceDouble(arguments?["x"])
        let y = coerceDouble(arguments?["y"])
        if let x, let y, let pidRaw = arguments?["pid"]?.intValue {
            do {
                return try WindowCoordinateSpace.screenPoint(
                    fromImagePixel: CGPoint(x: x, y: y),
                    forPid: Int32(pidRaw)
                )
            } catch {
                return nil
            }
        }
        guard let pidRaw = arguments?["pid"]?.intValue,
              let windowIdRaw = arguments?["window_id"]?.intValue,
              let index = arguments?["element_index"]?.intValue
        else {
            return nil
        }
        do {
            let element = try await AppStateRegistry.engine.lookup(
                pid: pid_t(pidRaw),
                windowId: UInt32(windowIdRaw),
                elementIndex: index
            )
            return AXInput.screenCenter(of: element)
        } catch {
            return nil
        }
    }

    private func coerceDouble(_ value: Value?) -> Double? {
        if let d = value?.doubleValue { return d }
        if let i = value?.intValue { return Double(i) }
        return nil
    }

    /// Pull the first text chunk out of a `CallTool.Result` — used as
    /// the recorded "result_summary" field. Non-text content (images,
    /// structured JSON) is ignored; every action tool in the driver
    /// already includes a text summary as its first content item.
    private func firstTextContent(of result: CallTool.Result) -> String {
        for item in result.content {
            if case .text(let text, _, _) = item {
                return text
            }
        }
        return ""
    }

    /// Turn `[String: Value]?` into a JSON-serializable `[String: Any]`
    /// snapshot that can cross the actor boundary into
    /// `RecordingSession`. Unlike the MCP `Value` enum, the converted
    /// dict uses plain Foundation scalar types — readable straight
    /// from `JSONSerialization`.
    private func snapshotArguments(_ arguments: [String: Value]?) -> [String: Any] {
        guard let arguments else { return [:] }
        var out: [String: Any] = [:]
        for (key, value) in arguments {
            out[key] = jsonSafe(value)
        }
        return out
    }

    private func jsonSafe(_ value: Value) -> Any {
        switch value {
        case .null: return NSNull()
        case .bool(let b): return b
        case .int(let i): return i
        case .double(let d): return d
        case .string(let s): return s
        case .data(_, let d): return d.base64EncodedString()
        case .array(let arr): return arr.map { jsonSafe($0) }
        case .object(let obj):
            var out: [String: Any] = [:]
            for (k, v) in obj { out[k] = jsonSafe(v) }
            return out
        }
    }

    /// Lazy accessor to break the static-init cycle between
    /// `ToolRegistry.default` and the tools that need to dispatch back
    /// into the registry (`replay_trajectory`). A direct
    /// `ToolRegistry.default` reference inside a tool handler's init
    /// closure produces "circular reference" errors from Swift's static
    /// analyzer. Wrapping the reference in a function hides it from the
    /// analyzer and lets the runtime resolve it on first call.
    public static func shared() -> ToolRegistry {
        return .default
    }

    public static let `default` = ToolRegistry(handlers: [
        ListAppsTool.handler,
        ListWindowsTool.handler,
        LaunchAppTool.handler,
        GetScreenSizeTool.handler,
        CheckPermissionsTool.handler,
        ScreenshotTool.handler,

        GetCursorPositionTool.handler,
        MoveCursorTool.handler,
        ScrollTool.handler,
        TypeTextTool.handler,
        PressKeyTool.handler,
        HotkeyTool.handler,
        GetWindowStateTool.handler,
        ClickTool.handler,
        DoubleClickTool.handler,
        RightClickTool.handler,
        DragTool.handler,
        SetValueTool.handler,
        SetAgentCursorEnabledTool.handler,
        SetAgentCursorMotionTool.handler,
        SetAgentCursorStyleTool.handler,
        GetAgentCursorStateTool.handler,
        SetRecordingTool.handler,
        GetRecordingStateTool.handler,
        ReplayTrajectoryTool.handler,
        GetConfigTool.handler,
        SetConfigTool.handler,
        ZoomTool.handler,
        PageTool.handler,
    ])
}
