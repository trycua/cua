use crate::protocol::ToolResult;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};

type HmacSha256 = Hmac<Sha256>;
const SNAPSHOT_VERSION: &str = "sa1";
const TOKEN_VERSION: &str = "et1";
const TAG_BYTES: usize = 16;
static SNAPSHOT_COUNTER: AtomicU32 = AtomicU32::new(1);

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

#[derive(Debug, Clone)]
pub struct ElementTarget {
    pub element_index: usize,
    identity_tag: Option<[u8; TAG_BYTES]>,
}
impl ElementTarget {
    pub fn matches_identity(&self, identity: &[u8]) -> bool {
        self.identity_tag
            .is_none_or(|expected| constant_time_eq(&expected, &identity_tag(identity)))
    }
    pub fn has_identity(&self) -> bool {
        self.identity_tag.is_some()
    }

    pub fn resolve_unique<T>(
        &self,
        candidates: impl IntoIterator<Item = (Vec<u8>, T)>,
        complete: bool,
    ) -> Result<Option<T>, String> {
        if !complete {
            return Err("incomplete accessibility tree cannot establish a unique element".into());
        }
        let mut matches = candidates
            .into_iter()
            .filter_map(|(identity, element)| self.matches_identity(&identity).then_some(element));
        let first = matches.next();
        Ok(first.filter(|_| matches.next().is_none()))
    }
}
#[derive(Debug, Clone)]
struct SnapshotAddress {
    encoded: String,
    pid: i32,
    window_id: u64,
}
#[derive(Debug)]
enum ParseFailure {
    Invalid,
    GenerationMismatch,
}

pub fn mint_snapshot_handle(pid: i32, window_id: u64) -> String {
    let scope = current_runtime_scope();
    let generation = generation_tag(&scope);
    let nonce = SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let body = format!(
        "{SNAPSHOT_VERSION}.{}.{}.{window_id:016x}.{nonce:08x}",
        hex(&generation),
        pid as u32
    );
    format!(
        "{body}.{}",
        hex(&mac_tag(&scope, b"snapshot", body.as_bytes()))
    )
}
pub fn token_for(snapshot_handle: &str, element_index: usize) -> String {
    token_for_identity(snapshot_handle, element_index, &[])
        .expect("snapshot handle minted in the current runtime")
}
pub fn format_token(snapshot_handle: &str, element_index: usize) -> String {
    token_for(snapshot_handle, element_index)
}
pub fn token_for_identity(
    snapshot_handle: &str,
    element_index: usize,
    identity: &[u8],
) -> Result<String, ToolResult> {
    let snapshot = parse_snapshot(snapshot_handle).map_err(snapshot_failure)?;
    let scope = current_runtime_scope();
    let identity = (!identity.is_empty()).then(|| identity_tag(identity));
    let identity_hex = identity.map_or_else(|| "-".into(), |tag| hex(&tag));
    let body = format!(
        "{TOKEN_VERSION}.{}.{element_index}.{identity_hex}",
        snapshot.encoded
    );
    Ok(format!(
        "{body}.{}",
        hex(&mac_tag(&scope, b"element", body.as_bytes()))
    ))
}
pub fn parse_token(token: &str) -> Option<(String, usize)> {
    let parsed = parse_element_token(token).ok()?;
    Some((parsed.snapshot.encoded, parsed.element_index))
}
pub fn parse_snapshot_handle(handle: &str) -> Option<(i32, u64)> {
    let parsed = parse_snapshot(handle).ok()?;
    Some((parsed.pid, parsed.window_id))
}

pub fn resolve_element_args<T, F>(
    pid: i32,
    element_index: Option<usize>,
    element_token: Option<&str>,
    snapshot_handle: Option<&str>,
    window_id: Option<u64>,
    tool: &str,
    resolve_fresh: F,
) -> Result<ResolvedElement<T>, ToolResult>
where
    F: FnOnce(u64, &ElementTarget) -> Result<Option<T>, String>,
{
    match (element_index, element_token, snapshot_handle) {
        (None, None, None) => return Ok(ResolvedElement::None),
        (None, None, Some(_)) => return Err(refusal("element_index_required", format!("{tool}: snapshot_id requires element_index"))),
        (Some(_), None, None) => return Err(refusal("snapshot_id_required", format!("{tool}: bare element_index is not accepted; pass element_token, or snapshot_id together with element_index"))),
        _ => {}
    }
    let (snapshot, target, via_token) = if let Some(token) = element_token {
        let parsed = parse_element_token(token).map_err(token_failure)?;
        if element_index.is_some_and(|index| index != parsed.element_index)
            || snapshot_handle.is_some_and(|handle| handle != parsed.snapshot.encoded)
        {
            return Err(conflicting_target(
                tool,
                true,
                parsed.snapshot.window_id,
                window_id,
            ));
        }
        (
            parsed.snapshot,
            ElementTarget {
                element_index: parsed.element_index,
                identity_tag: parsed.identity_tag,
            },
            true,
        )
    } else {
        let snapshot = parse_snapshot(snapshot_handle.unwrap()).map_err(snapshot_failure)?;
        (
            snapshot,
            ElementTarget {
                element_index: element_index.unwrap(),
                identity_tag: None,
            },
            false,
        )
    };
    if snapshot.pid != pid || window_id.is_some_and(|window| window != snapshot.window_id) {
        return Err(conflicting_target(
            tool,
            via_token,
            snapshot.window_id,
            window_id,
        ));
    }
    if !target.has_identity() {
        return Err(refusal(
            "element_identity_required",
            format!("{tool}: pass an identity-bearing element_token from get_window_state; an index alone cannot identify an observed control"),
        ));
    }
    let element = resolve_fresh(snapshot.window_id, &target).map_err(|message| refusal("element_resolution_failed", message))?.ok_or_else(|| refusal("invalid_element_token", format!("element_token element_index {} does not identify an actionable element in the current accessibility state", target.element_index)))?;
    Ok(ResolvedElement::Element {
        window_id: Some(snapshot.window_id),
        element_index: target.element_index,
        via_token,
        element,
    })
}

/// Resolve native state without blocking the async tool executor.
pub async fn resolve_native<T, F>(
    pid: i32,
    element_index: Option<usize>,
    element_token: Option<&str>,
    snapshot_handle: Option<&str>,
    window_id: Option<u64>,
    tool: &str,
    resolve_fresh: F,
) -> Result<ResolvedElement<T>, ToolResult>
where
    T: Send + 'static,
    F: FnOnce(u64, &ElementTarget) -> Result<Option<T>, String> + Send + 'static,
{
    if element_index.is_none() && element_token.is_none() && snapshot_handle.is_none() {
        return Ok(ResolvedElement::None);
    }
    let scope = current_runtime_scope();
    let element_token = element_token.map(str::to_owned);
    let snapshot_handle = snapshot_handle.map(str::to_owned);
    let tool = tool.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::tool::with_runtime_scope(scope, || {
            resolve_element_args(
                pid,
                element_index,
                element_token.as_deref(),
                snapshot_handle.as_deref(),
                window_id,
                &tool,
                resolve_fresh,
            )
        })
    })
    .await
    .map_err(|error| {
        refusal(
            "element_resolution_failed",
            format!("native lookup worker failed: {error}"),
        )
    })?
}

struct ParsedElementToken {
    snapshot: SnapshotAddress,
    element_index: usize,
    identity_tag: Option<[u8; TAG_BYTES]>,
}
fn parse_element_token(token: &str) -> Result<ParsedElementToken, ParseFailure> {
    let mut fields = token.rsplitn(4, '.');
    let mac = decode_tag(fields.next().ok_or(ParseFailure::Invalid)?)?;
    let identity_field = fields.next().ok_or(ParseFailure::Invalid)?;
    let element_index = fields
        .next()
        .ok_or(ParseFailure::Invalid)?
        .parse()
        .map_err(|_| ParseFailure::Invalid)?;
    let prefix_and_snapshot = fields.next().ok_or(ParseFailure::Invalid)?;
    let snapshot = prefix_and_snapshot
        .strip_prefix(&format!("{TOKEN_VERSION}."))
        .ok_or(ParseFailure::Invalid)?;
    let snapshot = parse_snapshot(snapshot)?;
    let body_len = token.len() - mac.len() * 2 - 1;
    let expected = mac_tag(
        &current_runtime_scope(),
        b"element",
        token[..body_len].as_bytes(),
    );
    if !constant_time_eq(&mac, &expected) {
        return Err(ParseFailure::Invalid);
    }
    let identity_tag = if identity_field == "-" {
        None
    } else {
        Some(decode_tag(identity_field)?)
    };
    Ok(ParsedElementToken {
        snapshot,
        element_index,
        identity_tag,
    })
}
fn parse_snapshot(handle: &str) -> Result<SnapshotAddress, ParseFailure> {
    let fields: Vec<_> = handle.split('.').collect();
    if fields.len() != 6 || fields[0] != SNAPSHOT_VERSION {
        return Err(ParseFailure::Invalid);
    }
    let generation = decode_tag(fields[1])?;
    if !constant_time_eq(&generation, &generation_tag(&current_runtime_scope())) {
        return Err(ParseFailure::GenerationMismatch);
    }
    let pid = u32::from_str_radix(fields[2], 10)
        .map(|pid| pid as i32)
        .map_err(|_| ParseFailure::Invalid)?;
    let window_id = u64::from_str_radix(fields[3], 16).map_err(|_| ParseFailure::Invalid)?;
    u32::from_str_radix(fields[4], 16).map_err(|_| ParseFailure::Invalid)?;
    let mac = decode_tag(fields[5])?;
    let body_len = handle.len() - fields[5].len() - 1;
    let expected = mac_tag(
        &current_runtime_scope(),
        b"snapshot",
        handle[..body_len].as_bytes(),
    );
    if !constant_time_eq(&mac, &expected) {
        return Err(ParseFailure::Invalid);
    }
    Ok(SnapshotAddress {
        encoded: handle.to_owned(),
        pid,
        window_id,
    })
}
fn current_runtime_scope() -> String {
    crate::tool::current_dispatch_runtime_scope().unwrap_or_else(|| "legacy".into())
}
fn generation_tag(scope: &str) -> [u8; TAG_BYTES] {
    Sha256::digest(scope.as_bytes())[..TAG_BYTES]
        .try_into()
        .unwrap()
}
fn identity_tag(identity: &[u8]) -> [u8; TAG_BYTES] {
    mac_tag(&current_runtime_scope(), b"identity", identity)
}
fn mac_tag(scope: &str, domain: &[u8], body: &[u8]) -> [u8; TAG_BYTES] {
    let mut mac = HmacSha256::new_from_slice(scope.as_bytes()).expect("HMAC accepts any key size");
    mac.update(domain);
    mac.update(&[0]);
    mac.update(body);
    mac.finalize().into_bytes()[..TAG_BYTES].try_into().unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn decode_tag(value: &str) -> Result<[u8; TAG_BYTES], ParseFailure> {
    if value.len() != TAG_BYTES * 2 {
        return Err(ParseFailure::Invalid);
    }
    let mut out = [0_u8; TAG_BYTES];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| ParseFailure::Invalid)?;
    }
    Ok(out)
}
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
            == 0
}
fn token_failure(failure: ParseFailure) -> ToolResult {
    match failure {
        ParseFailure::GenerationMismatch => refusal(
            "generation_mismatch",
            "element_token belongs to another runtime generation".into(),
        ),
        ParseFailure::Invalid => refusal(
            "invalid_element_token",
            "element_token has invalid format or authentication".into(),
        ),
    }
}
fn snapshot_failure(failure: ParseFailure) -> ToolResult {
    match failure {
        ParseFailure::GenerationMismatch => refusal(
            "generation_mismatch",
            "snapshot_id belongs to another runtime generation".into(),
        ),
        ParseFailure::Invalid => refusal(
            "invalid_snapshot_id",
            "snapshot_id has invalid format or authentication".into(),
        ),
    }
}
fn conflicting_target(
    tool: &str,
    via_token: bool,
    snapshot_window: u64,
    supplied_window: Option<u64>,
) -> ToolResult {
    let message = if via_token {
        format!(
            "{tool}: element_token conflicts with element_index, snapshot_id, pid, or window_id"
        )
    } else {
        format!(
            "{tool}: snapshot belongs to window_id {snapshot_window}, not {}",
            supplied_window.unwrap_or(snapshot_window)
        )
    };
    refusal("conflicting_element_target", message)
}
pub(crate) fn refusal(code: &str, message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(
        serde_json::json!({"status":"refused","refusal":{"code":code,"message":message}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::with_runtime_scope;
    fn code(error: ToolResult) -> String {
        error.structured_content.unwrap()["refusal"]["code"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    #[test]
    fn token_resolves_without_lookup_table_or_cache_owner() {
        with_runtime_scope("stateless-token-runtime".into(), || {
            let snapshot = mint_snapshot_handle(42, 7);
            let token = token_for_identity(&snapshot, 3, b"button:save").unwrap();
            let resolved = resolve_element_args(
                42,
                None,
                Some(&token),
                None,
                None,
                "click",
                |window, target| {
                    assert_eq!(window, 7);
                    assert!(target.matches_identity(b"button:save"));
                    Ok(Some(99))
                },
            )
            .unwrap();
            assert!(matches!(
                resolved,
                ResolvedElement::Element {
                    window_id: Some(7),
                    element_index: 3,
                    element: 99,
                    ..
                }
            ));
        });
    }
    #[test]
    fn malformed_and_tampered_tokens_fail_closed() {
        with_runtime_scope("token-tamper-runtime".into(), || {
            let snapshot = mint_snapshot_handle(42, 7);
            let mut token = token_for_identity(&snapshot, 3, b"button:save").unwrap();
            token.push('0');
            assert_eq!(
                code(
                    resolve_element_args::<(), _>(
                        42,
                        None,
                        Some(&token),
                        None,
                        None,
                        "click",
                        |_, _| Ok(Some(()))
                    )
                    .unwrap_err()
                ),
                "invalid_element_token"
            );
        });
    }
    #[test]
    fn runtime_generation_mismatch_is_explicit() {
        let token = with_runtime_scope("token-runtime-a".into(), || {
            let snapshot = mint_snapshot_handle(42, 7);
            token_for_identity(&snapshot, 3, b"button:save").unwrap()
        });
        with_runtime_scope("token-runtime-b".into(), || {
            assert_eq!(
                code(
                    resolve_element_args::<(), _>(
                        42,
                        None,
                        Some(&token),
                        None,
                        None,
                        "click",
                        |_, _| Ok(Some(()))
                    )
                    .unwrap_err()
                ),
                "generation_mismatch"
            )
        });
    }
    #[test]
    fn window_mismatch_is_refused() {
        with_runtime_scope("token-window-runtime".into(), || {
            let snapshot = mint_snapshot_handle(42, 7);
            let token = token_for_identity(&snapshot, 3, b"button:save").unwrap();
            assert_eq!(
                code(
                    resolve_element_args::<(), _>(
                        42,
                        None,
                        Some(&token),
                        None,
                        Some(8),
                        "click",
                        |_, _| Ok(Some(()))
                    )
                    .unwrap_err()
                ),
                "conflicting_element_target"
            );
        });
    }
    #[test]
    fn disappeared_current_element_is_invalid() {
        with_runtime_scope("token-disappeared-runtime".into(), || {
            let snapshot = mint_snapshot_handle(42, 7);
            let token = token_for_identity(&snapshot, 3, b"button:save").unwrap();
            assert_eq!(
                code(
                    resolve_element_args::<(), _>(
                        42,
                        None,
                        Some(&token),
                        None,
                        None,
                        "click",
                        |_, _| Ok(None)
                    )
                    .unwrap_err()
                ),
                "invalid_element_token"
            );
        });
    }
}
