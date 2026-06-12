//! Cross-platform contract for keeping the agent-cursor overlay at z+1 of
//! the application under test.
//!
//! Each platform crate owns a concrete [`ZOrderEnforcer`] that holds the
//! overlay's native window handle and is driven by the platform's existing
//! tick cadence (Win32 `WM_TIMER`, macOS render frame, X11 render thread).
//! `reassert` is the single named seam where a future OS-event-driven
//! observer (`SetWinEventHook(EVENT_OBJECT_REORDER)`, AppKit
//! `NSWindowDidChangeOcclusionState`, X11 `SubstructureNotifyMask`) can
//! replace the polling implementation without touching the render loops.
//!
//! ## Contract
//!
//! - `target = Some(wid)` — the overlay must sit **one stacking position
//!   above** the native window identified by `wid` (HWND root on Windows,
//!   `CGWindowID` on macOS, X11 XID on Linux). If a third window is
//!   currently above the target, the overlay must appear **below** it so
//!   the user's foreground keeps rendering on top of the synthetic cursor.
//!
//! - `target = None` — the overlay must sit at the top of the **non-topmost**
//!   band. It must never be promoted into the OS "always-on-top" band
//!   (`HWND_TOPMOST`, `NSStatusWindowLevel`, override-redirect raise).
//!
//! - `wid` no longer maps to a live window — fall back to the `None`
//!   behaviour for this tick. Do not error; the next tick may have a
//!   new target.
//!
//! - Implementations must be cheap and idempotent. `reassert` is called
//!   on every render tick; doing redundant work is acceptable, but
//!   side effects (focus changes, activations, animations) are not.

/// Keeps the overlay window at z+1 of `target`.
///
/// See the [module-level docs](self) for the full behavioural contract.
///
/// The trait is deliberately not `Send + 'static` — each platform's
/// enforcer is owned by exactly one thread (the overlay render thread)
/// and can borrow platform handles (e.g. the X11 `Connection`) that
/// aren't easily made `'static`. Cross-thread setup is handled by each
/// platform's own static / `OnceLock`.
pub trait ZOrderEnforcer {
    /// Re-pin the overlay so it sits just above `target` (or at the top
    /// of the non-topmost band when `target` is `None`).
    ///
    /// Called from the platform render thread on every tick.
    fn reassert(&self, target: Option<u64>);
}
