//! FocusGuard — Rust port of Swift's `FocusGuard.withFocusSuppressed`
//! (`libs/cua-driver/Sources/CuaDriverCore/Focus/FocusGuard.swift`).
//!
//! ## What this does
//!
//! Wraps an individual AX-action call (the inside of a click / type /
//! set_value / drag dispatch) with a **targeted** focus-steal suppression
//! lease. Catches the case where dispatching an AX attribute write
//! triggers a reflexive self-activation in the target app — Safari /
//! WebKit will sometimes pull itself to the front when an
//! `AXSelectedText` write hits a focused input, even when the AX call
//! itself doesn't go through `NSApp.activate(…)`.
//!
//! ## Layering relative to Swift
//!
//! Swift's `FocusGuard.withFocusSuppressed` does three things:
//!
//! 1. **Enablement** — set `AXManualAccessibility` /
//!    `AXEnhancedUserInterface` on the app root so Chromium/Electron
//!    targets respond to attribute-level dispatch.
//! 2. **Synthetic focus** — write `AXFocused` / `AXMain` on the
//!    enclosing window + element before the action, restore after.
//!    Makes AppKit behave as if the action came from a focused
//!    process, preventing reflex activations.
//! 3. **Reactive** — arm `SystemFocusStealPreventer` with a targeted
//!    entry. If the target self-activates anyway, re-activates the
//!    prior frontmost.
//!
//! This Rust port only ships **layer 3** (the reactive suppressor).
//! Layers 1+2 require AX assertion + AX attribute write/restore machinery
//! that isn't yet ported — and empirically the layer-3 reactive guard
//! catches the majority of side-effects when combined with
//! `WindowChangeDetector`'s wildcard lease at the snapshot→detect
//! boundary. We document this gap in PARITY.md so the port is auditable.
//!
//! ## Why this is a separate module
//!
//! `focus_steal::with_suppression` already exists as a thin closure
//! API around the dispatcher. `focus_guard::with_focus_suppressed`
//! wraps it with:
//!
//! 1. A "is target already frontmost?" check so we don't arm a useless
//!    self→self suppressor (matches Swift's `isTargetFrontmost` guard).
//! 2. A 50ms post-action sleep that gives any in-flight focus-grab
//!    reflex time to fire and be observed by the suppressor before the
//!    lease is dropped. Matches Swift's `Task.sleep(nanoseconds: 50ms)`.
//! 3. A static `origin` label for tracing — call sites pass a short
//!    string like `"click.AXPress"` so leaked leases / late-firing
//!    observers can be traced back to the caller.

use std::time::Duration;

use crate::apps;
use crate::focus_steal;

/// Wrap an async closure `f` with a targeted focus-steal suppressor.
///
/// - `target_pid` — the pid the action is dispatched to. `Some(pid)` is
///   the standard case; `None` skips the targeted entry entirely
///   (caller relies on the surrounding `WindowChangeDetector` wildcard
///   lease).
/// - `prior_frontmost` — the pid to restore focus to if the target
///   activates. Typically captured from `apps::frontmost_pid()` before
///   the snapshot.
/// - `origin` — short static label for tracing, e.g. `"click.AXPress"`.
/// - `f` — the action to run with suppression armed.
///
/// Returns whatever `f` returns. Drops the lease ~50ms after `f`
/// resolves so any in-flight reflex activation is observed before
/// suppression ends.
///
/// If `target_pid` is already the frontmost app (no point fighting
/// ourselves) or `prior_frontmost` is `None` (no app to restore to),
/// the function still runs `f` — just without the lease. This keeps
/// the call sites simple (no per-tool `if frontmost == target` ladders).
pub async fn with_focus_suppressed<F, Fut, R>(
    target_pid: Option<i32>,
    prior_frontmost: Option<i32>,
    origin: &'static str,
    f: F,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    // Decide whether to arm a targeted lease. Three conditions must hold:
    //   1. We have a known target pid (Some).
    //   2. We have a prior frontmost to restore to (Some).
    //   3. The target isn't already frontmost — Swift's
    //      `isTargetFrontmost` short-circuit, mirrored here. Without
    //      it we'd arm an entry that re-activates the prior frontmost
    //      against a no-op activation, fighting our own previous
    //      restore.
    let should_arm = match (target_pid, prior_frontmost) {
        (Some(tp), Some(pf)) => tp != pf,
        _ => false,
    };

    let _lease = if should_arm {
        // unwrap()s are safe — `should_arm` is true only when both are Some.
        Some(focus_steal::begin_suppression(
            target_pid,
            prior_frontmost.unwrap(),
            origin,
        ))
    } else {
        None
    };

    let result = f().await;

    // Post-action settle — give the reactive observer time to fire on
    // any side-effect activation before we drop the lease. Matches
    // Swift's 50ms sleep in `FocusGuard.withFocusSuppressed`.
    if _lease.is_some() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // _lease drops here (RAII end_suppression).
    result
}

/// Convenience wrapper that captures the current frontmost via
/// `apps::frontmost_pid()` at call time. Use when the caller doesn't
/// already have a `prior_frontmost` from a surrounding snapshot —
/// e.g. tools that only opt into the layer-3 guard without the
/// snapshot/detect cycle.
///
/// Most action tools should prefer `with_focus_suppressed` with an
/// explicit `prior_frontmost` captured *before* the snapshot, so the
/// wildcard lease + targeted lease both restore to the same pid.
pub async fn with_focus_suppressed_now<F, Fut, R>(
    target_pid: Option<i32>,
    origin: &'static str,
    f: F,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    let prior = apps::frontmost_pid();
    with_focus_suppressed(target_pid, prior, origin, f).await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// `with_focus_suppressed` runs the closure and returns its value.
    /// Skip-arming path (no prior frontmost) still executes f.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runs_closure_without_prior_frontmost() {
        let n = with_focus_suppressed(
            Some(42),
            None, // no prior → no lease armed
            "test.no_prior",
            || async { 7 },
        )
        .await;
        assert_eq!(n, 7);
    }

    /// Skip-arming path: target == prior_frontmost (self → self).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn runs_closure_when_target_is_prior_frontmost() {
        let n = with_focus_suppressed(
            Some(42),
            Some(42), // self → self → skip
            "test.self_to_self",
            || async { 11 },
        )
        .await;
        assert_eq!(n, 11);
    }

    /// Arming path: target != prior_frontmost. The lease is armed,
    /// closure runs, lease drops on the way out (we can't directly
    /// observe the dispatcher state through the public API here —
    /// the focus_steal module's own tests cover the lease lifecycle —
    /// but we exercise the codepath to make sure it compiles and the
    /// 50ms sleep doesn't deadlock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn arms_lease_when_target_differs_from_prior() {
        let n = with_focus_suppressed(
            Some(123),
            Some(456),
            "test.armed",
            || async { 99 },
        )
        .await;
        assert_eq!(n, 99);
    }

    /// `with_focus_suppressed_now` resolves prior_frontmost via the
    /// live NSWorkspace query — exercise the codepath end-to-end.
    /// In a unit-test context (no NSApp main thread) the query may
    /// return None; the closure must still run.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn now_variant_runs_even_when_frontmost_is_none() {
        let n = with_focus_suppressed_now(Some(7), "test.now", || async { 5 }).await;
        assert_eq!(n, 5);
    }
}
