//! Opaque per-snapshot element tokens (Surface 6).
//!
//! ## Why this exists
//!
//! Today consumers (Hermes wrapper, Codex, Claude Code) treat the bare
//! 1-based `element_index` returned by `get_window_state` as valid until
//! the next snapshot — but there's no formal validity contract. If
//! cua-driver ever changes its internal indexing the silent failure mode
//! is a misclick: the integer still parses, the AX path still resolves
//! *something*, and the user lands on the wrong button.
//!
//! Surface 6 adds an opaque token alongside the integer index whose
//! validity is **explicit** and **invalidated cheaply** when the next
//! snapshot supersedes the previous one for the same (pid, window_id).
//!
//! ## Token format
//!
//! Chosen for "smallest to implement", per the Surface 6 plan:
//!
//! ```text
//!   s{snapshot_id_hex}:{element_index}
//! ```
//!
//! - `snapshot_id_hex` is the complete lowercase 8-hex-char value of a
//!   process-global u32 snapshot counter (`AtomicU32`). Keeping all 32 bits
//!   prevents a live token from aliasing a newer snapshot after only 65,536
//!   calls.
//! - `element_index` is the same `usize` already returned in
//!   `structuredContent.elements[].element_index`. Keeping it in plain
//!   sight in the token means a server-side log line like
//!   `element_token=s00007a3f:42` is debug-grep-able without a side-table.
//!
//! Tokens are at least 11 chars (`"s00000001:0"`); the decimal element index
//! determines the remaining length.
//!
//! ## Validity contract
//!
//! - Snapshot IDs are minted in `register_snapshot` (called by every
//!   platform's `get_window_state` implementation immediately after the
//!   AX/UIA/AT-SPI walk lands in the per-platform element cache).
//! - A snapshot is valid until either (a) the LRU evicts it, or (b) a
//!   newer snapshot for the same `pid` pushes it past the LRU cap of
//!   [`LRU_CAP_PER_PID`].
//! - Resolving a stale token returns the explicit error string
//!   [`STALE_TOKEN_ERROR`] — consumers MUST treat that as "re-snapshot
//!   and retry", never as "click failed".
//!
//! The LRU is **per runtime and pid**, not global. Two snapshots in different
//! lanes never collide even when their numeric counters match.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

/// LRU cap of valid snapshots retained per pid. Only the newest snapshot for
/// each `(pid, window_id)` remains valid; the cap bounds snapshots retained for
/// other windows owned by the same process.
///
/// Chosen at 8: enough for an agent that re-snapshots once per turn over
/// a multi-window session (open Slack, open Safari, swap to Cursor, …)
/// before recycling; small enough that memory pressure is irrelevant.
/// Matches the "e.g. 8 most recent" suggestion in the Surface 6 plan.
pub const LRU_CAP_PER_PID: usize = 8;

/// Sentinel string returned by [`TokenRegistry::resolve`] when the token
/// parses but the snapshot it references has been invalidated. Consumers
/// (Hermes/Codex/Claude Code) MUST surface this as a re-snapshot-and-retry
/// signal, not a silent misclick.
pub const STALE_TOKEN_ERROR: &str =
    "element_token is stale; call get_window_state again to refresh";

/// One valid snapshot retained in the per-pid LRU.
#[derive(Debug, Clone, Copy)]
struct SnapshotEntry {
    /// Monotonic, process-global id assigned by [`mint_snapshot_id`].
    snapshot_id: u32,
    /// The window the snapshot was taken against. Resolution returns
    /// this so tools can verify the caller's `window_id` arg matches —
    /// a token-only call doesn't have to pass window_id at all.
    window_id: u32,
    /// Maximum element_index that was assigned in this snapshot. The
    /// resolver rejects out-of-range tokens up-front instead of waiting
    /// for the per-platform cache to NPE.
    max_element_index: usize,
}

/// Process-global token registry. Thread-safe; tools resolve from any
/// task via the shared [`global`] accessor.
///
/// The data model is a `HashMap<pid, Vec<SnapshotEntry>>` where each
/// pid's vec is the LRU (newest at the back). Vec instead of VecDeque
/// because the cap is tiny (8) and walks are linear either way.
pub struct TokenRegistry {
    by_runtime_and_pid: Mutex<HashMap<(String, i32), Vec<SnapshotEntry>>>,
}

impl TokenRegistry {
    fn new() -> Self {
        Self {
            by_runtime_and_pid: Mutex::new(HashMap::new()),
        }
    }

    /// Record a fresh snapshot for `pid` / `window_id`. Returns the
    /// minted snapshot id so the caller can embed it in the per-element
    /// token strings emitted alongside `element_index` in the structured
    /// `elements` array.
    ///
    /// `element_count` is the number of actionable elements in the
    /// snapshot (the count of nodes that received an `element_index`).
    /// Used for up-front range checks on `resolve`.
    ///
    /// Side effect: if this pid already has [`LRU_CAP_PER_PID`] snapshots
    /// in its lane, the oldest is evicted and any token that referenced
    /// it becomes stale — that's the contract.
    pub fn register_snapshot(&self, pid: i32, window_id: u32, element_count: usize) -> u32 {
        // Keep the full 32-bit counter. Truncating to 16 bits repeats an id
        // every 65,536 process-global snapshots. A long-lived daemon can then
        // mint an id that still exists in another runtime/pid lane, allowing a
        // stale token to resolve to the wrong snapshot instead of failing.
        let id = mint_snapshot_id();
        let runtime_scope = current_runtime_scope();
        let mut entries = self.by_runtime_and_pid.lock().unwrap();
        let lane = entries.entry((runtime_scope, pid)).or_default();
        // Every platform replaces its per-window element cache on the next
        // snapshot. Retaining an older token for the same window would resolve
        // that token against the new cache and could target a different row.
        // Invalidate it at the same boundary as the cache replacement.
        lane.retain(|entry| entry.window_id != window_id);
        lane.push(SnapshotEntry {
            snapshot_id: id,
            window_id,
            max_element_index: element_count.saturating_sub(1),
        });
        // Evict oldest. The loop guards against pre-existing over-cap
        // state from a previous version of the binary; in steady state
        // this fires exactly once per call.
        while lane.len() > LRU_CAP_PER_PID {
            lane.remove(0);
        }
        id
    }

    /// Resolve `token` against the LRU for `pid`. On success returns
    /// `(window_id, element_index)` — the same pair the caller would
    /// have passed as `(window_id, element_index)` integers. On failure
    /// returns one of:
    ///
    /// - `"element_token has invalid format"` — couldn't parse the
    ///   `s{hex}:{idx}` shape.
    /// - [`STALE_TOKEN_ERROR`] — parsed, but the snapshot id is no
    ///   longer in the pid's LRU (either evicted or never registered).
    /// - `"element_token element_index out of range"` — the index in
    ///   the token is past the max recorded for the snapshot.
    pub fn resolve(&self, pid: i32, token: &str) -> Result<(u32, usize), String> {
        let (sid, idx) =
            parse_token(token).ok_or_else(|| "element_token has invalid format".to_string())?;
        let runtime_scope = current_runtime_scope();
        let entries = self.by_runtime_and_pid.lock().unwrap();
        let belongs_to_another_runtime = || {
            entries.iter().any(|((scope, entry_pid), snapshots)| {
                scope != &runtime_scope
                    && *entry_pid == pid
                    && snapshots.iter().any(|entry| entry.snapshot_id == sid)
            })
        };
        let lane = entries.get(&(runtime_scope.clone(), pid)).ok_or_else(|| {
            if belongs_to_another_runtime() {
                "element_token belongs to another runtime generation".to_owned()
            } else {
                STALE_TOKEN_ERROR.to_owned()
            }
        })?;
        let entry = lane.iter().find(|e| e.snapshot_id == sid).ok_or_else(|| {
            if belongs_to_another_runtime() {
                "element_token belongs to another runtime generation".to_owned()
            } else {
                STALE_TOKEN_ERROR.to_owned()
            }
        })?;
        if idx > entry.max_element_index {
            return Err(format!(
                "element_token element_index {idx} out of range (snapshot had {} elements)",
                entry.max_element_index + 1
            ));
        }
        Ok((entry.window_id, idx))
    }

    pub fn clear_runtime_scope(&self, runtime_scope: &str) -> usize {
        let mut entries = self.by_runtime_and_pid.lock().unwrap();
        let before = entries.len();
        entries.retain(|(scope, _), _| scope != runtime_scope);
        before - entries.len()
    }

    /// Build the canonical token string for `snapshot_id` / `element_index`.
    /// Pure helper, mirrors [`format_token`] but lives on the registry so
    /// callers don't have to import the free function.
    #[allow(dead_code)]
    pub fn format(snapshot_id: u32, element_index: usize) -> String {
        format_token(snapshot_id, element_index)
    }

    /// Test-only: snapshot count for a pid. Used by the LRU-eviction
    /// unit test to assert the cap was honoured.
    #[cfg(test)]
    fn snapshot_count(&self, pid: i32) -> usize {
        self.by_runtime_and_pid
            .lock()
            .unwrap()
            .iter()
            .filter(|((_, entry_pid), _)| *entry_pid == pid)
            .map(|(_, entries)| entries.len())
            .sum()
    }

    /// Test-only: clear all state. Lets parallel unit tests start clean
    /// without relying on the global counter being at a specific value.
    #[cfg(test)]
    fn clear(&self) {
        self.by_runtime_and_pid.lock().unwrap().clear();
    }
}

fn current_runtime_scope() -> String {
    crate::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".to_owned())
}

impl Default for TokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-global counter for snapshot ids. Monotonically increasing —
/// even after eviction we never reuse an id during the process lifetime
/// (u32 wraps after 4 billion calls, well past any realistic agent run).
static SNAPSHOT_COUNTER: AtomicU32 = AtomicU32::new(1);

/// Mint a fresh snapshot id. `1`-based so `"s00000000:..."` is never a
/// legitimate token — makes "uninitialised default" bugs in client code
/// pop on the first call instead of accidentally aliasing a real
/// snapshot.
fn mint_snapshot_id() -> u32 {
    // `Relaxed` is fine: the only invariant we need is uniqueness of the
    // returned value, which `fetch_add` provides on its own. No happens-
    // before edge with the Mutex below — the lock provides that.
    SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Format `(snapshot_id, element_index)` as the canonical token string.
/// 4-hex-char snapshot prefix means tokens stay under 12 chars even
/// with 4-digit indices.
///
/// Snapshot ids are carried at full 32-bit width. The prefix is 8 hex
/// chars, so a token reads `s0a1b2c3d:12` — still inside Surface 6's
/// stated 8-16 character budget and still greppable in logs without a
/// side table. The registry treats the `(pid, snapshot_id)` pair as the
/// lookup key, so a same-bits collision across pids never aliases.
pub fn format_token(snapshot_id: u32, element_index: usize) -> String {
    format!("s{snapshot_id:08x}:{element_index}")
}

/// Parse a canonical token string into `(snapshot_id, element_index)`.
/// Returns `None` on any shape error (unknown prefix, missing colon,
/// non-hex, non-decimal). The token strings are produced by
/// [`format_token`] only — consumers MUST treat the format as opaque
/// and never construct one by hand.
fn parse_token(token: &str) -> Option<(u32, usize)> {
    let body = token.strip_prefix('s')?;
    let (hex, idx) = body.split_once(':')?;
    if hex.len() != 8 {
        return None;
    }
    let sid = u32::from_str_radix(hex, 16).ok()?;
    let idx = idx.parse::<usize>().ok()?;
    Some((sid, idx))
}

fn parse_snapshot_handle(snapshot_id: &str) -> Option<u32> {
    let hex = snapshot_id.strip_prefix('s')?;
    if hex.len() != 8 || hex.contains(':') {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// Process-global handle to the token registry. Used by every platform's
/// `get_window_state` (to register a fresh snapshot) and every element-
/// targeting tool (to resolve a passed-in token).
pub fn global() -> &'static TokenRegistry {
    static REG: OnceLock<TokenRegistry> = OnceLock::new();
    REG.get_or_init(TokenRegistry::new)
}

/// Build an `(snapshot_id, element_index)` token by minting both halves
/// from the current globals. Convenience for the per-platform
/// `build_elements_array` paths that already iterate over actionable
/// nodes and want a token per row.
///
/// `snapshot_id` is the value returned by [`TokenRegistry::register_snapshot`]
/// for the current `get_window_state` call. Pass the same id for every
/// element in one snapshot — the token registry already tracks them as
/// a group keyed by that id.
pub fn token_for(snapshot_id: u32, element_index: usize) -> String {
    format_token(snapshot_id, element_index)
}

/// Result of validating an `element_token` or snapshot-bound `element_index`
/// on a tool call's args. Returned by [`resolve_element_args`].
#[derive(Debug, Clone)]
pub enum ResolvedElement {
    /// Neither `element_token` nor `element_index` was supplied — the
    /// tool should fall through to its non-element addressing mode
    /// (typically pixel `x, y`) or error.
    None,
    /// Resolved to `(window_id, element_index)`. Element targets always resolve
    /// through the snapshot registry, so `window_id` is always present. The
    /// optional shape remains temporarily to avoid duplicating tool plumbing.
    Element {
        window_id: Option<u32>,
        element_index: usize,
        /// True when the caller supplied a token and we resolved
        /// through the registry — informational, used by tools that
        /// want to log "via token" in the success summary.
        via_token: bool,
    },
}

/// Validate tool args that accept `element_index`, `snapshot_id`, and
/// `element_token`. Returns either a structured refusal or the resolved
/// `(window_id, element_index)` pair.
///
/// Rule:
/// - **Neither**: returns [`ResolvedElement::None`]. The tool decides
///   whether to error or fall through to a pixel path.
/// - **Only `element_index`**: refuses with `snapshot_id_required`.
/// - **`element_index` + `snapshot_id`**: resolves that exact registered
///   snapshot and fails closed if it is stale.
/// - **Only `element_token`**: resolves through the registry. On stale
///   or malformed token, returns an error. On success returns
///   `Element { window_id: Some(<from snapshot>), element_index, via_token: true }`.
/// - **Token plus other target fields**: every supplied field must identify the
///   same snapshot, window, and element. A conflict fails closed.
///
/// `args_window_id` is the `window_id` arg the caller already pulled
/// off the JSON via the existing `args.opt_u64("window_id")`. Passing
/// it in here lets the helper keep that lookup in one place per tool
/// rather than duplicating it.
pub fn resolve_element_args(
    pid: i32,
    args_element_index: Option<usize>,
    args_element_token: Option<&str>,
    args_snapshot_id: Option<&str>,
    args_window_id: Option<u32>,
    tool_name: &str,
) -> Result<ResolvedElement, crate::protocol::ToolResult> {
    let refusal = |code: &str, message: String| {
        crate::protocol::ToolResult::error(message.clone()).with_structured(serde_json::json!({
            "status": "refused",
            "refusal": { "code": code, "message": message }
        }))
    };
    let resolve_token = |tok: &str| {
        let (wid, idx) = global().resolve(pid, tok).map_err(|message| {
            let code = if message.contains("another runtime generation") {
                "generation_mismatch"
            } else if message == STALE_TOKEN_ERROR {
                "stale_element_token"
            } else {
                "invalid_element_token"
            };
            refusal(code, message)
        })?;
        Ok::<_, crate::protocol::ToolResult>((wid, idx))
    };

    match (args_element_index, args_element_token, args_snapshot_id) {
        (None, None, None) => Ok(ResolvedElement::None),
        (None, None, Some(_)) => Err(refusal(
            "element_index_required",
            format!("{tool_name}: snapshot_id requires element_index"),
        )),
        (Some(_), None, None) => Err(refusal(
            "snapshot_id_required",
            format!(
                "{tool_name}: bare element_index is not accepted; pass element_token, or snapshot_id together with element_index"
            ),
        )),
        (Some(idx), None, Some(snapshot_handle)) => {
            let snapshot_id = parse_snapshot_handle(snapshot_handle).ok_or_else(|| {
                refusal(
                    "invalid_snapshot_id",
                    format!("{tool_name}: snapshot_id has invalid format"),
                )
            })?;
            let token = format_token(snapshot_id, idx);
            let (wid, resolved_idx) = resolve_token(&token)?;
            if args_window_id.is_some_and(|arg_wid| arg_wid != wid) {
                return Err(refusal(
                    "conflicting_element_target",
                    format!(
                        "{tool_name}: snapshot belongs to window_id {wid}, not {}",
                        args_window_id.unwrap()
                    ),
                ));
            }
            Ok(ResolvedElement::Element {
                window_id: Some(wid),
                element_index: resolved_idx,
                via_token: false,
            })
        }
        (idx_opt, Some(tok), snapshot_opt) => {
            let (wid, idx) = resolve_token(tok)?;
            if idx_opt.is_some_and(|arg_idx| arg_idx != idx)
                || args_window_id.is_some_and(|arg_wid| arg_wid != wid)
                || snapshot_opt.is_some_and(|handle| {
                    parse_snapshot_handle(handle)
                        != parse_token(tok).map(|(snapshot_id, _)| snapshot_id)
                })
            {
                return Err(refusal(
                    "conflicting_element_target",
                    format!(
                        "{tool_name}: element_token conflicts with element_index, snapshot_id, or window_id"
                    ),
                ));
            }
            Ok(ResolvedElement::Element {
                window_id: Some(wid),
                element_index: idx,
                via_token: true,
            })
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_registry() -> TokenRegistry {
        TokenRegistry::new()
    }

    #[test]
    fn token_round_trips_through_format_then_parse() {
        // The full 32-bit id survives the round trip now that neither the
        // registry nor format_token truncates.
        let token = format_token(0x1234, 42);
        assert_eq!(token, "s00001234:42");
        let (sid, idx) = parse_token(&token).expect("parse_token should accept its own output");
        assert_eq!(sid, 0x1234);
        assert_eq!(idx, 42);
    }

    #[test]
    fn token_format_pads_to_eight_hex_chars() {
        // Small ids must still pad to the full 8-char prefix so the parser's
        // length check passes. Surface 6's stated format is "8-16 chars";
        // `s0a1b2c3d:12` is 12, comfortably inside that.
        let token = format_token(1, 0);
        assert_eq!(token, "s00000001:0");
        let token2 = format_token(0, 999);
        assert_eq!(token2, "s00000000:999");
        // Ids above the old 16-bit ceiling now survive the round trip
        // instead of aliasing onto a low id.
        let wide = format_token(0x0001_0001, 3);
        assert_eq!(wide, "s00010001:3");
        assert_eq!(parse_token(&wide), Some((0x0001_0001, 3)));
    }

    #[test]
    fn parse_rejects_unknown_prefix_or_shape() {
        assert!(parse_token("").is_none());
        assert!(parse_token("x00001234:42").is_none(), "wrong prefix");
        assert!(parse_token("s00001234").is_none(), "missing colon");
        assert!(parse_token("s000012345:42").is_none(), "hex too long");
        assert!(parse_token("s1234:42").is_none(), "hex too short");
        assert!(parse_token("szzzzzzzz:42").is_none(), "non-hex");
        assert!(parse_token("s00001234:abc").is_none(), "non-decimal index");
    }

    #[test]
    fn register_then_resolve_returns_window_and_index() {
        let reg = fresh_registry();
        let pid = 100;
        let snapshot_id = reg.register_snapshot(pid, 42, /* element_count */ 5);
        let token = format_token(snapshot_id, 3);
        let (wid, idx) = reg.resolve(pid, &token).expect("fresh token must resolve");
        assert_eq!(wid, 42);
        assert_eq!(idx, 3);
    }

    #[test]
    fn resolve_with_unknown_pid_returns_stale_error() {
        // `STALE_TOKEN_ERROR` is the contract string consumers grep for.
        let reg = fresh_registry();
        let token = format_token(0x1234, 0);
        let err = reg.resolve(/* pid = */ 999, &token).unwrap_err();
        assert_eq!(err, STALE_TOKEN_ERROR);
    }

    #[test]
    fn resolve_with_bad_format_returns_invalid_error() {
        let reg = fresh_registry();
        // Pre-register a snapshot so we know the failure isn't from an
        // empty registry — the format check must run before the lane
        // lookup so callers get the more useful error.
        reg.register_snapshot(10, 1, 1);
        let err = reg.resolve(10, "garbage").unwrap_err();
        assert!(err.contains("invalid format"), "got: {err}");
    }

    #[test]
    fn out_of_range_index_returns_actionable_error() {
        let reg = fresh_registry();
        let pid = 11;
        let snapshot_id = reg.register_snapshot(pid, 1, /* element_count */ 3);
        // Snapshot has indices 0..2 — 7 is past the end.
        let token = format_token(snapshot_id, 7);
        let err = reg.resolve(pid, &token).unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn next_snapshot_for_same_window_invalidates_old_immediately() {
        let reg = fresh_registry();
        let pid = 12;
        let s1 = reg.register_snapshot(pid, 1, 5);
        let s2 = reg.register_snapshot(pid, 1, 5);
        assert_eq!(
            reg.resolve(pid, &format_token(s1, 0)).unwrap_err(),
            STALE_TOKEN_ERROR
        );
        let _ = reg.resolve(pid, &format_token(s2, 0)).expect("s2 fresh");
    }

    #[test]
    fn snapshots_for_different_windows_share_the_bounded_lru() {
        let reg = fresh_registry();
        let pid = 12;
        let s1 = reg.register_snapshot(pid, 1, 5);
        let s2 = reg.register_snapshot(pid, 2, 5);
        assert_eq!(reg.resolve(pid, &format_token(s1, 0)).unwrap().0, 1);
        assert_eq!(reg.resolve(pid, &format_token(s2, 0)).unwrap().0, 2);
    }

    #[test]
    fn lru_eviction_invalidates_oldest_snapshot() {
        let reg = fresh_registry();
        let pid = 13;
        // Fill the LRU.
        let oldest = reg.register_snapshot(pid, 1, 5);
        for window_id in 2..=(LRU_CAP_PER_PID as u32 + 1) {
            // Push LRU_CAP_PER_PID more, which evicts `oldest`.
            let _ = reg.register_snapshot(pid, window_id, 5);
        }
        // Lane size must respect the cap.
        assert_eq!(reg.snapshot_count(pid), LRU_CAP_PER_PID);
        // Oldest must be stale now.
        let err = reg.resolve(pid, &format_token(oldest, 0)).unwrap_err();
        assert_eq!(err, STALE_TOKEN_ERROR);
    }

    #[test]
    fn tokens_in_different_pids_dont_collide() {
        // Same snapshot counter values across pids must resolve back to
        // each pid's own window_id, never the other's. This is the
        // per-pid lane property the registry promises.
        let reg = fresh_registry();
        let s_a = reg.register_snapshot(/* pid = */ 100, /* window_id = */ 11, 3);
        let s_b = reg.register_snapshot(/* pid = */ 200, /* window_id = */ 22, 3);
        let token_a = format_token(s_a, 0);
        let token_b = format_token(s_b, 0);
        // Cross-pid attempts must NOT resolve to the other pid's window.
        assert_eq!(reg.resolve(100, &token_a).unwrap().0, 11);
        assert_eq!(reg.resolve(200, &token_b).unwrap().0, 22);
        // Attempting to use pid A's token under pid B must fail stale.
        let err = reg.resolve(200, &token_a).unwrap_err();
        assert_eq!(err, STALE_TOKEN_ERROR);
    }

    #[test]
    fn global_registry_is_shared_across_calls() {
        // Smoke test that `global()` returns the same instance every
        // call. We don't depend on cross-test isolation here — the
        // assertion is structural, not value-based.
        let reg_a = global();
        let reg_b = global();
        assert!(std::ptr::eq(reg_a, reg_b));
    }

    #[test]
    fn stale_token_returns_explicit_error_not_silent_misclick() {
        // Surface 6 hard constraint: we must NEVER silently re-map a
        // stale token to "some index" — the consumer has to see the
        // error string and re-snapshot.
        let reg = fresh_registry();
        let pid = 14;
        let s1 = reg.register_snapshot(pid, 1, 5);
        // Evict by pushing LRU_CAP_PER_PID newer snapshots.
        for _ in 0..LRU_CAP_PER_PID {
            let _ = reg.register_snapshot(pid, 1, 5);
        }
        let err = reg.resolve(pid, &format_token(s1, 2)).unwrap_err();
        assert_eq!(err, STALE_TOKEN_ERROR);
    }

    #[test]
    fn clear_then_register_starts_clean() {
        let reg = fresh_registry();
        let _ = reg.register_snapshot(1, 1, 1);
        reg.clear();
        assert_eq!(reg.snapshot_count(1), 0);
    }

    // ── resolve_element_args precedence rule ─────────────────────────
    //
    // These cover the Surface 6 dispatch contract:
    //
    // - element_index_alone_still_works
    // - element_token_alone_resolves_to_same_action
    // - both_provided_token_wins_disagree_warns
    //
    // The "stale" and "different pids" surfaces are already covered by
    // the registry-level tests above; resolve_element_args is just the
    // thin precedence layer on top.

    #[test]
    fn bare_element_index_is_refused() {
        let result = resolve_element_args(
            /* pid = */ 1,
            /* element_index = */ Some(7),
            /* element_token = */ None,
            /* snapshot_id = */ None,
            /* window_id = */ Some(99),
            "click",
        );
        assert!(result.is_err());
    }

    #[test]
    fn element_token_alone_resolves_to_same_action() {
        // Register a snapshot in the GLOBAL registry (resolve_element_args
        // uses `global()`), then resolve the token through the same path
        // the tool would use.
        let reg = global();
        // Use a pid unlikely to collide with other tests.
        let pid = 0x7fff_0001_i32;
        let snapshot_id = reg.register_snapshot(pid, /* window_id = */ 555, 4);
        let token = format_token(snapshot_id, 2);
        let resolved = resolve_element_args(
            pid,
            None,
            Some(&token),
            None,
            // window_id arg intentionally omitted — the token carries it.
            None,
            "click",
        )
        .expect("token-only must succeed");
        match resolved {
            ResolvedElement::Element {
                window_id,
                element_index,
                via_token,
            } => {
                assert_eq!(window_id, Some(555), "window_id comes from the snapshot");
                assert_eq!(element_index, 2);
                assert!(via_token, "token path must report via_token=true");
            }
            _ => panic!("expected Element, got {resolved:?}"),
        }
    }

    #[test]
    fn conflicting_token_and_index_are_refused() {
        let reg = global();
        let pid = 0x7fff_0002_i32;
        let snapshot_id = reg.register_snapshot(pid, 777, 5);
        let token = format_token(snapshot_id, 3);
        let result = resolve_element_args(pid, Some(99), Some(&token), None, None, "click");
        assert!(result.is_err());
    }

    #[test]
    fn snapshot_id_and_index_resolve_safely() {
        let reg = global();
        let pid = 0x7fff_0004_i32;
        let snapshot_id = reg.register_snapshot(pid, 888, 5);
        let handle = format!("s{snapshot_id:08x}");
        let resolved = resolve_element_args(pid, Some(2), None, Some(&handle), Some(888), "click")
            .expect("snapshot-bound index must resolve");
        assert!(matches!(
            resolved,
            ResolvedElement::Element {
                window_id: Some(888),
                element_index: 2,
                via_token: false
            }
        ));
    }

    #[test]
    fn token_only_stale_returns_error_not_silent_fallback_to_integer() {
        // Surface 6 hard constraint: a stale token MUST NOT fall back
        // to the integer — that would silently misclick.
        let pid = 0x7fff_0003_i32;
        // Token references a snapshot that was never registered → stale.
        let token = format_token(0xdead, 0);
        let err =
            resolve_element_args(pid, Some(0), Some(&token), None, Some(1), "click").unwrap_err();
        // ToolResult::error wraps the message in a Content::Text — the
        // assertion uses the protocol-level error_text accessor.
        assert!(
            err.is_error.unwrap_or(false),
            "stale token must return an error ToolResult"
        );
    }

    #[test]
    fn neither_returns_none() {
        let resolved = resolve_element_args(1, None, None, None, None, "click")
            .expect("neither arg returns None, not error");
        assert!(matches!(resolved, ResolvedElement::None));
    }
}
