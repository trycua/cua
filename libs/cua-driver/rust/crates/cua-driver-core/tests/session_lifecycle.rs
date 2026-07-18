//! End-to-end session lifecycle / disposal test (cross-platform; runs on the Mac
//! via `cargo test -p cua-driver-core`).
//!
//! A session is a caller-declared identity that OWNS disposable per-run state:
//! the agent cursor, per-session config overrides, and any recording it started.
//! Those concrete registries live OUTSIDE this core crate:
//!   * the agent cursor → `platform-macos::CursorRegistry` + the overlay,
//!   * per-session config overrides → `platform-macos::SessionConfigRegistry`,
//!   * the owned recording → stopped by `serve.rs`'s recording hook.
//! None of them is reachable from a core-only test. They are ALL wired to
//! disposal through ONE mechanism: each registers a `register_session_end_hook`,
//! and BOTH teardown paths — `end_session` (explicit) and `evict_idle` (the
//! idle-TTL sweep) — dispose by fanning `fire_session_end` out to those hooks.
//!
//! This test therefore asserts disposal at the deepest deterministic layer
//! available in core: it registers its own stand-in hooks — one per real
//! production hook (cursor-remove, config-clear, recording-stop) — and proves
//! that EVERY one of them runs, exactly once, on BOTH `end_session` and
//! `evict_idle`. If the fan-out for either path regressed (e.g. the sweep stopped
//! disposing like `end_session`), these hooks would not fire and the test fails.
//!
//! What is only checkable at the daemon level (NOT asserted here): the actual
//! post-state of `platform-macos::CursorRegistry` / overlay / `SessionConfigRegistry`
//! and the daemon's `RecordingSession` — that requires a `serve`-layer/platform
//! integration test. Here we assert the hooks those registries hang off of fire on
//! both paths, which is the cross-platform half of the contract.

use cua_driver_core::session::{
    active_session_count, end_session, evict_idle, is_session_ended, register_session_end_hook,
    revive_session, touch_session,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// `evict_idle` operates on the PROCESS-GLOBAL activity map, and the test binary
/// runs tests in parallel. A concurrent `evict_idle(ZERO)` from another test
/// would otherwise steal (reap) this test's idle session before its own sweep
/// observes it — making "my sweep returned my id" flaky. Serialize every test
/// that runs an eviction so each sweep deterministically owns the sessions it
/// touched. (Poison-tolerant: a panicking test must not wedge the rest.)
fn eviction_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Process-global session state is shared across every test in this binary, so
/// each test uses its own unique session id and hooks that filter on it.
fn unique_id(tag: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("test-lifecycle-{tag}-{}-{n}", std::process::id())
}

/// The three disposal side-effects a session owns, each modelling one real
/// production `register_session_end_hook` registration:
///   * `cursor_removed`   → `platform-macos`: `CursorRegistry::remove` + overlay.
///   * `config_cleared`   → `platform-macos`: `SessionConfigRegistry::clear`.
///   * `recording_stopped`→ `serve.rs`: `RecordingSession::stop_owner(Some(sid))`.
/// Each counts fires for ONE specific session id (hooks are process-global, so we
/// filter). A correctly-disposed session shows all three == 1.
struct Disposal {
    cursor_removed: Arc<AtomicUsize>,
    config_cleared: Arc<AtomicUsize>,
    recording_stopped: Arc<AtomicUsize>,
}

impl Disposal {
    /// Register the three independent cleanup hooks for `sid`, mirroring how the
    /// macOS platform crate and the serve layer each register their own.
    fn wire(sid: &str) -> Self {
        let cursor_removed = Arc::new(AtomicUsize::new(0));
        let config_cleared = Arc::new(AtomicUsize::new(0));
        let recording_stopped = Arc::new(AtomicUsize::new(0));

        for counter in [&cursor_removed, &config_cleared, &recording_stopped] {
            let counter = counter.clone();
            let want = sid.to_owned();
            register_session_end_hook(move |got| {
                if got == want {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            });
        }

        Disposal {
            cursor_removed,
            config_cleared,
            recording_stopped,
        }
    }

    fn counts(&self) -> (usize, usize, usize) {
        (
            self.cursor_removed.load(Ordering::Relaxed),
            self.config_cleared.load(Ordering::Relaxed),
            self.recording_stopped.load(Ordering::Relaxed),
        )
    }

    /// Assert the session was fully disposed: every cleanup hook fired exactly
    /// once (cursor removed, config cleared, recording stopped).
    fn assert_fully_disposed_once(&self, ctx: &str) {
        assert_eq!(
            self.counts(),
            (1, 1, 1),
            "{ctx}: every disposal hook (cursor-remove, config-clear, recording-stop) \
             must fire exactly once",
        );
    }

    fn assert_not_disposed(&self, ctx: &str) {
        assert_eq!(self.counts(), (0, 0, 0), "{ctx}: nothing disposed yet");
    }
}

/// Full lifecycle: start → activity (alive under long TTL) → explicit end
/// disposes (all three hooks) → a recycled id starts fresh (no leaked/poisoned
/// remnant).
#[test]
fn end_session_disposes_then_id_reuse_starts_fresh() {
    let _guard = eviction_lock();
    let sid = unique_id("explicit");
    let disposal = Disposal::wire(&sid);

    // 1. start_session(id) registers the session (active, trackable). Mirrors
    //    StartSessionTool: revive a recycled id, then touch to begin the idle-TTL
    //    clock. The agent cursor is created on the session's first action; in core
    //    the trackable, not-ended, counted state is the proxy for "registered /
    //    cursor present" (the CursorRegistry is platform-macos, daemon-level only).
    revive_session(&sid);
    touch_session(&sid);
    assert!(
        !is_session_ended(&sid),
        "freshly started session must be live"
    );
    assert!(
        active_session_count() >= 1,
        "session must be counted as active"
    );
    disposal.assert_not_disposed("just started");

    // 2. After activity, the session is alive and NOT evicted under a long TTL.
    touch_session(&sid);
    let evicted = evict_idle(Duration::from_secs(3600));
    assert!(
        !evicted.contains(&sid),
        "long-TTL sweep must not evict a just-touched session"
    );
    assert!(!is_session_ended(&sid), "still live after a long-TTL sweep");
    disposal.assert_not_disposed("after long-TTL no-op sweep");

    // 3. end_session disposes: all three cleanup hooks fire (cursor removed,
    //    config cleared, recording stopped).
    end_session(&sid);
    assert!(is_session_ended(&sid), "ended session is tombstoned");
    disposal.assert_fully_disposed_once("end_session");
    // The idle-TTL entry is gone, so a later sweep won't re-fire for it
    // (fire_session_end is idempotent regardless).
    assert!(
        !evict_idle(Duration::ZERO).contains(&sid),
        "no leftover TTL entry"
    );
    disposal.assert_fully_disposed_once("end_session is idempotent (no double-dispose)");

    // 4. Reuse the same id: it must start FRESH, not a leaked/poisoned remnant.
    assert!(
        revive_session(&sid),
        "re-declaring a recycled id revives it"
    );
    touch_session(&sid);
    assert!(
        !is_session_ended(&sid),
        "revived id is live again, actions no longer rejected"
    );
    assert!(
        active_session_count() >= 1,
        "recycled id is counted active again"
    );

    // Clean up so we don't leak the TTL entry past the test.
    end_session(&sid);
}

/// Idle-TTL auto-eviction disposes a session the SAME way `end_session` does:
/// every cleanup hook fires — not merely that the id left the activity map.
#[test]
fn idle_ttl_eviction_disposes_like_end_session() {
    let _guard = eviction_lock();
    let sid = unique_id("idle");
    let disposal = Disposal::wire(&sid);

    // Start, then leave the session UNTOUCHED past a short TTL.
    touch_session(&sid);
    disposal.assert_not_disposed("before idle window");

    // Age the session well beyond the short TTL we'll sweep with.
    std::thread::sleep(Duration::from_millis(25));
    let evicted = evict_idle(Duration::from_millis(5));

    // (a) the id left the activity map (the shallow signal)...
    assert!(
        evicted.contains(&sid),
        "idle session past the short TTL is evicted"
    );
    // (b) ...AND it was disposed exactly like end_session: tombstoned + every
    //     cleanup hook fired once. This is the side-effect assertion, not just
    //     map membership.
    assert!(is_session_ended(&sid), "idle-evicted session is tombstoned");
    disposal.assert_fully_disposed_once("idle-TTL eviction");

    // A second sweep must not re-dispose (idempotent fan-out).
    let _ = evict_idle(Duration::ZERO);
    disposal.assert_fully_disposed_once("idle-TTL eviction is idempotent");
}

/// A just-touched session survives a short-TTL sweep that evicts an idle sibling:
/// guards against the sweep being over-eager (reaping live sessions).
#[test]
fn fresh_session_survives_short_ttl_that_evicts_idle_sibling() {
    let _guard = eviction_lock();
    let idle = unique_id("survive-idle");
    let fresh = unique_id("survive-fresh");
    let idle_disposal = Disposal::wire(&idle);
    let fresh_disposal = Disposal::wire(&fresh);

    touch_session(&idle);
    std::thread::sleep(Duration::from_millis(40));
    // Touch `fresh` immediately before the sweep so its age (~0) is far under the
    // 10ms TTL, while `idle` (~40ms) is far over it.
    touch_session(&fresh);
    let evicted = evict_idle(Duration::from_millis(10));

    assert!(evicted.contains(&idle), "idle sibling evicted");
    idle_disposal.assert_fully_disposed_once("idle sibling disposed");

    assert!(
        !evicted.contains(&fresh),
        "freshly-touched session must survive the sweep"
    );
    assert!(!is_session_ended(&fresh), "fresh session not tombstoned");
    fresh_disposal.assert_not_disposed("fresh session not disposed by the sweep");

    end_session(&fresh); // cleanup
}
