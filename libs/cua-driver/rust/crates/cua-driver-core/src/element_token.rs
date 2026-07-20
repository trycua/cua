//! Opaque element capabilities issued by `get_window_state`.
//!
//! Tokens contain no pid, window, generation, index, or node identity. Each
//! token is a UUID v4 capability minted by the current registry instance, and
//! the registry stores the complete binding server-side. A token from a prior
//! daemon process is therefore unknown after restart instead of aliasing a new
//! snapshot whose counters happen to repeat.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;

pub const LRU_CAP_PER_PID: usize = 8;
pub const STALE_TOKEN_ERROR: &str =
    "element_token is stale; call get_window_state again to refresh";

pub const TOKEN_INVALID_CODE: &str = "element_token_invalid";
pub const TOKEN_UNKNOWN_CODE: &str = "element_token_unknown";
pub const TOKEN_PID_MISMATCH_CODE: &str = "element_token_pid_mismatch";
pub const TOKEN_WINDOW_MISMATCH_CODE: &str = "element_token_window_mismatch";
pub const TOKEN_INDEX_MISMATCH_CODE: &str = "element_token_index_mismatch";
pub const TOKEN_STALE_GENERATION_CODE: &str = "element_token_stale_generation";
pub const TOKEN_IDENTITY_MISMATCH_CODE: &str = "element_token_identity_mismatch";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableTokenError {
    pub code: &'static str,
    pub message: String,
}

impl StableTokenError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn into_tool_result(self) -> crate::protocol::ToolResult {
        crate::protocol::ToolResult::error(self.message.clone()).with_structured(
            serde_json::json!({
                "code": self.code,
                "message": self.message,
            }),
        )
    }

    pub fn stale_generation(generation: u32, pid: i32, window_id: u32) -> Self {
        Self::new(
            TOKEN_STALE_GENERATION_CODE,
            format!(
                "element_token generation {generation} is stale for pid={pid} \
                 window_id={window_id}; call get_window_state again"
            ),
        )
    }

    pub fn identity_mismatch(element_index: usize) -> Self {
        Self::new(
            TOKEN_IDENTITY_MISMATCH_CODE,
            format!(
                "element_token AX node identity no longer matches element_index \
                 {element_index}; call get_window_state again"
            ),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StableTokenBinding {
    pub pid: i32,
    pub window_id: u32,
    pub generation: u32,
    pub element_index: usize,
    pub node_identity: u64,
}

#[derive(Debug, Clone)]
struct TokenBinding {
    pid: i32,
    window_id: u32,
    generation: u32,
    element_index: usize,
    node_identity: Option<u64>,
}

#[derive(Debug)]
struct SnapshotEntry {
    generation: u32,
    window_id: u32,
    tokens: Vec<String>,
}

#[derive(Default)]
struct RegistryState {
    by_pid: HashMap<i32, Vec<SnapshotEntry>>,
    by_token: HashMap<String, TokenBinding>,
}

pub struct TokenRegistry {
    state: Mutex<RegistryState>,
    next_generation: AtomicU32,
}

impl TokenRegistry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState::default()),
            next_generation: AtomicU32::new(1),
        }
    }

    pub fn register_snapshot(&self, pid: i32, window_id: u32, element_count: usize) -> u32 {
        self.register_snapshot_inner(pid, window_id, element_count, HashMap::new())
    }

    pub fn register_snapshot_with_identities(
        &self,
        pid: i32,
        window_id: u32,
        element_count: usize,
        nodes: impl IntoIterator<Item = (usize, u64)>,
    ) -> u32 {
        self.register_snapshot_inner(pid, window_id, element_count, nodes.into_iter().collect())
    }

    fn register_snapshot_inner(
        &self,
        pid: i32,
        window_id: u32,
        element_count: usize,
        node_identities: HashMap<usize, u64>,
    ) -> u32 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap();
        let mut tokens = Vec::with_capacity(element_count);

        for element_index in 0..element_count {
            let token = mint_token(&state.by_token);
            state.by_token.insert(
                token.clone(),
                TokenBinding {
                    pid,
                    window_id,
                    generation,
                    element_index,
                    node_identity: node_identities.get(&element_index).copied(),
                },
            );
            tokens.push(token);
        }

        let evicted = {
            let lane = state.by_pid.entry(pid).or_default();
            lane.push(SnapshotEntry {
                generation,
                window_id,
                tokens,
            });
            let mut evicted = Vec::new();
            while lane.len() > LRU_CAP_PER_PID {
                evicted.push(lane.remove(0));
            }
            evicted
        };
        for snapshot in evicted {
            for token in snapshot.tokens {
                state.by_token.remove(&token);
            }
        }
        generation
    }

    pub fn token_for(&self, generation: u32, element_index: usize) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .by_pid
            .values()
            .flat_map(|lane| lane.iter())
            .find(|snapshot| snapshot.generation == generation)
            .and_then(|snapshot| snapshot.tokens.get(element_index))
            .cloned()
    }

    pub fn resolve_stable(
        &self,
        pid: i32,
        args_window_id: Option<u32>,
        args_element_index: Option<usize>,
        token: &str,
    ) -> Result<StableTokenBinding, StableTokenError> {
        validate_token_shape(token)?;
        let state = self.state.lock().unwrap();
        let binding = state.by_token.get(token).ok_or_else(|| {
            StableTokenError::new(
                TOKEN_UNKNOWN_CODE,
                "element_token is unknown to this cua-driver process",
            )
        })?;
        validate_binding(binding, pid, args_window_id, args_element_index)?;

        let current_generation = state
            .by_pid
            .get(&pid)
            .and_then(|lane| {
                lane.iter()
                    .rev()
                    .find(|snapshot| snapshot.window_id == binding.window_id)
            })
            .map(|snapshot| snapshot.generation);
        if current_generation != Some(binding.generation) {
            return Err(StableTokenError::stale_generation(
                binding.generation,
                pid,
                binding.window_id,
            ));
        }
        let node_identity = binding.node_identity.ok_or_else(|| {
            StableTokenError::new(
                TOKEN_UNKNOWN_CODE,
                format!(
                    "element_token has no AX node identity for element_index={} \
                     in generation={}",
                    binding.element_index, binding.generation
                ),
            )
        })?;
        Ok(StableTokenBinding {
            pid: binding.pid,
            window_id: binding.window_id,
            generation: binding.generation,
            element_index: binding.element_index,
            node_identity,
        })
    }

    pub fn resolve(&self, pid: i32, token: &str) -> Result<(u32, usize), String> {
        validate_token_shape(token).map_err(|error| error.message)?;
        let state = self.state.lock().unwrap();
        let binding = state
            .by_token
            .get(token)
            .ok_or_else(|| STALE_TOKEN_ERROR.to_string())?;
        if binding.pid != pid {
            return Err(STALE_TOKEN_ERROR.to_string());
        }
        let current_generation = state
            .by_pid
            .get(&pid)
            .and_then(|lane| {
                lane.iter()
                    .rev()
                    .find(|snapshot| snapshot.window_id == binding.window_id)
            })
            .map(|snapshot| snapshot.generation);
        if current_generation != Some(binding.generation) {
            return Err(STALE_TOKEN_ERROR.to_string());
        }
        Ok((binding.window_id, binding.element_index))
    }

    #[allow(dead_code)]
    pub fn format(generation: u32, element_index: usize) -> String {
        token_for(generation, element_index)
    }

    #[cfg(test)]
    fn snapshot_count(&self, pid: i32) -> usize {
        self.state
            .lock()
            .unwrap()
            .by_pid
            .get(&pid)
            .map(Vec::len)
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn clear(&self) {
        *self.state.lock().unwrap() = RegistryState::default();
    }
}

impl Default for TokenRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn mint_token(existing: &HashMap<String, TokenBinding>) -> String {
    loop {
        let token = format!("e_{}", Uuid::new_v4().simple());
        if !existing.contains_key(&token) {
            return token;
        }
    }
}

fn validate_token_shape(token: &str) -> Result<(), StableTokenError> {
    let raw = token.strip_prefix("e_").ok_or_else(|| {
        StableTokenError::new(TOKEN_INVALID_CODE, "element_token has invalid format")
    })?;
    let uuid = Uuid::parse_str(raw).map_err(|_| {
        StableTokenError::new(TOKEN_INVALID_CODE, "element_token has invalid format")
    })?;
    if uuid.get_version_num() != 4 || uuid.get_variant() != uuid::Variant::RFC4122 {
        return Err(StableTokenError::new(
            TOKEN_INVALID_CODE,
            "element_token has invalid format",
        ));
    }
    Ok(())
}

fn validate_binding(
    binding: &TokenBinding,
    pid: i32,
    args_window_id: Option<u32>,
    args_element_index: Option<usize>,
) -> Result<(), StableTokenError> {
    if binding.pid != pid {
        return Err(StableTokenError::new(
            TOKEN_PID_MISMATCH_CODE,
            format!(
                "element_token belongs to pid={}, not requested pid={pid}",
                binding.pid
            ),
        ));
    }
    if let Some(window_id) = args_window_id {
        if binding.window_id != window_id {
            return Err(StableTokenError::new(
                TOKEN_WINDOW_MISMATCH_CODE,
                format!(
                    "element_token belongs to window_id={}, not requested window_id={window_id}",
                    binding.window_id
                ),
            ));
        }
    }
    if let Some(element_index) = args_element_index {
        if binding.element_index != element_index {
            return Err(StableTokenError::new(
                TOKEN_INDEX_MISMATCH_CODE,
                format!(
                    "element_token identifies element_index={}, not requested \
                     element_index={element_index}",
                    binding.element_index
                ),
            ));
        }
    }
    Ok(())
}

pub fn global() -> &'static TokenRegistry {
    static REGISTRY: OnceLock<TokenRegistry> = OnceLock::new();
    REGISTRY.get_or_init(TokenRegistry::new)
}

pub fn token_for(generation: u32, element_index: usize) -> String {
    global()
        .token_for(generation, element_index)
        .expect("token requested for an unregistered snapshot element")
}

pub fn snapshot_id(generation: u32) -> String {
    format!("s{generation:08x}")
}

pub fn add_snapshot_metadata(structured: &mut serde_json::Value, generation: u32) {
    structured["snapshot_id"] = serde_json::json!(snapshot_id(generation));
    structured["generation"] = serde_json::json!(generation);
}

pub fn format_token(generation: u32, element_index: usize) -> String {
    token_for(generation, element_index)
}

#[derive(Debug, Clone)]
pub enum ResolvedElement {
    None,
    Element {
        window_id: Option<u32>,
        element_index: usize,
        via_token: bool,
    },
}

pub fn resolve_element_args(
    pid: i32,
    args_element_index: Option<usize>,
    args_element_token: Option<&str>,
    args_window_id: Option<u32>,
    _tool_name: &str,
) -> Result<ResolvedElement, crate::protocol::ToolResult> {
    match (args_element_index, args_element_token) {
        (None, None) => Ok(ResolvedElement::None),
        (Some(element_index), None) => Ok(ResolvedElement::Element {
            window_id: args_window_id,
            element_index,
            via_token: false,
        }),
        (element_index, Some(token)) => {
            let (window_id, resolved_index) = global()
                .resolve(pid, token)
                .map_err(crate::protocol::ToolResult::error)?;
            if let Some(element_index) = element_index {
                if element_index != resolved_index {
                    return Err(StableTokenError::new(
                        TOKEN_INDEX_MISMATCH_CODE,
                        format!(
                            "element_token identifies element_index={resolved_index}, not \
                             requested element_index={element_index}"
                        ),
                    )
                    .into_tool_result());
                }
            }
            if let Some(args_window_id) = args_window_id {
                if args_window_id != window_id {
                    return Err(StableTokenError::new(
                        TOKEN_WINDOW_MISMATCH_CODE,
                        format!(
                            "element_token belongs to window_id={window_id}, not requested \
                             window_id={args_window_id}"
                        ),
                    )
                    .into_tool_result());
                }
            }
            Ok(ResolvedElement::Element {
                window_id: Some(window_id),
                element_index: resolved_index,
                via_token: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(registry: &TokenRegistry, generation: u32, index: usize) -> String {
        registry.token_for(generation, index).unwrap()
    }

    #[test]
    fn tokens_are_uuid_v4_opaque_capabilities() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot(10, 20, 2);
        let first = token(&registry, generation, 0);
        let second = token(&registry, generation, 1);
        assert_ne!(first, second);
        validate_token_shape(&first).unwrap();
        assert!(!first.contains("10"));
        assert!(!first.contains(":0"));
        assert!(!first.contains(&format!("{generation:08x}")));
    }

    #[test]
    fn empty_snapshot_preserves_cross_platform_metadata_schema_without_element_token() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot(10, 20, 0);
        assert_eq!(registry.token_for(generation, 0), None);

        let mut structured = serde_json::json!({});
        add_snapshot_metadata(&mut structured, generation);
        assert_eq!(
            structured["snapshot_id"],
            serde_json::json!(format!("s{generation:08x}"))
        );
        assert!(structured["snapshot_id"].is_string());
        assert_eq!(structured["generation"], serde_json::json!(generation));
        assert!(structured["generation"].is_number());
    }

    #[test]
    fn registry_instances_issue_unique_tokens_and_reject_cross_restart_replay() {
        let before_restart = TokenRegistry::new();
        let old_generation = before_restart.register_snapshot(10, 20, 1);
        let old_token = token(&before_restart, old_generation, 0);

        let after_restart = TokenRegistry::new();
        let new_generation = after_restart.register_snapshot(10, 20, 1);
        let new_token = token(&after_restart, new_generation, 0);

        assert_ne!(old_token, new_token);
        assert_eq!(
            after_restart
                .resolve_stable(10, Some(20), Some(0), &old_token)
                .unwrap_err()
                .code,
            TOKEN_UNKNOWN_CODE
        );
    }

    #[test]
    fn token_tamper_fails_closed() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot_with_identities(10, 20, 1, [(0, 0xfeed)]);
        let original = token(&registry, generation, 0);
        let mut tampered = original.into_bytes();
        let last = tampered.last_mut().unwrap();
        *last = if *last == b'a' { b'b' } else { b'a' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert_eq!(
            registry
                .resolve_stable(10, Some(20), Some(0), &tampered)
                .unwrap_err()
                .code,
            TOKEN_UNKNOWN_CODE
        );
    }

    #[test]
    fn stable_token_binds_pid_window_generation_index_and_identity() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot_with_identities(10, 20, 5, [(4, 0xfeed)]);
        let binding = registry
            .resolve_stable(10, Some(20), Some(4), &token(&registry, generation, 4))
            .unwrap();
        assert_eq!(
            binding,
            StableTokenBinding {
                pid: 10,
                window_id: 20,
                generation,
                element_index: 4,
                node_identity: 0xfeed,
            }
        );
    }

    #[test]
    fn pid_window_and_index_forgery_fail_closed() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot_with_identities(10, 20, 3, [(2, 9)]);
        let token = token(&registry, generation, 2);
        assert_eq!(
            registry
                .resolve_stable(11, Some(20), Some(2), &token)
                .unwrap_err()
                .code,
            TOKEN_PID_MISMATCH_CODE
        );
        assert_eq!(
            registry
                .resolve_stable(10, Some(21), Some(2), &token)
                .unwrap_err()
                .code,
            TOKEN_WINDOW_MISMATCH_CODE
        );
        assert_eq!(
            registry
                .resolve_stable(10, Some(20), Some(1), &token)
                .unwrap_err()
                .code,
            TOKEN_INDEX_MISMATCH_CODE
        );
    }

    #[test]
    fn newer_snapshot_makes_stable_token_stale() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot_with_identities(10, 20, 1, [(0, 7)]);
        let stale = token(&registry, generation, 0);
        registry.register_snapshot_with_identities(10, 20, 1, [(0, 8)]);
        assert_eq!(
            registry
                .resolve_stable(10, Some(20), Some(0), &stale)
                .unwrap_err()
                .code,
            TOKEN_STALE_GENERATION_CODE
        );
        assert_eq!(
            registry.resolve(10, &stale).unwrap_err(),
            STALE_TOKEN_ERROR,
            "generic token consumers must not resolve an index from a superseded snapshot"
        );
    }

    #[test]
    fn lru_eviction_removes_capabilities() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot(13, 1, 1);
        let oldest = token(&registry, generation, 0);
        for _ in 0..LRU_CAP_PER_PID {
            registry.register_snapshot(13, 1, 1);
        }
        assert_eq!(registry.snapshot_count(13), LRU_CAP_PER_PID);
        assert_eq!(
            registry.resolve(13, &oldest).unwrap_err(),
            STALE_TOKEN_ERROR
        );
    }

    #[test]
    fn malformed_token_is_distinct_from_unknown_token() {
        let registry = TokenRegistry::new();
        assert_eq!(
            registry
                .resolve_stable(1, None, None, "s00000001:0")
                .unwrap_err()
                .code,
            TOKEN_INVALID_CODE
        );
    }

    #[test]
    fn clear_removes_all_capabilities() {
        let registry = TokenRegistry::new();
        let generation = registry.register_snapshot(1, 1, 1);
        let token = token(&registry, generation, 0);
        registry.clear();
        assert_eq!(registry.snapshot_count(1), 0);
        assert_eq!(registry.resolve(1, &token).unwrap_err(), STALE_TOKEN_ERROR);
    }

    #[test]
    fn global_registry_is_shared_across_calls() {
        assert!(std::ptr::eq(global(), global()));
    }

    #[test]
    fn element_index_alone_still_works() {
        let resolved = resolve_element_args(1, Some(7), None, Some(99), "click").unwrap();
        assert!(matches!(
            resolved,
            ResolvedElement::Element {
                window_id: Some(99),
                element_index: 7,
                via_token: false,
            }
        ));
    }

    #[test]
    fn global_token_resolves_and_index_forgery_errors() {
        let pid = 0x7fff_0001_i32;
        let generation = global().register_snapshot(pid, 555, 4);
        let token = token_for(generation, 2);
        let resolved = resolve_element_args(pid, None, Some(&token), None, "click").unwrap();
        assert!(matches!(
            resolved,
            ResolvedElement::Element {
                window_id: Some(555),
                element_index: 2,
                via_token: true,
            }
        ));
        assert!(resolve_element_args(pid, Some(3), Some(&token), Some(555), "click").is_err());
    }

    #[test]
    fn stable_token_errors_expose_structured_code_and_message() {
        let result = StableTokenError::new(TOKEN_UNKNOWN_CODE, "unknown token").into_tool_result();
        assert_eq!(result.is_error, Some(true));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], TOKEN_UNKNOWN_CODE);
        assert_eq!(structured["message"], "unknown token");
    }
}
