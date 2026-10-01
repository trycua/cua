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

pub(crate) fn refusal(code: &str, message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused", "refusal": { "code": code, "message": message }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
