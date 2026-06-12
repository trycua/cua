import AppKit
import QuartzCore
import SwiftUI

/// Named color stop for the agent-cursor's axial stroke gradient.
/// Pairing color + location keeps the spec's lavender stops grep-able
/// in one place — touch these values to retint the pointer.
public struct AgentCursorGradientStop: @unchecked Sendable {
    public let color: NSColor
    public let location: CGFloat
    public init(color: NSColor, location: CGFloat) {
        self.color = color
        self.location = location
    }
}

/// Visual constants for the agent cursor — shape sizing, gradient stops,
/// bloom falloff, stroke widths, and an optional custom image.
///
/// ## Custom cursor image
///
/// Set `image` to an `NSImage` (PNG, JPEG, PDF, or SVG loaded via
/// `NSImage(contentsOf:)`) to replace the default procedural arrow with
/// your own graphic. The image is drawn at `shapeSize × shapeSize` points,
/// rotated to track the motion heading. The bloom halo is still rendered
/// underneath — set `bloomCenterAlpha: 0` to suppress it.
///
/// ## Custom colors
///
/// Override `strokeGradientStops` to change the arrow fill, and
/// `bloomColor` / `bloomCenterAlpha` to change the glow hue. Parse hex
/// strings with `NSColor(hex:)`.
///
/// ## `@unchecked Sendable`
///
/// NSColor and NSImage are reference types without formal Sendable
/// conformance in Swift 6. All fields are `let` (immutable after init),
/// so concurrent reads are safe — hence `@unchecked`.
public struct AgentCursorStyle: @unchecked Sendable {
    /// Container layer size (points). Default 60.
    public let containerSize: CGFloat

    /// Drawn size of the cursor shape (points). Default 22.
    public let shapeSize: CGFloat

    /// Axial stroke gradient stops for the procedural arrow. Ignored when
    /// `image` is set.
    public let strokeGradientStops: [AgentCursorGradientStop]
    public let strokeGradientAngleDegrees: CGFloat

    public let strokeWidth: CGFloat
    public let highlightStrokeWidth: CGFloat

    /// Bloom halo color. Also used for the focus-rect highlight.
    public let bloomColor: NSColor
    public let bloomCenterAlpha: CGFloat
    public let bloomMidAlpha: CGFloat
    public let bloomBreathPeak: CGFloat

    /// Optional custom cursor image. When non-nil, the procedural arrow
    /// is replaced by this image rendered at `shapeSize × shapeSize`
    /// points, rotated to track the motion heading. Accepts any format
    /// NSImage can load: PNG, JPEG, PDF, SVG (macOS 12+).
    ///
    /// Load from a file path:
    /// ```swift
    /// AgentCursorStyle(image: NSImage(contentsOf: URL(fileURLWithPath: "/path/to/cursor.png")))
    /// ```
    public let image: NSImage?

    public init(
        containerSize: CGFloat = 60,
        shapeSize: CGFloat = 22,
        strokeGradientStops: [AgentCursorGradientStop] = AgentCursorStyle.defaultGradientStops,
        strokeGradientAngleDegrees: CGFloat = 135,
        strokeWidth: CGFloat = 2,
        highlightStrokeWidth: CGFloat = 0.5,
        bloomColor: NSColor = NSColor(red: 0x5E / 255, green: 0xC0 / 255, blue: 0xE8 / 255, alpha: 1),
        bloomCenterAlpha: CGFloat = 0.55,
        bloomMidAlpha: CGFloat = 0.15,
        bloomBreathPeak: CGFloat = 0.75,
        image: NSImage? = nil
    ) {
        self.containerSize = containerSize
        self.shapeSize = shapeSize
        self.strokeGradientStops = strokeGradientStops
        self.strokeGradientAngleDegrees = strokeGradientAngleDegrees
        self.strokeWidth = strokeWidth
        self.highlightStrokeWidth = highlightStrokeWidth
        self.bloomColor = bloomColor
        self.bloomCenterAlpha = bloomCenterAlpha
        self.bloomMidAlpha = bloomMidAlpha
        self.bloomBreathPeak = bloomBreathPeak
        self.image = image
    }

    // cua-driver heritage gradient: ice-blue tip → cyan body → mint tail.
    public static let defaultGradientStops: [AgentCursorGradientStop] = [
        AgentCursorGradientStop(
            color: NSColor(red: 0xDB / 255, green: 0xEE / 255, blue: 0xFF / 255, alpha: 1),
            location: 0.0
        ),
        AgentCursorGradientStop(
            color: NSColor(red: 0x5E / 255, green: 0xC0 / 255, blue: 0xE8 / 255, alpha: 1),
            location: 0.53
        ),
        AgentCursorGradientStop(
            color: NSColor(red: 0x54 / 255, green: 0xCD / 255, blue: 0xA0 / 255, alpha: 1),
            location: 1.0
        ),
    ]

    public static let `default` = AgentCursorStyle()
}

// MARK: - NSColor hex parsing

extension NSColor {
    /// Parse a CSS hex color string: `#RGB`, `#RRGGBB`, or `#RRGGBBAA`.
    /// Returns nil when the string is not a valid hex color.
    public convenience init?(hex: String) {
        var str = hex.trimmingCharacters(in: .whitespaces)
        if str.hasPrefix("#") { str = String(str.dropFirst()) }
        if str.count == 3 {
            str = str.map { "\($0)\($0)" }.joined()
        }
        var value: UInt64 = 0
        guard Scanner(string: str).scanHexInt64(&value) else { return nil }
        switch str.count {
        case 6:
            self.init(
                red: CGFloat((value >> 16) & 0xFF) / 255,
                green: CGFloat((value >> 8) & 0xFF) / 255,
                blue: CGFloat(value & 0xFF) / 255,
                alpha: 1
            )
        case 8:
            self.init(
                red: CGFloat((value >> 24) & 0xFF) / 255,
                green: CGFloat((value >> 16) & 0xFF) / 255,
                blue: CGFloat((value >> 8) & 0xFF) / 255,
                alpha: CGFloat(value & 0xFF) / 255
            )
        default:
            return nil
        }
    }
}

/// The agent cursor overlay — a purely visual floating arrow that
/// shows where the agent is "looking" while it works. It does NOT
/// deliver input events; the driver's real clicks continue to run
/// through the AX-element_index path (invisible AX RPC). This overlay
/// is a trust signal: the user sees what the agent is targeting.
///
/// ## Lifecycle
///
/// The overlay is lazy — the window + view are created on first
/// `show()` call and retained for the lifetime of the process. `hide()`
/// orders the window off-screen but doesn't tear down the view tree;
/// the next `show()` is effectively instant.
///
/// ## Threading
///
/// Everything here is `@MainActor`-isolated because AppKit requires it.
/// Call from a `Task { @MainActor in … }` block if you're coming from
/// an async non-main context.
///
/// ## Run-loop prerequisite
///
/// AppKit drawing + CA animations need a live main-thread run loop
/// pumping events. The driver's default stdio-MCP entry point does NOT
/// currently bootstrap `NSApplication.shared.run()` — wire-up for that
/// lives outside this module (a later commit). Until that wire-up is
/// in place, `show()` still succeeds but the cursor won't draw or
/// animate. That's fine for the first commit: the types compile and
/// unit tests can exercise the math without a running screen.
@MainActor
public final class AgentCursor {
    public static let shared = AgentCursor()

    /// Master toggle. When false, `show`/`animate` are no-ops and no
    /// window is created. Defaults to `true` — the driver is
    /// primarily used for user-visible demos where the trust signal
    /// matters more than absolute-silent automation. Headless / CI
    /// callers can opt out with `set_agent_cursor_enabled '{"enabled":false}'`.
    public private(set) var isEnabled: Bool = true

    /// Session-level motion defaults used by `animateAndWait` and
    /// the single-argument overload of `animate`. Tunable at runtime
    /// via the `set_agent_cursor_motion` MCP tool.
    public var defaultMotionOptions: CursorMotionPath.Options = .default

    /// How long each cursor glide takes. Used as the default
    /// `duration` for `animateAndWait`/`animate` when the caller
    /// doesn't pass one. 0.75s reads cleanly in demos even over
    /// short inter-button paths (~50pt); crank it up further for
    /// screen recordings via `set_agent_cursor_motion`.
    public var glideDurationSeconds: CFTimeInterval = 0.75

    /// Post-ripple pause before the tool returns, letting the cursor
    /// visibly rest on the target so a sequence of clicks reads as
    /// deliberate human pacing rather than a blur. Only applied when
    /// the overlay is enabled (invisible automation pays no cost).
    /// 0.4s pairs well with the default glide.
    public var dwellAfterClickSeconds: CFTimeInterval = 0.4

    /// How long the overlay lingers after the last pointer action
    /// before it auto-hides. Each `animateAndWait` / `finishClick`
    /// resets this timer, so a burst of back-to-back clicks keeps the
    /// cursor visible throughout; then it slides off after the driver
    /// has been idle this long. 3s leaves comfortable headroom for a
    /// follow-up action to arrive without the overlay popping in and
    /// out, while still being short enough that the cursor is gone
    /// before the user starts wondering if the agent is still running.
    public var idleHideDelay: TimeInterval = 8.0

    private var overlay: AgentCursorOverlayWindow?
    private var idleHideTask: Task<Void, Never>?

    /// CGWindowID of the target window the overlay is currently
    /// z-pinned above. Cached so consecutive clicks on the same
    /// target skip redundant `NSWindow.order(_:relativeTo:)` calls,
    /// which would otherwise cause a one-frame flash as the window
    /// server re-composites.
    private var pinnedWindowId: Int?

    /// pid of the app the overlay is currently z-pinned above.
    /// The workspace observer re-runs `pinAbove` for this pid
    /// whenever any app activation notification fires, which
    /// catches the async "raise" that macOS processes a few
    /// frames after an AX click returns.
    private var pinnedPid: pid_t?

    /// Observer token for `NSWorkspace.didActivateApplicationNotification`.
    /// Registered lazily on first `pinAbove` and invalidated when
    /// the overlay is hidden or the cursor disabled.
    private var activationObserver: NSObjectProtocol?

    /// Continuous repin loop: fires at ~30 fps while the overlay is
    /// z-pinned to a target. Catches async window-level raises that
    /// macOS issues after AX actions (the target can briefly elevate
    /// above the overlay before any one-shot tick fires). Replaces
    /// the previous sparse [60, 180, 360 …] defensive-repin schedule
    /// — at 33ms the overlay snaps back within one frame rather than
    /// up to 300ms, making the "dip behind" artefact imperceptible.
    ///
    /// `CGWindowListCopyWindowInfo` at 30 fps costs a few hundred
    /// microseconds per call — well within the 33ms budget and much
    /// cheaper than pixel capture.
    private var continuousRepinTask: Task<Void, Never>?

    /// Count of consecutive `reapplyPinAbove` ticks that couldn't
    /// find the pinned pid's on-screen window. Used to tolerate
    /// single-frame misses during target redraw / raise animations
    /// — hiding the overlay on the first miss caused a visible
    /// "cursor disappears for ~1s during click" because mid-raise
    /// window enumerations transiently return no match. `orderOut`
    /// only fires after ≥2 consecutive misses.
    private var missedPinCount: Int = 0

    private init() {}

    /// Enable or disable the cursor. Disabling immediately hides the
    /// overlay and cancels any in-flight animations and idle timers.
    ///
    /// Disable fully tears down the window + view stored properties
    /// (via `close()`, then nils them) so the next `show()` rebuilds
    /// a fresh overlay via `ensureWindow()`. An earlier version only
    /// called `orderOut` and kept the stored references; on a subsequent
    /// enable the retained NSWindow would no longer reliably re-register
    /// with the window server via `orderFront(nil)` — `list_windows`
    /// would return zero windows for the daemon pid and the cursor
    /// would stay invisible through every click. Mirroring the
    /// daemon-restart rebuild path (fresh window) is what keeps the
    /// enable-after-disable path working.
    public func setEnabled(_ enabled: Bool) {
        guard isEnabled != enabled else { return }
        isEnabled = enabled
        if !enabled {
            cancelIdleHide()
            hide()
            tearDownActivationObserver()
            pinnedPid = nil
            // Drop the NSWindow so a later enable + show() rebuilds from
            // scratch. See docstring above.
            overlay?.close()
            overlay = nil
        }
    }

    /// Apply a persisted ``AgentCursorConfig`` to the live singleton.
    /// Used at daemon boot so `AgentCursor.shared` starts in the state
    /// the user last wrote, rather than the compiled-in defaults. Any
    /// future knobs added to `AgentCursorConfig` should propagate here
    /// so the boot-time snapshot is a single source of truth.
    public func apply(config: AgentCursorConfig) {
        setEnabled(config.enabled)
        defaultMotionOptions = CursorMotionPath.Options(
            startHandle: CGFloat(config.motion.startHandle),
            endHandle: CGFloat(config.motion.endHandle),
            arcSize: CGFloat(config.motion.arcSize),
            arcFlow: CGFloat(config.motion.arcFlow),
            spring: CGFloat(config.motion.spring)
        )
        applyStyleConfig(config.style)
    }

    /// Apply a custom visual style to the cursor overlay. Takes effect
    /// immediately — the next rendered frame picks up the new style.
    /// Swift dep users call this directly; MCP users go through
    /// `set_agent_cursor_style`.
    public func setStyle(_ style: AgentCursorStyle) {
        AgentCursorRenderer.shared.style = style
    }

    public func applyStyleConfig(_ styleConfig: AgentCursorConfig.Style) {
        let gradientStops: [AgentCursorGradientStop]
        if let hexColors = styleConfig.gradientColors, !hexColors.isEmpty {
            gradientStops = hexColors.enumerated().compactMap { i, hex -> AgentCursorGradientStop? in
                guard let color = NSColor(hex: hex) else { return nil }
                let loc = CGFloat(i) / CGFloat(max(hexColors.count - 1, 1))
                return AgentCursorGradientStop(color: color, location: loc)
            }
        } else {
            gradientStops = AgentCursorStyle.defaultGradientStops
        }
        let bloomColor = styleConfig.bloomColor.flatMap { NSColor(hex: $0) }
            ?? NSColor(red: 0x5E / 255, green: 0xC0 / 255, blue: 0xE8 / 255, alpha: 1)
        var nsImage: NSImage? = nil
        if let path = styleConfig.imagePath {
            nsImage = NSImage(contentsOf: URL(fileURLWithPath: (path as NSString).expandingTildeInPath))
        }
        setStyle(AgentCursorStyle(
            strokeGradientStops: gradientStops,
            bloomColor: bloomColor,
            image: nsImage
        ))
    }

    /// Show the overlay window. Idempotent — successive calls are
    /// cheap no-ops once the window is already visible. Creates the
    /// window + content view on first call. No-op when disabled.
    ///
    /// Uses `orderFrontRegardless()` rather than `orderFront(nil)`:
    /// the daemon runs under `.accessory` activation policy, and a
    /// freshly-allocated `.floating`-level, clear-background,
    /// borderless NSWindow from an accessory-policy app
    /// `orderFront(nil)`'d doesn't become key-window-eligible →
    /// WindowServer marks it `kCGWindowIsOnscreen = false` even
    /// though we ordered it. Result: the cyan overlay is present in
    /// the window list but not composited, so SCStream capture and
    /// visual rendering both miss it. `orderFrontRegardless()`
    /// forces the on-screen transition without requiring the app
    /// to activate — exactly the semantics we want (backgrounded
    /// overlay that is always visible but never key).
    public func show() {
        guard isEnabled else { return }
        let win = ensureWindow()
        if !win.isVisible {
            win.orderFrontRegardless()
        }
    }

    /// Pin the overlay just above the given pid's frontmost on-screen
    /// window at `.normal` level. `order(.above, relativeTo:)` places
    /// the overlay exactly one slot above the target so any windows
    /// that were already above the target remain above the overlay —
    /// producing `[target, overlay, fg-windows]` z-ordering. Consecutive
    /// clicks on the same target skip redundant re-orders.
    ///
    /// When the target has no on-screen window (hidden launch still
    /// pending, offscreen window, etc.) the overlay is ordered out
    /// after ≥2 consecutive missed ticks — rather than floating over
    /// unrelated apps the user happens to have frontmost.
    public func pinAbove(pid: pid_t) {
        guard isEnabled else { return }
        pinnedPid = pid
        missedPinCount = 0  // fresh pin — any earlier miss streak is stale
        ensureActivationObserver()
        reapplyPinAbove()
        startContinuousRepin()
    }

    /// Start a continuous ~30 fps repin loop for the current target.
    /// Fires every 33ms while `pinnedPid` is set, re-ordering the
    /// overlay just above the target each tick. This catches window-
    /// level raises that macOS issues asynchronously after AX actions
    /// (without the activation notification firing) — the overlay
    /// snaps back within ≤33ms instead of waiting up to 300ms for a
    /// sparse defensive tick.
    ///
    /// The loop exits automatically when `pinnedPid` is cleared
    /// (overlay hidden or target gone) so there's no separate
    /// cancellation needed in the common path. Explicit cancellation
    /// is still done in `hide()` for immediate tear-down.
    private func startContinuousRepin() {
        continuousRepinTask?.cancel()
        continuousRepinTask = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 33_000_000)  // ~30 fps
                guard let self, !Task.isCancelled, self.pinnedPid != nil else { return }
                self.reapplyPinAbove()
            }
        }
    }

    /// Re-run the pin for the most recent `pinnedPid`. Called by
    /// `pinAbove` and by the workspace activation observer.
    private func reapplyPinAbove() {
        guard isEnabled, let pid = pinnedPid else { return }
        let win = ensureWindow()

        // Find target's frontmost on-screen "normal-layer" window
        // (layer == 0). Dock, menu bar, and shields show up at higher
        // layers and aren't what the caller is clicking.
        let targetWindow = WindowEnumerator.visibleWindows()
            .filter { $0.pid == pid && $0.layer == 0 && $0.isOnScreen }
            .max(by: { $0.zIndex < $1.zIndex })

        guard let targetWindow else {
            // Target has no on-screen window — it's minimized, hidden,
            // or on another Space. Drop the overlay entirely rather
            // than floating it above other apps: there's nothing to
            // pin above, and showing a stranded cursor over the user's
            // actual frontmost app is worse than nothing.
            //
            // BUT — a single missed tick is usually just a mid-raise
            // frame where `visibleWindows()` transiently returns no
            // match. Hiding on the first miss caused the overlay to
            // vanish for ~1s during every click. Require ≥2
            // consecutive misses before hiding; the next scheduled
            // repin tick (60–300ms later) will catch the window
            // once it's back on screen and reset the counter.
            missedPinCount += 1
            if missedPinCount >= 2 {
                if win.isVisible { win.orderOut(nil) }
                pinnedWindowId = nil
            }
            return
        }
        missedPinCount = 0
        // Place the overlay just above the target within the `.normal`
        // window level — producing the ordering:
        //   [target-window, overlay, windows-that-were-above-target]
        // This means windows that were already in front of the target
        // (e.g. the user's foreground app) correctly occlude the cursor,
        // making it clear which window the agent is interacting with.
        //
        // The overlay was previously kept at `.floating` so the window
        // server guaranteed it stayed above all `.normal` windows. The
        // downside is that it covered the user's foreground app, making
        // the z-ordering visually misleading for background-window
        // automation. Now at `.normal`, apps above the target remain
        // above the overlay — background interaction stays visually
        // sandwiched where it belongs.
        win.order(.above, relativeTo: targetWindow.id)
        pinnedWindowId = targetWindow.id
    }

    /// Lazily register a `didActivateApplicationNotification`
    /// observer that re-pins whenever any app activates. AX-
    /// dispatched clicks raise the target asynchronously — often
    /// a few frames after `performAction` returns — so a single
    /// post-click `pinAbove` can fire before the raise lands and
    /// leaves the overlay stranded underneath. The activation
    /// notification is the ground-truth signal for "some window
    /// just changed z-order"; re-pinning on every one of them
    /// closes the race at essentially zero cost (observer only
    /// fires on user- and system-level activation events).
    private func ensureActivationObserver() {
        guard activationObserver == nil else { return }
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            // `queue: .main` hops to the main thread, but that's
            // NOT the same as main-actor isolation in Swift
            // concurrency — hop explicitly so we can call the
            // actor-isolated reapplyPinAbove().
            Task { @MainActor in
                self?.reapplyPinAbove()
            }
        }
    }

    private func tearDownActivationObserver() {
        if let obs = activationObserver {
            NSWorkspace.shared.notificationCenter.removeObserver(obs)
            activationObserver = nil
        }
        continuousRepinTask?.cancel()
        continuousRepinTask = nil
    }

    /// Hide the overlay window. No-op if not shown. Keeps the window
    /// retained so the next `show()` is instant.
    ///
    /// Also clears the pin state and stops the continuous repin loop.
    /// Without this cleanup, the loop would keep calling
    /// `reapplyPinAbove`, re-ordering the overlay window back into
    /// the z-stack — visibly resurrecting the cursor the idle-hide
    /// timer just removed.
    public func hide() {
        overlay?.orderOut(nil)
        pinnedWindowId = nil
        pinnedPid = nil
        missedPinCount = 0
        continuousRepinTask?.cancel()
        continuousRepinTask = nil
        clearFocusRect()
    }

    /// Move the cursor immediately to a screen-point coordinate. No
    /// animation. Use `animate(to:duration:)` for smooth motion.
    /// Coordinates are screen points (top-left origin), matching what
    /// `AXUIElement`'s `AXPosition` attribute returns.
    public func setPosition(_ point: CGPoint) {
        AgentCursorRenderer.shared.setInitialPosition(point)
    }

    /// Animate the cursor to `point`, then suspend until the glide is
    /// complete. Use this from tool-invocation sites so the AX action
    /// fires after the user has seen the cursor arrive at the target.
    ///
    /// No-op (returns immediately) when disabled, so call sites don't
    /// need to branch on `isEnabled` — they just `await` unconditionally.
    public func animateAndWait(
        to point: CGPoint,
        duration: CFTimeInterval? = nil,
        options: CursorMotionPath.Options? = nil
    ) async {
        guard isEnabled else { return }
        let duration = duration ?? glideDurationSeconds
        cancelIdleHide()  // incoming activity — defer auto-hide
        show()  // ensure the overlay is visible; no-op if already shown
        animate(to: point)
        // Block until the cursor reaches the endpoint (spring begins).
        // The actual click fires immediately after this returns, so the
        // user sees the cursor land before the AX action dispatches.
        await AgentCursorRenderer.shared.waitForArrival()
    }

    /// Animate the cursor to `point` along a Dubins arc path. The
    /// renderer computes the minimum-turning-radius arc from the current
    /// position to `point` and integrates it forward with a speed
    /// profile and spring settle.
    ///
    /// No-op when disabled.
    public func animate(
        to point: CGPoint,
        duration: CFTimeInterval? = nil,
        options: CursorMotionPath.Options? = nil
    ) {
        guard isEnabled else { return }
        _ = ensureWindow()
        // Always arrive pointing upper-left (45°), approaching from the
        // lower-right — matches the macOS system-cursor convention and
        // gives every click a consistent visual signature regardless of
        // where the cursor started.
        AgentCursorRenderer.shared.moveTo(point: point, endAngleDegrees: 45.0)
    }

    /// Post-click visual beat. Suspends the caller for `duration` so the
    /// cursor visibly rests on the target before the next action fires.
    /// A future iteration can add a SwiftUI ripple drawn in
    /// `AgentCursorView`; for now the dwell time alone is sufficient.
    ///
    /// No-op when disabled.
    public func playClickPress(duration: CFTimeInterval = 0.65) async {
        guard isEnabled else { return }
        try? await Task.sleep(nanoseconds: UInt64(duration * 1_000_000_000))
    }

    /// Mark the "click landed" moment — pauses the caller for the
    /// configured dwell so the cursor visibly rests on the target
    /// before the next flight, then arms the idle-hide timer. No
    /// visual effect; the overlay is just the triangle. Call after
    /// the AX action + post-AX re-pin + press animation. The `pid`
    /// argument is unused here but kept so callers pass the target
    /// context at the click-lifecycle boundary.
    ///
    /// No-op when disabled.
    public func finishClick(pid: pid_t) async {
        _ = pid  // reserved for future per-pid dwell / hide policy
        guard isEnabled else { return }

        // Human-pacing dwell: pause the caller (and thus the next AX
        // action) so the cursor visibly rests on the target before the
        // next flight starts. Keeps back-to-back clicks from reading
        // as a blur.
        if dwellAfterClickSeconds > 0 {
            try? await Task.sleep(
                nanoseconds: UInt64(dwellAfterClickSeconds * 1_000_000_000))
        }

        // "Click landed" is the natural moment to arm the idle timer
        // so the overlay auto-hides if no further clicks arrive within
        // `idleHideDelay`. Any subsequent `animateAndWait` cancels
        // this timer and reschedules, so consecutive clicks keep the
        // overlay visible throughout the burst.
        scheduleIdleHide()
    }

    /// Show a glowing highlight rectangle around the given screen rect.
    /// Used by ClickTool to draw a focus indicator on the targeted AX
    /// element. Pass nil to clear the rect. No-op when disabled.
    public func showFocusRect(_ rect: CGRect?) {
        guard isEnabled else { return }
        AgentCursorRenderer.shared.focusRect = rect
    }

    /// Clear the glowing focus rect. Called by hide() so the rect
    /// doesn't linger after the cursor auto-hides.
    private func clearFocusRect() {
        AgentCursorRenderer.shared.focusRect = nil
    }

    /// For tests + spike code: tear down the window so the next
    /// `show()` rebuilds it from scratch. Not part of the public
    /// tool-surface contract.
    public func resetForTesting() {
        cancelIdleHide()
        overlay?.orderOut(nil)
        overlay = nil
    }

    // MARK: - Private

    /// Arm (or re-arm) the idle auto-hide timer. Cancels any previously
    /// scheduled hide so the most recent activity wins.
    private func scheduleIdleHide() {
        cancelIdleHide()
        let delay = idleHideDelay
        idleHideTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            guard !Task.isCancelled else { return }
            await MainActor.run {
                guard let self else { return }
                // A late-arriving click may have flipped isEnabled off
                // or swapped out the overlay; both branches are safe.
                self.hide()
                self.idleHideTask = nil
            }
        }
    }

    private func cancelIdleHide() {
        idleHideTask?.cancel()
        idleHideTask = nil
    }

    private func ensureWindow() -> AgentCursorOverlayWindow {
        if let overlay { return overlay }
        let win = AgentCursorOverlayWindow()
        let hostView = NSHostingView(rootView: AgentCursorView())
        win.contentView = hostView
        self.overlay = win
        return win
    }
}
