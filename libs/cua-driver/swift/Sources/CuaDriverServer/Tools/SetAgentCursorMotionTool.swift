import CuaDriverCore
import Foundation
import MCP

/// Tune the five knobs the agent cursor uses for its Bezier-arc +
/// spring-settle motion. All fields optional — pass only the knobs
/// you want to change. Missing knobs keep their current value.
///
/// Session-scoped: the new values apply to every subsequent
/// `click` / `right_click` until the next call to this tool or
/// until the MCP session ends.
public enum SetAgentCursorMotionTool {
    public static let handler = ToolHandler(
        tool: Tool(
            name: "set_agent_cursor_motion",
            description: """
                Tune the agent cursor's motion curve. All fields optional;
                only the knobs you pass change, the rest keep their
                current value. The knobs map to the cubic Bezier that the
                cursor traces between successive positions, plus the
                spring animation that settles the cursor's rotation on
                arrival.

                - start_handle: how far along the straight line the
                  first control point sits, as a fraction in [0, 1].
                  Lower = tight departure, higher = floppy departure.
                - end_handle: same, measured from the end point.
                - arc_size: perpendicular deflection as a fraction of
                  path length. 0 = straight line. 0.25 = modest arc.
                  0.5 = dramatic swoop. Typical: 0.25.
                - arc_flow: asymmetry bias in [-1, 1]. Positive bulges
                  the arc toward the destination; negative toward the
                  origin; 0 is symmetric.
                - spring: settle damping in [0.3, 1.0]. 1.0 = critically
                  damped (no overshoot); 0.4 = bouncy. Typical: 0.72.
                - glide_duration_ms: wall-clock duration of each cursor
                  flight in milliseconds. Higher = slower, more
                  visible for demos / screen recordings. Typical: 750.
                - dwell_after_click_ms: pause the tool holds after
                  the click ripple before returning, letting the
                  cursor visibly rest on the target so consecutive
                  clicks read as human pacing. Only applied while
                  the cursor is enabled. Typical: 400.
                - idle_hide_ms: how long the overlay lingers after
                  the last pointer action before auto-hiding.
                  Re-armed by every click, so a burst of consecutive
                  actions keeps the cursor visible throughout. Higher
                  = more tolerant of follow-up actions without the
                  overlay popping in and out. Typical: 3000.

                Defaults: start_handle=0.3, end_handle=0.3, arc_size=0.25,
                arc_flow=0.0, spring=0.72, glide_duration_ms=750,
                dwell_after_click_ms=400, idle_hide_ms=3000.
                """,
            inputSchema: [
                "type": "object",
                "properties": [
                    "start_handle": [
                        "type": "number",
                        "description": "Start-handle fraction in [0, 1]. Default 0.3.",
                    ],
                    "end_handle": [
                        "type": "number",
                        "description": "End-handle fraction in [0, 1]. Default 0.3.",
                    ],
                    "arc_size": [
                        "type": "number",
                        "description": "Arc deflection as fraction of path length. Default 0.25.",
                    ],
                    "arc_flow": [
                        "type": "number",
                        "description": "Asymmetry bias in [-1, 1]. Default 0.",
                    ],
                    "spring": [
                        "type": "number",
                        "description": "Settle damping in [0.3, 1]. Default 0.72.",
                    ],
                    "glide_duration_ms": [
                        "type": "number",
                        "description":
                            "Flight duration per click in ms. Higher = slower. Default 750.",
                        "minimum": 50,
                        "maximum": 5000,
                    ],
                    "dwell_after_click_ms": [
                        "type": "number",
                        "description":
                            "Pause after the click ripple in ms, for human-like pacing. Default 400.",
                        "minimum": 0,
                        "maximum": 5000,
                    ],
                    "idle_hide_ms": [
                        "type": "number",
                        "description":
                            "How long the overlay lingers after the last click before auto-hiding, in ms. Default 3000.",
                        "minimum": 100,
                        "maximum": 60000,
                    ],
                ],
                "additionalProperties": false,
            ],
            annotations: .init(
                readOnlyHint: false,
                destructiveHint: false,
                idempotentHint: true,
                openWorldHint: false
            )
        ),
        invoke: { arguments in
            // JSON numbers without a decimal point parse as `.int` from
            // the MCP Value type, which means `.doubleValue` would reject
            // them — a caller writing `{"glide_duration_ms": 1500}` gets
            // silently ignored without this coercion.
            func number(_ value: Value?) -> Double? {
                if let i = value?.intValue { return Double(i) }
                return value?.doubleValue
            }
            let (updated, glideMs, dwellMs, idleMs) = await MainActor.run {
                () -> (CursorMotionPath.Options, Double, Double, Double) in
                var opts = AgentCursor.shared.defaultMotionOptions
                if let v = number(arguments?["start_handle"]) { opts.startHandle = v }
                if let v = number(arguments?["end_handle"]) { opts.endHandle = v }
                if let v = number(arguments?["arc_size"]) { opts.arcSize = v }
                if let v = number(arguments?["arc_flow"]) { opts.arcFlow = v }
                if let v = number(arguments?["spring"]) { opts.spring = v }
                AgentCursor.shared.defaultMotionOptions = opts
                if let ms = number(arguments?["glide_duration_ms"]) {
                    AgentCursor.shared.glideDurationSeconds = ms / 1000.0
                }
                if let ms = number(arguments?["dwell_after_click_ms"]) {
                    AgentCursor.shared.dwellAfterClickSeconds = ms / 1000.0
                }
                if let ms = number(arguments?["idle_hide_ms"]) {
                    AgentCursor.shared.idleHideDelay = ms / 1000.0
                }
                return (
                    opts,
                    AgentCursor.shared.glideDurationSeconds * 1000,
                    AgentCursor.shared.dwellAfterClickSeconds * 1000,
                    AgentCursor.shared.idleHideDelay * 1000
                )
            }

            // Persist the five Bezier/spring knobs so the next daemon
            // restart re-applies them. Durations (`glide_duration_ms`,
            // `dwell_after_click_ms`, `idle_hide_ms`) are intentionally
            // session-only for now — they're more tuning knobs than
            // preferences and don't belong in the persistent schema
            // without a clearer use case.
            do {
                try await ConfigStore.shared.mutate { config in
                    config.agentCursor.motion = AgentCursorConfig.Motion(
                        startHandle: Double(updated.startHandle),
                        endHandle: Double(updated.endHandle),
                        arcSize: Double(updated.arcSize),
                        arcFlow: Double(updated.arcFlow),
                        spring: Double(updated.spring)
                    )
                }
            } catch {
                return CallTool.Result(
                    content: [
                        .text(
                            text:
                                "Agent cursor motion updated live, but persisting to config failed: \(error.localizedDescription)",
                            annotations: nil, _meta: nil
                        )
                    ],
                    isError: true
                )
            }

            let summary =
                "cursor motion: startHandle=\(updated.startHandle)"
                + " endHandle=\(updated.endHandle)"
                + " arcSize=\(updated.arcSize)"
                + " arcFlow=\(updated.arcFlow)"
                + " spring=\(updated.spring)"
                + " glideDurationMs=\(Int(glideMs))"
                + " dwellAfterClickMs=\(Int(dwellMs))"
                + " idleHideMs=\(Int(idleMs))"
            return CallTool.Result(
                content: [.text(text: "✅ \(summary)", annotations: nil, _meta: nil)]
            )
        }
    )
}
