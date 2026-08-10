//! Per-session desktop visibility scope.
//!
//! A session may be confined to a set of processes: an eval run that launched
//! Notepad and a calculator has no business seeing — or acting on — the
//! operator's password manager, chat client, or Settings window. When a scope
//! is bound, every enumeration the driver performs for that session is
//! filtered to the scope's pids *at the source*, before any summary line,
//! count, or structured record is built.
//!
//! That "at the source" property is the whole point. Filtering enumeration
//! text after the fact leaves stale totals ("Found 141 app(s)" above three
//! lines) which read to a model as a broken tool rather than as a smaller
//! world. A scoped session must see a desktop that is internally consistent:
//! the counts, the arrays, and the lines all describe the same processes.
//!
//! Like [`crate::capture_scope`], the scope is keyed exclusively by the
//! public, caller-declared `session` argument. Reserved transport mirrors such
//! as `_session_id` never mint or resolve scope state — otherwise an anonymous
//! proxy connection could inherit (or escape) another caller's confinement.
//!
//! No bound scope means no confinement: the driver enumerates the whole
//! desktop exactly as it always has. This module only ever removes things.

use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

/// The processes a confined session may see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VisibilityScope {
    pids: BTreeSet<u32>,
}

impl VisibilityScope {
    pub fn new(pids: impl IntoIterator<Item = u32>) -> Self {
        Self {
            pids: pids.into_iter().collect(),
        }
    }

    pub fn allows(&self, pid: u32) -> bool {
        self.pids.contains(&pid)
    }

    pub fn pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.pids.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.pids.is_empty()
    }
}

/// Trusted daemon-startup gate for per-session visibility confinement.
///
/// Mirrors the [`crate::authorization`] convention of resolving trusted
/// process configuration from an environment variable read once at startup,
/// never from a tool argument or transport field. Confinement is **off by
/// default**: existing production callers see byte-for-byte the same
/// enumeration behavior unless an operator explicitly opts a daemon in.
pub const VISIBILITY_SCOPE_ENV: &str = "CUA_DRIVER_VISIBILITY_SCOPE_ENABLED";

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

static FEATURE_ENABLED: OnceLock<bool> = OnceLock::new();

/// Whether this daemon process has opted into visibility-scope confinement.
///
/// Resolved once and cached, matching [`crate::authorization::configured_permission_mode`].
/// Tests may exercise `bind_session`/`scope_for` directly without setting the
/// env var — this gate only governs the automatic session-start/launch_app
/// wiring, not the underlying scope primitives themselves.
pub fn enabled() -> bool {
    *FEATURE_ENABLED.get_or_init(|| env_flag(VISIBILITY_SCOPE_ENV))
}

static SCOPES: OnceLock<Mutex<HashMap<String, VisibilityScope>>> = OnceLock::new();

fn scopes() -> &'static Mutex<HashMap<String, VisibilityScope>> {
    SCOPES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Confine a session to `pids`, replacing any previous scope.
pub fn bind_session(session: &str, pids: impl IntoIterator<Item = u32>) {
    if session.is_empty() {
        return;
    }
    scopes()
        .lock()
        .expect("visibility scopes")
        .insert(session.to_owned(), VisibilityScope::new(pids));
}

/// Grow a confined session's scope — what it launches, it may see.
///
/// A no-op for an unconfined session: growing an absent scope would confine a
/// session that was never meant to be, which fails closed in the wrong
/// direction (an operator's whole desktop suddenly reduced to one pid).
pub fn extend_session(session: &str, pid: u32) {
    if session.is_empty() {
        return;
    }
    if let Some(scope) = scopes()
        .lock()
        .expect("visibility scopes")
        .get_mut(session)
    {
        scope.pids.insert(pid);
    }
}

/// Drop a session's confinement. Called when the session ends.
pub fn clear_session(session: &str) {
    scopes().lock().expect("visibility scopes").remove(session);
}

/// The scope bound to `session`, if it is confined.
pub fn scope_for(session: &str) -> Option<VisibilityScope> {
    if session.is_empty() {
        return None;
    }
    scopes().lock().expect("visibility scopes").get(session).cloned()
}

/// The public session label of a tool invocation, as the registry leaves it.
///
/// By the time a tool sees its arguments the registry has rewritten `session`
/// into the runtime-private form `__cua_runtime_<scope>:<public>` and recorded
/// the caller's own label under the trusted `_public_session_label` key (any
/// caller-supplied `_`-prefixed argument having been stripped first). Scopes
/// are bound under the public label, so both forms have to resolve back to it
/// — otherwise confinement silently does nothing, which is the one failure
/// mode a security filter must not have.
pub(crate) fn public_session(args: &Value) -> Option<&str> {
    if let Some(label) = args.get("_public_session_label").and_then(Value::as_str) {
        return Some(label);
    }
    let session = args.get("session").and_then(Value::as_str)?;
    match session.strip_prefix("__cua_runtime_") {
        // `<scope>:<public>` — the public label is the tail.
        Some(rest) => rest.split_once(':').map(|(_, public)| public),
        None => Some(session),
    }
}

/// The scope governing a tool invocation. `None` means unconfined — enumerate
/// the whole desktop, exactly as the driver always has.
pub fn scope_for_args(args: &Value) -> Option<VisibilityScope> {
    public_session(args).and_then(scope_for)
}

/// Why a confined session cannot take a full-display capture.
///
/// Shared across the platform backends so all three refuse identically: a
/// desktop screenshot cannot be scoped — the pixels of every other app on the
/// machine come with it — and a cropped approximation would still be a guess.
pub const DESKTOP_CAPTURE_REFUSAL: &str =
    "get_desktop_state is unavailable to this session: it is scoped to its own windows, and a \
     full-display capture would include every other app on the machine. Use \
     get_window_state(pid, window_id) instead.";

/// Whether a pid is visible under an optional scope. Unconfined sees all.
pub fn allows(scope: Option<&VisibilityScope>, pid: u32) -> bool {
    scope.is_none_or(|s| s.allows(pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_unconfined_session_sees_the_whole_desktop() {
        assert!(scope_for("never-bound").is_none());
        assert!(allows(None, 4242));
        assert!(scope_for_args(&json!({})).is_none());
    }

    #[test]
    fn the_feature_is_off_unless_a_trusted_operator_opts_the_daemon_in() {
        // No env var set in the default test process environment: the gate
        // must read disabled. This is the load-bearing assertion for "today's
        // callers are completely unaffected unless they explicitly opt in" —
        // if this ever reads `true` without an operator having set
        // VISIBILITY_SCOPE_ENV, confinement would start silently.
        if std::env::var_os(VISIBILITY_SCOPE_ENV).is_none() {
            assert!(!env_flag(VISIBILITY_SCOPE_ENV));
        }
    }

    #[test]
    fn a_confined_session_sees_only_its_own_processes() {
        bind_session("scope-test-a", [10, 11]);
        let scope = scope_for_args(&json!({ "session": "scope-test-a" })).expect("bound");
        assert!(scope.allows(10));
        assert!(!scope.allows(12));
        clear_session("scope-test-a");
        assert!(scope_for("scope-test-a").is_none());
    }

    #[test]
    fn what_a_session_launches_it_may_see() {
        bind_session("scope-test-b", [10]);
        extend_session("scope-test-b", 99);
        assert!(scope_for("scope-test-b").expect("bound").allows(99));
        clear_session("scope-test-b");
    }

    #[test]
    fn extending_an_unconfined_session_does_not_confine_it() {
        // The failure this guards: a stray extend would collapse an operator's
        // whole desktop down to the single pid just handed in.
        extend_session("scope-test-c", 7);
        assert!(scope_for("scope-test-c").is_none());
    }

    #[test]
    fn transport_mirrors_never_resolve_a_scope() {
        bind_session("scope-test-d", [10]);
        // `_session_id` is a transport mirror; only the caller-declared
        // `session` may select a confinement.
        assert!(scope_for_args(&json!({ "_session_id": "scope-test-d" })).is_none());
        clear_session("scope-test-d");
    }

    #[test]
    fn the_runtime_namespaced_session_still_resolves() {
        // The registry rewrites `session` before a tool ever sees it. Missing
        // this is silent: confinement resolves to None and every scoped call
        // quietly enumerates the whole machine.
        bind_session("scope-test-e", [10]);
        let namespaced = json!({ "session": "__cua_runtime_deadbeef:scope-test-e" });
        assert!(scope_for_args(&namespaced).expect("resolves").allows(10));
        let labelled = json!({
            "session": "__cua_runtime_deadbeef:scope-test-e",
            "_public_session_label": "scope-test-e",
        });
        assert!(scope_for_args(&labelled).expect("resolves").allows(10));
        clear_session("scope-test-e");
    }
}
