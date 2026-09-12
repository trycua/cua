use crate::protocol::ToolResult;
use std::sync::atomic::{AtomicU32, Ordering};

pub const LRU_CAP_PER_PID: usize = 8;
pub const STALE_TOKEN_ERROR: &str =
    "element_token is stale; call get_window_state again to refresh";

static SNAPSHOT_COUNTER: AtomicU32 = AtomicU32::new(1);

pub(crate) fn mint_snapshot_id() -> u32 {
    SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

pub fn format_token(snapshot_id: u32, element_index: usize) -> String {
    format!("s{snapshot_id:08x}:{element_index}")
}

pub fn token_for(snapshot_id: u32, element_index: usize) -> String {
    format_token(snapshot_id, element_index)
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
    use crate::element_cache::{
        current_runtime_cache, register_runtime_cache, ElementCacheCore, SnapshotPayload,
    };
    use crate::tool::with_runtime_scope;
    use std::sync::Arc;

    struct Payload(Vec<usize>);
    impl SnapshotPayload for Payload {
        type Element = usize;
        fn len(&self) -> usize {
            self.0.len()
        }
        fn retain(&self, index: usize) -> Option<usize> {
            self.0.get(index).copied()
        }
    }
    fn cache() -> ElementCacheCore<Payload> {
        ElementCacheCore::new()
    }
    fn publish(cache: &ElementCacheCore<Payload>, pid: i32, window: u64, count: usize) -> u32 {
        cache.publish(pid, window, Payload((0..count).collect()))
    }
    fn resolve(
        cache: &ElementCacheCore<Payload>,
        pid: i32,
        token: &str,
    ) -> Result<(u64, usize), String> {
        cache
            .resolve_element_args(pid, None, Some(token), None, None, "click")
            .map(|result| match result {
                ResolvedElement::Element {
                    window_id: Some(window),
                    element_index,
                    element,
                    ..
                } => {
                    assert_eq!(element, element_index);
                    (window, element_index)
                }
                _ => panic!("expected element"),
            })
            .map_err(|error| {
                error.structured_content.unwrap()["refusal"]["message"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
    }

    #[test]
    fn token_round_trips_through_format_then_parse() {
        assert_eq!(format_token(0x1234, 42), "s00001234:42");
        assert_eq!(parse_token(&format_token(0x1234, 42)), Some((0x1234, 42)));
    }
    #[test]
    fn token_format_pads_to_eight_hex_chars() {
        assert_eq!(format_token(1, 0), "s00000001:0");
        assert_eq!(format_token(0, 999), "s00000000:999");
        assert_eq!(format_token(0x0001_0001, 3), "s00010001:3");
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
    #[test]
    fn register_then_resolve_returns_window_and_index() {
        let cache = cache();
        let id = publish(&cache, 100, 42, 5);
        assert_eq!(resolve(&cache, 100, &format_token(id, 3)), Ok((42, 3)));
    }
    #[test]
    fn resolve_with_unknown_pid_returns_stale_error() {
        assert_eq!(
            resolve(&cache(), 999, &format_token(0x1234, 0)),
            Err(STALE_TOKEN_ERROR.into())
        );
    }
    #[test]
    fn resolve_with_bad_format_returns_invalid_error() {
        let cache = cache();
        publish(&cache, 10, 1, 1);
        assert!(resolve(&cache, 10, "garbage")
            .unwrap_err()
            .contains("invalid format"));
    }
    #[test]
    fn out_of_range_index_returns_actionable_error() {
        let cache = cache();
        let id = publish(&cache, 11, 1, 3);
        assert!(resolve(&cache, 11, &format_token(id, 7))
            .unwrap_err()
            .contains("out of range"));
    }
    #[test]
    fn next_snapshot_for_same_window_invalidates_old_immediately() {
        let cache = cache();
        let first = publish(&cache, 12, 1, 5);
        let second = publish(&cache, 12, 1, 5);
        assert_eq!(
            resolve(&cache, 12, &format_token(first, 0)),
            Err(STALE_TOKEN_ERROR.into())
        );
        assert_eq!(resolve(&cache, 12, &format_token(second, 0)), Ok((1, 0)));
    }
    #[test]
    fn snapshots_for_different_windows_share_the_bounded_lru() {
        let cache = cache();
        let first = publish(&cache, 12, 1, 5);
        let second = publish(&cache, 12, 2, 5);
        assert_eq!(resolve(&cache, 12, &format_token(first, 0)), Ok((1, 0)));
        assert_eq!(resolve(&cache, 12, &format_token(second, 0)), Ok((2, 0)));
    }
    #[test]
    fn lru_eviction_invalidates_oldest_snapshot() {
        let cache = cache();
        let ids: Vec<_> = (1..=LRU_CAP_PER_PID as u64 + 1)
            .map(|window| publish(&cache, 13, window, 5))
            .collect();
        assert_eq!(
            resolve(&cache, 13, &format_token(ids[0], 0)),
            Err(STALE_TOKEN_ERROR.into())
        );
        assert_eq!(
            ids.iter()
                .filter(|id| resolve(&cache, 13, &format_token(**id, 0)).is_ok())
                .count(),
            LRU_CAP_PER_PID
        );
    }
    #[test]
    fn tokens_in_different_pids_dont_collide() {
        let cache = cache();
        let first = publish(&cache, 100, 11, 3);
        let second = publish(&cache, 200, 22, 3);
        assert_eq!(resolve(&cache, 100, &format_token(first, 0)), Ok((11, 0)));
        assert_eq!(resolve(&cache, 200, &format_token(second, 0)), Ok((22, 0)));
        assert_eq!(
            resolve(&cache, 200, &format_token(first, 0)),
            Err(STALE_TOKEN_ERROR.into())
        );
    }
    #[test]
    fn runtime_cache_discovery_is_weak_and_shared_across_calls() {
        with_runtime_scope("token-discovery-test".into(), || {
            let cache = Arc::new(cache());
            register_runtime_cache(&cache);
            assert!(Arc::ptr_eq(
                &cache,
                &current_runtime_cache::<Payload>().unwrap()
            ));
            drop(cache);
            assert!(current_runtime_cache::<Payload>().is_none());
        });
    }
    #[test]
    fn stale_token_returns_explicit_error_not_silent_misclick() {
        let cache = cache();
        let first = publish(&cache, 14, 1, 5);
        for _ in 0..LRU_CAP_PER_PID {
            publish(&cache, 14, 1, 5);
        }
        assert_eq!(
            resolve(&cache, 14, &format_token(first, 2)),
            Err(STALE_TOKEN_ERROR.into())
        );
    }
    #[test]
    fn clear_then_register_starts_clean() {
        let cache = cache();
        let first = publish(&cache, 1, 1, 1);
        assert_eq!(cache.clear(), 1);
        assert_eq!(cache.clear(), 0);
        assert_eq!(
            resolve(&cache, 1, &format_token(first, 0)),
            Err(STALE_TOKEN_ERROR.into())
        );
        let second = publish(&cache, 1, 1, 1);
        assert_eq!(resolve(&cache, 1, &format_token(second, 0)), Ok((1, 0)));
    }
    #[test]
    fn bare_element_index_is_refused() {
        assert!(cache()
            .resolve_element_args(1, Some(7), None, None, Some(99), "click")
            .is_err());
    }
    #[test]
    fn element_token_alone_resolves_to_same_action() {
        let cache = cache();
        let id = publish(&cache, 1, 555, 4);
        let resolved = cache
            .resolve_element_args(1, None, Some(&format_token(id, 2)), None, None, "click")
            .unwrap();
        assert!(matches!(
            resolved,
            ResolvedElement::Element {
                window_id: Some(555),
                element_index: 2,
                via_token: true,
                element: 2
            }
        ));
    }
    #[test]
    fn conflicting_token_and_index_are_refused() {
        let cache = cache();
        let id = publish(&cache, 1, 777, 5);
        assert!(cache
            .resolve_element_args(1, Some(99), Some(&format_token(id, 3)), None, None, "click")
            .is_err());
    }
    #[test]
    fn snapshot_id_and_index_resolve_safely() {
        let cache = cache();
        let id = publish(&cache, 1, 888, 5);
        let result = cache
            .resolve_element_args(
                1,
                Some(2),
                None,
                Some(&format!("s{id:08x}")),
                Some(888),
                "click",
            )
            .unwrap();
        assert!(matches!(
            result,
            ResolvedElement::Element {
                window_id: Some(888),
                element_index: 2,
                via_token: false,
                element: 2
            }
        ));
    }
    #[test]
    fn token_only_stale_returns_error_not_silent_fallback_to_integer() {
        let result = cache()
            .resolve_element_args(
                1,
                Some(0),
                Some(&format_token(0xdead, 0)),
                None,
                Some(1),
                "click",
            )
            .unwrap_err();
        assert!(result.is_error.unwrap_or(false));
    }
    #[test]
    fn neither_returns_none() {
        assert!(matches!(
            cache()
                .resolve_element_args(1, None, None, None, None, "click")
                .unwrap(),
            ResolvedElement::None
        ));
    }
}
