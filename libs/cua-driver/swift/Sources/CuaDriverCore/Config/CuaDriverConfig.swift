import Foundation

/// Persistent, on-disk configuration for the cua-driver daemon. Stored as
/// JSON at `~/Library/Application Support/<app-name>/config.json` so
/// settings survive daemon restarts, unlike the session-scoped
/// `RecordingSession` / live `AgentCursor` state.
///
/// Codable + JSON keys in snake_case via the shared encoder/decoder
/// configuration in `ConfigStore` — so the file on disk reads as
/// `{"schema_version": 1, "agent_cursor": {...}, "capture_mode": "vision"}`
/// even though the Swift properties are `schemaVersion` / `agentCursor`
/// / `captureMode`.
public struct CuaDriverConfig: Codable, Sendable, Equatable {
    /// Bumped when the on-disk schema changes in a non-additive way.
    /// Readers should tolerate higher schema versions by falling back to
    /// defaults and logging a warning (forward compat), and older versions
    /// by running an in-place migration (backward compat).
    public var schemaVersion: Int

    /// Persisted agent-cursor preferences. Applied to `AgentCursor.shared`
    /// at daemon startup; `set_agent_cursor_enabled` and
    /// `set_agent_cursor_motion` mutate the live state AND write through
    /// this field so the next daemon restart boots in the same
    /// configuration.
    public var agentCursor: AgentCursorConfig

    /// Shapes what `get_window_state` returns on each snapshot.
    /// `.som` ("set of marks", default) returns tree + screenshot so
    /// element_index clicks work out of the box and the screenshot is
    /// there for disambiguation. `.vision` returns the window PNG only
    /// — skips the AX walk entirely for vision-first VLM agents that
    /// don't use element_index. `.ax` returns the AX tree only (no PNG,
    /// no screen-capture call) for pure element_index workflows.
    public var captureMode: CaptureMode

    /// Anonymous telemetry opt-out. Default `true` (opt-in) to match
    /// lume's posture. Override at run time via
    /// `CUA_DRIVER_TELEMETRY_ENABLED={0|1}` or mutate persistently via
    /// `cua-driver config telemetry {enable|disable}`.
    public var telemetryEnabled: Bool

    /// Automatic update opt-out. Default `true` (auto-update enabled).
    /// Override at run time via `CUA_DRIVER_AUTO_UPDATE_ENABLED={0|1}`
    /// or mutate persistently via `cua-driver config updates {enable|disable}`.
    public var autoUpdateEnabled: Bool

    /// The default long-side cap applied when a config is absent OR
    /// when a legacy config with `max_image_dimension = 0` is loaded
    /// (see `ConfigStore.load()` migration). Centralized so changing
    /// the number updates both the compiled default and the migrated
    /// value in one place. 1568 matches Anthropic's multimodal
    /// long-side limit.
    public static let defaultMaxImageDimension: Int = 1568

    /// Cap the longest edge of screenshot images to this many pixels.
    /// When a captured image exceeds this, it's resized proportionally
    /// before base64-encoding. The model sees the smaller image and
    /// reasons in its coordinate space; the click tool scales back up
    /// internally when dispatching the AX / CG action.
    ///
    /// Default: 1568 px — matches the long-side limit Anthropic's
    /// multimodal vision pipeline applies to image inputs. By
    /// capping the returned PNG at the same resolution the model
    /// actually receives, we close the "model clicks in thumbnail
    /// space but tool expects native coords" gap: the coordinate
    /// the model reasons about IS the coordinate the tool accepts.
    /// Set to 0 explicitly to opt out and send at native resolution
    /// (useful for pixel-perfect verification flows).
    public var maxImageDimension: Int

    public init(
        schemaVersion: Int = 1,
        agentCursor: AgentCursorConfig = .default,
        captureMode: CaptureMode = .som,
        telemetryEnabled: Bool = true,
        autoUpdateEnabled: Bool = true,
        maxImageDimension: Int = CuaDriverConfig.defaultMaxImageDimension
    ) {
        self.schemaVersion = schemaVersion
        self.agentCursor = agentCursor
        self.captureMode = captureMode
        self.telemetryEnabled = telemetryEnabled
        self.autoUpdateEnabled = autoUpdateEnabled
        self.maxImageDimension = maxImageDimension
    }

    /// Explicit coding keys so the custom `init(from:)` below has a
    /// stable enum to reference. The `.convertFromSnakeCase` strategy
    /// on the shared decoder maps `schema_version` / `agent_cursor` /
    /// `capture_mode` on disk to these camelCase cases.
    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case agentCursor
        case captureMode
        case telemetryEnabled
        case autoUpdateEnabled
        case maxImageDimension
    }

    /// Decoder with per-field fallbacks so an older config file (missing
    /// the `agent_cursor` or `capture_mode` key) decodes cleanly to
    /// defaults rather than failing the entire load. Without this,
    /// adding a new required-by-Codable field would break every user's
    /// existing config.
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.schemaVersion =
            (try? container.decode(Int.self, forKey: .schemaVersion)) ?? 1
        self.agentCursor =
            (try? container.decode(
                AgentCursorConfig.self, forKey: .agentCursor
            )) ?? .default
        self.captureMode =
            (try? container.decode(
                CaptureMode.self, forKey: .captureMode
            )) ?? .som
        self.telemetryEnabled =
            (try? container.decode(Bool.self, forKey: .telemetryEnabled)) ?? true
        self.autoUpdateEnabled =
            (try? container.decode(Bool.self, forKey: .autoUpdateEnabled)) ?? true
        self.maxImageDimension =
            (try? container.decode(Int.self, forKey: .maxImageDimension))
            ?? CuaDriverConfig.defaultMaxImageDimension
    }

    /// The baseline config used when no file exists on disk, the file is
    /// unreadable, or `reset` is invoked.
    public static let `default` = CuaDriverConfig()
}

/// What `get_window_state` includes in its response. "SOM" = "set of
/// marks", the GUI-agent literature term for a combined screenshot +
/// AX-indexed element overlay; each mode lets token-cost-sensitive
/// workflows opt out of one half of the work.
///
///   - `som`    — both tree + screenshot (default). Element-indexed
///     clicks work out of the box and the screenshot is there for
///     disambiguation when labels repeat.
///   - `vision` — window PNG only; `tree_markdown`, `element_count`,
///     and `turn_id` omitted from the response and the AX walk is
///     skipped entirely. Formerly named `screenshot` (the raw string
///     `"screenshot"` still decodes to `.vision` for on-disk config
///     back-compat).
///   - `ax`     — AX tree only; the `screenshot_*` fields are omitted
///     and the screen-capture call is skipped entirely.
public enum CaptureMode: String, Codable, Sendable, Equatable, CaseIterable {
    case vision
    case ax
    case som

    /// Accepts `"screenshot"` as a deprecated alias for `.vision` so a
    /// user's existing `config.json` written under the old name keeps
    /// loading without a migration step. Saves always go through the
    /// synthesized raw-value encoder, so a load-then-save silently
    /// rewrites the on-disk value to `"vision"`.
    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let raw = try container.decode(String.self)
        switch raw {
        case "vision", "screenshot": self = .vision
        case "ax": self = .ax
        case "som": self = .som
        default:
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription:
                    "Unknown capture_mode: \(raw). "
                    + "Expected one of: vision, ax, som."
            )
        }
    }
}

/// Persisted agent-cursor state: the master enable flag plus the five
/// motion knobs that drive the Bezier-arc glide + spring settle. Live
/// state (`AgentCursor.shared`) is initialized from this at daemon boot
/// and the two MCP setters (`set_agent_cursor_enabled`,
/// `set_agent_cursor_motion`) write back through here so the values
/// survive a restart.
///
/// Defaults mirror the compiled-in defaults on `AgentCursor.shared` /
/// `CursorMotionPath.Options` so users who never touch the config see
/// identical behavior to the pre-persistence driver.
public struct AgentCursorConfig: Codable, Sendable, Equatable {
    /// Master toggle. When false, pointer actions skip the cursor
    /// animation entirely. Default: `true` (trust-signal-first posture).
    public var enabled: Bool

    /// Five motion knobs matching `CursorMotionPath.Options`. Nested so
    /// the JSON groups naturally under `"motion": {...}` instead of
    /// five sibling fields fighting for space with `enabled`.
    public var motion: Motion

    /// Persisted cursor style overrides. Applied to `AgentCursorRenderer.shared`
    /// at daemon startup via `AgentCursor.shared.apply(config:)`.
    public var style: Style

    public struct Motion: Codable, Sendable, Equatable {
        /// Start-handle fraction along the straight line, in `[0, 1]`.
        public var startHandle: Double
        /// End-handle fraction measured from the end, in `[0, 1]`.
        public var endHandle: Double
        /// Perpendicular deflection as a fraction of path length.
        public var arcSize: Double
        /// Asymmetry bias in `[-1, 1]`.
        public var arcFlow: Double
        /// Settle damping in `[0.3, 1]`.
        public var spring: Double

        public init(
            startHandle: Double = 0.3,
            endHandle: Double = 0.3,
            arcSize: Double = 0.25,
            arcFlow: Double = 0.0,
            spring: Double = 0.72
        ) {
            self.startHandle = startHandle
            self.endHandle = endHandle
            self.arcSize = arcSize
            self.arcFlow = arcFlow
            self.spring = spring
        }

        public static let `default` = Motion()
    }

    /// Persisted visual style overrides. Only non-nil fields are applied;
    /// nil fields fall back to `AgentCursorStyle.default`.
    public struct Style: Codable, Sendable, Equatable {
        /// Gradient color stops as CSS hex strings (#RRGGBB / #RGB).
        /// When set, replaces the default ice-blue→cyan→mint gradient.
        public var gradientColors: [String]?

        /// Bloom halo color as a CSS hex string. When set, also tints the
        /// focus-rect highlight drawn around clicked AX elements.
        public var bloomColor: String?

        /// Absolute or `~`-rooted path to a PNG, JPEG, PDF, or SVG file
        /// that replaces the default procedural arrow shape.
        public var imagePath: String?

        public init(
            gradientColors: [String]? = nil,
            bloomColor: String? = nil,
            imagePath: String? = nil
        ) {
            self.gradientColors = gradientColors
            self.bloomColor = bloomColor
            self.imagePath = imagePath
        }

        public static let `default` = Style()
    }

    private enum CodingKeys: String, CodingKey {
        case enabled
        case motion
        case style
    }

    public init(
        enabled: Bool = true,
        motion: Motion = .default,
        style: Style = .default
    ) {
        self.enabled = enabled
        self.motion = motion
        self.style = style
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.enabled = (try? container.decode(Bool.self, forKey: .enabled)) ?? true
        self.motion = (try? container.decode(Motion.self, forKey: .motion)) ?? .default
        self.style = (try? container.decode(Style.self, forKey: .style)) ?? .default
    }

    public static let `default` = AgentCursorConfig()
}
