import AppKit

/// Transparent, click-through, borderless window used to host the agent
/// cursor overlay. Shares the classic "floating HUD" recipe: no window
/// chrome, no shadow, clear background, ignores mouse events so the user
/// keeps using the machine normally, installed at `.statusBar + 1` so
/// it stays above ordinary windows but below system menus and modals.
///
/// `canBecomeKey` / `canBecomeMain` are overridden to `false` so the
/// window never becomes the focus target — critical to preserve the
/// driver's "never steal focus" contract.
///
/// Sized to cover the main display only. Multi-display support is
/// intentionally degraded here: on setups with a second monitor
/// arranged above/beside the main, an `NSScreen.screens` union frame
/// lands the `NSWindow` entirely off the visible area (observed
/// `CGWindowBounds` at `y=-2062` with `onscreen=false`), so the cyan
/// cursor never renders to the user or to `SCStream` capture. The
/// correct fix is one overlay per `NSScreen`; tracked as a follow-up.
/// Until then, the agent cursor is visible only while it is over the
/// main display — acceptable because users watch the main display
/// during recording.
public final class AgentCursorOverlayWindow: NSWindow {
    public override var canBecomeKey: Bool { false }
    public override var canBecomeMain: Bool { false }

    public convenience init() {
        let frame = AgentCursorOverlayWindow.mainScreenFrame()
        self.init(
            contentRect: frame,
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        ignoresMouseEvents = true
        // `.normal` level so the overlay is sandwiched in the regular
        // window stack. `AgentCursor.pinAbove(pid:)` calls
        // `order(.above, relativeTo: targetWindowId)` to place the
        // overlay exactly one slot above the target window — windows
        // that were already above the target remain above the overlay.
        // This produces the ordering: [target, overlay, fg-windows].
        level = .normal
        collectionBehavior = [
            .canJoinAllSpaces, .fullScreenAuxiliary, .stationary,
        ]
        isReleasedWhenClosed = false
        // The overlay must stay visible even when `cua-driver` itself
        // isn't the active application. Without this, the window would
        // disappear any time the driver loses focus (which is every
        // tool call, given our no-focus-steal contract).
        hidesOnDeactivate = false
    }

    /// The main display's frame in AppKit global coordinates. Used as
    /// the overlay window's frame. A previous implementation used the
    /// union of `NSScreen.screens` to get a continuous coordinate
    /// space across every connected display, but on multi-display
    /// setups (especially when a secondary monitor is arranged above
    /// the main) the resulting `NSWindow` landed entirely off-screen
    /// — `CGWindowIsOnscreen=false`, bounds above `y=0` in CGWindow
    /// coords — so the overlay never rendered. Anchoring to the main
    /// screen always produces an on-screen window; the tradeoff is
    /// that the cursor is clipped when it glides to a non-main
    /// display. Proper multi-display support (one overlay per screen)
    /// is tracked as a follow-up.
    private static func mainScreenFrame() -> NSRect {
        return NSScreen.main?.frame ?? NSScreen.screens.first?.frame ?? .zero
    }
}
