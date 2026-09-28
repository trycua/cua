use crate::protocol::ToolResult;
use std::sync::atomic::{AtomicU32, Ordering};

pub const LRU_CAP_PER_PID: usize = 8;
pub const STALE_TOKEN_ERROR: &str =
    "element_token is stale; call get_window_state again to refresh";

static SNAPSHOT_COUNTER: AtomicU32 = AtomicU32::new(1);

pub(crate) fn mint_snapshot_id() -> u32 {
    SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub fn token_for(snapshot_id: u32, element_index: usize) -> String {
    format!("s{snapshot_id:08x}:{element_index}")
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
        window_id: Option<u64>,
        element_index: usize,
        via_token: bool,
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
                ..
            } => (Some(element_index), window_id, Some(element)),
        }
    }
}

pub(crate) struct ElementReference {
    pub snapshot_id: u32,
    pub element_index: usize,
    pub via_token: bool,
    window_id: Option<u64>,
    conflicting: bool,
}

impl ElementReference {
    pub fn validate_window(&self, window_id: u64, tool: &str) -> Result<(), ToolResult> {
        if !self.conflicting && self.window_id.is_none_or(|supplied| supplied == window_id) {
            return Ok(());
        }
        let message = if self.via_token {
            format!("{tool}: element_token conflicts with element_index, snapshot_id, or window_id")
        } else {
            format!(
                "{tool}: snapshot belongs to window_id {window_id}, not {}",
                self.window_id.unwrap()
            )
        };
        Err(refusal("conflicting_element_target", message))
    }
}

pub(crate) fn refusal(code: &str, message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(serde_json::json!({
        "status": "refused", "refusal": { "code": code, "message": message }
    }))
}

pub(crate) fn parse_element_args(
    element_index: Option<usize>,
    element_token: Option<&str>,
    snapshot_handle: Option<&str>,
    window_id: Option<u64>,
    tool: &str,
) -> Result<Option<ElementReference>, ToolResult> {
    match (element_index, element_token, snapshot_handle) {
        (None, None, None) => return Ok(None),
        (None, None, Some(_)) => return Err(refusal(
            "element_index_required", format!("{tool}: snapshot_id requires element_index"),
        )),
        (Some(_), None, None) => return Err(refusal(
            "snapshot_id_required",
            format!("{tool}: bare element_index is not accepted; pass element_token, or snapshot_id together with element_index"),
        )),
        _ => {}
    }
    let (snapshot_id, index) = if let Some(token) = element_token {
        parse_token(token).ok_or_else(|| {
            refusal(
                "invalid_element_token",
                "element_token has invalid format".into(),
            )
        })?
    } else {
        let id = parse_snapshot_handle(snapshot_handle.unwrap()).ok_or_else(|| {
            refusal(
                "invalid_snapshot_id",
                format!("{tool}: snapshot_id has invalid format"),
            )
        })?;
        (id, element_index.unwrap())
    };
    Ok(Some(ElementReference {
        snapshot_id,
        element_index: index,
        via_token: element_token.is_some(),
        window_id,
        conflicting: element_index.is_some_and(|supplied| supplied != index)
            || snapshot_handle
                .is_some_and(|handle| parse_snapshot_handle(handle) != Some(snapshot_id)),
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
