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

/// The refusal for [`STALE_TOKEN_WITHOUT_PID`]: `stale_element_token`, with
/// the hint that a current token names its own pid.
pub fn stale_token_without_pid() -> ToolResult {
    ToolResult::error(format!(
        "{STALE_TOKEN_WITHOUT_PID} (a current token names its own pid)."
    ))
    .with_structured(serde_json::json!({
        "status": "refused",
        "refusal": { "code": "stale_element_token", "message": STALE_TOKEN_WITHOUT_PID },
    }))
}

/// Fill a missing `pid` from the call's `element_token`. A token names its
/// snapshot, and so its window and process, so a token-only call needs no
/// pid. An explicit `pid` wins and a call without a token is left as is (the
/// tool reports its own missing-pid error). A token whose snapshot is gone
/// is refused as `stale_element_token` rather than as a missing pid.
pub fn fill_pid_from_token(
    args: &mut serde_json::Value,
    pid_for_token: impl FnOnce(&serde_json::Value) -> Option<i32>,
) -> Result<(), ToolResult> {
    let present = |key: &str| args.get(key).is_some_and(|value| !value.is_null());
    if present("pid") || !present("element_token") {
        return Ok(());
    }
    let pid = pid_for_token(args).ok_or_else(stale_token_without_pid)?;
    if let Some(object) = args.as_object_mut() {
        object.insert("pid".to_owned(), pid.into());
    }
    Ok(())
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
        fill_pid_from_token(&mut token_only, |_| Some(42)).unwrap();
        assert_eq!(token_only["pid"], 42);

        let mut explicit = serde_json::json!({ "pid": 7, "element_token": "s00000001:2" });
        fill_pid_from_token(&mut explicit, |_| panic!("an explicit pid wins")).unwrap();
        assert_eq!(explicit["pid"], 7);

        let mut no_token = serde_json::json!({ "x": 1, "y": 2 });
        fill_pid_from_token(&mut no_token, |_| panic!("no token, no lookup")).unwrap();
        assert!(no_token.get("pid").is_none());

        let mut stale = serde_json::json!({ "pid": null, "element_token": "s00000009:0" });
        let refused = fill_pid_from_token(&mut stale, |_| None).unwrap_err();
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
