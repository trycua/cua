use crate::protocol::ToolResult;
use std::sync::atomic::{AtomicU32, Ordering};

pub const LRU_CAP_PER_PID: usize = 8;
pub const STALE_TOKEN_ERROR: &str =
    "element_token is stale; call get_window_state again to refresh";

static SNAPSHOT_COUNTER: AtomicU32 = AtomicU32::new(1);

pub(crate) fn mint_snapshot_id() -> u32 {
    SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub fn format_snapshot_id(snapshot_id: u32) -> String {
    format!("s{snapshot_id:08x}")
}

pub fn token_for(snapshot_id: u32, element_index: usize) -> String {
    format!("{}:{element_index}", format_snapshot_id(snapshot_id))
}

pub fn parse_token(token: &str) -> Option<(u32, usize)> {
    let (handle, index) = token.split_once(':')?;
    Some((parse_snapshot_handle(handle)?, index.parse().ok()?))
}

pub fn parse_snapshot_handle(handle: &str) -> Option<u32> {
    let hex = handle.strip_prefix('s')?;
    if hex.len() != 8 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

#[derive(Debug, Clone)]
pub enum ResolvedElement<T> {
    None,
    Element {
        window_id: u64,
        element_index: usize,
        element: T,
    },
}

impl<T> ResolvedElement<T> {
    pub fn into_parts(
        self,
        fallback_window: Option<u64>,
    ) -> (Option<usize>, Option<u64>, Option<T>) {
        match self {
            Self::None => (None, fallback_window, None),
            Self::Element {
                window_id,
                element_index,
                element,
            } => (Some(element_index), Some(window_id), Some(element)),
        }
    }
}

/// Message for a call that names its target only by an `element_token` the
/// runtime no longer knows (or never minted), so no pid can be derived.
pub const STALE_TOKEN_WITHOUT_PID: &str =
    "element_token is stale or unknown; call get_window_state again to refresh";

/// A snapshot a refusal names as current, so the caller can tell whether it
/// already holds a newer one instead of re-reading blind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentSnapshot {
    pub snapshot_id: String,
    pub window_id: u64,
}

/// `current snapshots for pid 7: s0000002a (window 100)` — the phrase every
/// stale refusal shares, so both refusal shapes name the same thing the same
/// way.
pub fn current_snapshots_phrase(pid: i32, current: &[CurrentSnapshot]) -> String {
    if current.is_empty() {
        return format!("current snapshots for pid {pid}: none");
    }
    format!(
        "current snapshots for pid {pid}: {}",
        current
            .iter()
            .map(|snapshot| format!("{} (window {})", snapshot.snapshot_id, snapshot.window_id))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `current_snapshots` as structured content: the same array both refusal
/// shapes carry, so a client reads one field whichever path refused.
pub fn current_snapshots_json(current: &[CurrentSnapshot]) -> serde_json::Value {
    serde_json::Value::Array(
        current
            .iter()
            .map(|snapshot| {
                serde_json::json!({
                    "snapshot_id": snapshot.snapshot_id,
                    "window_id": snapshot.window_id,
                })
            })
            .collect(),
    )
}

/// What a call's `element_token` could be attributed to.
#[derive(Clone, Debug, Default)]
pub struct TokenOwner {
    /// The process the token's snapshot was minted for, current or retired.
    pub pid: Option<i32>,
    /// The process's current snapshots, when `pid` is known.
    pub current: Vec<CurrentSnapshot>,
    /// Whether the token still names a snapshot the runtime holds. A token for
    /// a retired snapshot can be attributed to its process but must not be
    /// acted on, so the call is refused rather than handed a pid.
    pub live: bool,
}

/// The refusal for [`STALE_TOKEN_WITHOUT_PID`]: `stale_element_token`, naming
/// the current snapshots whenever the token can be attributed to a process.
///
/// A refusal is the caller's chance to recover without a blind re-read, so the
/// token-only path reports the same `current_snapshots` the pid-carrying
/// refusal does. When the token cannot be attributed to any process — it was
/// never minted here, or it is malformed — nothing is invented: the message
/// says so instead.
pub fn stale_token_without_pid(owner: TokenOwner) -> ToolResult {
    let message = match (owner.pid, owner.current.is_empty()) {
        (Some(pid), false) => format!(
            "{STALE_TOKEN_WITHOUT_PID} (a current token names its own pid); {}",
            current_snapshots_phrase(pid, &owner.current)
        ),
        (Some(pid), true) => format!(
            "{STALE_TOKEN_WITHOUT_PID}; pid {pid} has no current snapshot. Call get_window_state \
             for the window you mean, then use the element_token it prints."
        ),
        (None, _) => format!(
            "{STALE_TOKEN_WITHOUT_PID}. This token cannot be attributed to a pid: this runtime \
             never minted it, or the snapshot it names has been gone long enough that the \
             process is no longer known."
        ),
    };
    let mut structured = serde_json::json!({
        "status": "refused",
        "refusal": { "code": "stale_element_token", "message": message },
    });
    if let Some(pid) = owner.pid {
        structured["pid"] = pid.into();
        structured["current_snapshots"] = current_snapshots_json(&owner.current);
    }
    ToolResult::error(message).with_structured(structured)
}

/// [`stale_token_without_pid`] for a caller that cannot attribute the token at
/// all (no snapshot store in reach).
pub fn stale_token_unattributed() -> ToolResult {
    stale_token_without_pid(TokenOwner::default())
}

/// Fill a missing `pid` from the call's `element_token`. A token names its
/// snapshot, and so its window and process, so a token-only call needs no
/// pid. An explicit `pid` wins and a call without a token is left as is (the
/// tool reports its own missing-pid error). A token whose snapshot is gone
/// is refused as `stale_element_token` rather than as a missing pid, and that
/// refusal names the current snapshots of the process the token belonged to.
pub fn fill_pid_from_token(
    args: &mut serde_json::Value,
    owner_for_token: impl FnOnce(&serde_json::Value) -> TokenOwner,
) -> Result<(), ToolResult> {
    let present = |key: &str| args.get(key).is_some_and(|value| !value.is_null());
    if present("pid") || !present("element_token") {
        return Ok(());
    }
    let owner = owner_for_token(args);
    // Fail closed: only a token naming a snapshot the runtime still holds may
    // be turned into a pid, so a stale token never reaches a tool that would
    // act on it. The refusal still names what is current for that process.
    let Some(pid) = owner.pid.filter(|_| owner.live) else {
        return Err(stale_token_without_pid(owner));
    };
    if let Some(object) = args.as_object_mut() {
        object.insert("pid".to_owned(), pid.into());
    }
    Ok(())
}

/// [`fill_pid_from_token`] for a caller whose resolver reports only a pid.
pub fn fill_pid_from_token_with(
    args: &mut serde_json::Value,
    pid_for_token: impl FnOnce(&serde_json::Value) -> Option<i32>,
) -> Result<(), ToolResult> {
    fill_pid_from_token(args, |args| {
        let pid = pid_for_token(args);
        TokenOwner {
            pid,
            current: Vec::new(),
            live: pid.is_some(),
        }
    })
}

pub(crate) fn refusal(code: &str, message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused", "refusal": { "code": code, "message": message }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_pid_from_token_resolves_only_token_only_calls() {
        let mut token_only = serde_json::json!({ "element_token": "s00000001:2" });
        fill_pid_from_token(&mut token_only, |_| TokenOwner {
            pid: Some(42),
            current: Vec::new(),
            live: true,
        })
        .unwrap();
        assert_eq!(token_only["pid"], 42);

        let mut explicit = serde_json::json!({ "pid": 7, "element_token": "s00000001:2" });
        fill_pid_from_token(&mut explicit, |_| panic!("an explicit pid wins")).unwrap();
        assert_eq!(explicit["pid"], 7);

        let mut no_token = serde_json::json!({ "x": 1, "y": 2 });
        fill_pid_from_token(&mut no_token, |_| panic!("no token, no lookup")).unwrap();
        assert!(no_token.get("pid").is_none());

        let mut stale = serde_json::json!({ "pid": null, "element_token": "s00000009:0" });
        let refused = fill_pid_from_token(&mut stale, |_| TokenOwner::default()).unwrap_err();
        let structured = refused.structured_content.unwrap();
        assert_eq!(structured["refusal"]["code"], "stale_element_token");
    }

    #[test]
    fn token_round_trips_through_format_then_parse() {
        assert_eq!(token_for(0x1234, 42), "s00001234:42");
        assert_eq!(parse_token(&token_for(0x1234, 42)), Some((0x1234, 42)));
    }
    #[test]
    fn token_format_pads_to_eight_hex_chars() {
        assert_eq!(token_for(1, 0), "s00000001:0");
        assert_eq!(token_for(0, 999), "s00000000:999");
        assert_eq!(token_for(0x0001_0001, 3), "s00010001:3");
        assert_eq!(parse_token("s00010001:3"), Some((0x0001_0001, 3)));
    }
    #[test]
    fn parse_rejects_unknown_prefix_or_shape() {
        for token in [
            "",
            "x00001234:42",
            "s00001234",
            "s000012345:42",
            "s1234:42",
            "szzzzzzzz:42",
            "s00001234:abc",
        ] {
            assert!(parse_token(token).is_none(), "{token}");
        }
    }
}
