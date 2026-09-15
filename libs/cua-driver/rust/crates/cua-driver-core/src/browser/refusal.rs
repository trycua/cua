//! Structured refusals for the browser-tool surface.
//!
//! The exact-or-refused contract means most failure paths are not
//! protocol errors — they are deliberate, machine-readable refusals the
//! calling agent is expected to branch on. A refusal serializes into
//! `structuredContent` as `{"status":"refused","refusal":{code,...}}`
//! with a stable snake_case `code` from the closed set below.

use serde::Serialize;
use serde_json::Value;

use crate::protocol::ToolResult;

/// Closed vocabulary of browser refusal codes. Additive-only: renaming
/// or removing a code is a breaking change for consumers that branch on
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRefusalCode {
    /// No CDP route exists for this process (not a browser, or an
    /// engine family without CDP support in v1).
    BrowserRouteUnavailable,
    /// The browser supports CDP but no owned endpoint is available —
    /// run `browser_prepare` explicitly.
    BrowserRequiresSetup,
    /// More than one CDP candidate matched the native window and the
    /// title tie-break could not pick a unique one.
    BrowserBindingAmbiguous,
    /// A previously minted binding no longer identifies the same
    /// process/endpoint (fingerprint drift, endpoint change, unknown or
    /// foreign-session target id).
    BrowserBindingStale,
    /// Revalidation could not prove the CDP target still corresponds to
    /// the bound native window, or mutation was attempted on a
    /// non-exact binding.
    BrowserWrongTargetRefused,
    /// The operation needs an explicit `tab_id` and none was given.
    BrowserTabRequired,
    /// The given `tab_id` is unknown in this session or its CDP target
    /// is gone.
    BrowserTabNotFound,
    /// The given page ref does not resolve in the tab's live snapshot
    /// namespace (malformed, foreign namespace, or invalidated by
    /// navigation).
    BrowserRefStale,
    /// The trusted Input-domain route is unavailable and the caller did
    /// not explicitly request the `dom_event` route.
    BrowserInputTrustUnavailable,
    /// The discovered endpoint's ownership proof does not attribute it
    /// to the target process.
    BrowserEndpointOwnerMismatch,
    /// The platform adapter requires explicit user/caller consent
    /// before it will prepare an endpoint.
    BrowserConsentRequired,
    /// A person explicitly dismissed or denied the browser-owned consent UI.
    BrowserConsentRevoked,
    /// The fixed reconnect attempt/deadline policy was exhausted.
    BrowserReconnectExhausted,
    /// A typing operation delivered only a proven prefix of the request.
    BrowserInputIncomplete,
    /// A semantic ref is live, but it does not declare the requested typed
    /// browser action.
    BrowserActionUnavailable,
    /// The live top-level document left the origin set approved in the
    /// capability manifest. Further browser input is paused.
    BrowserOriginOutsideScope,
}

impl BrowserRefusalCode {
    /// The stable wire string for this code.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BrowserRouteUnavailable => "browser_route_unavailable",
            Self::BrowserRequiresSetup => "browser_requires_setup",
            Self::BrowserBindingAmbiguous => "browser_binding_ambiguous",
            Self::BrowserBindingStale => "browser_binding_stale",
            Self::BrowserWrongTargetRefused => "browser_wrong_target_refused",
            Self::BrowserTabRequired => "browser_tab_required",
            Self::BrowserTabNotFound => "browser_tab_not_found",
            Self::BrowserRefStale => "browser_ref_stale",
            Self::BrowserInputTrustUnavailable => "browser_input_trust_unavailable",
            Self::BrowserEndpointOwnerMismatch => "browser_endpoint_owner_mismatch",
            Self::BrowserConsentRequired => "browser_consent_required",
            Self::BrowserConsentRevoked => "browser_consent_revoked",
            Self::BrowserReconnectExhausted => "browser_reconnect_exhausted",
            Self::BrowserInputIncomplete => "browser_input_incomplete",
            Self::BrowserActionUnavailable => "browser_action_unavailable",
            Self::BrowserOriginOutsideScope => "browser_origin_outside_scope",
        }
    }
}

/// A structured refusal: stable code + human-readable message +
/// optional machine detail (candidate lists, hints).
#[derive(Debug, Clone, Serialize)]
pub struct BrowserRefusal {
    pub code: BrowserRefusalCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl BrowserRefusal {
    pub fn new(code: BrowserRefusalCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

/// Raw OS error number for "too many open files in this process"
/// (EMFILE). Shared by Linux and macOS.
pub const EMFILE_NUMBER: i32 = 24;
/// Raw OS error number for "too many open files system-wide" (ENFILE).
/// Shared by Linux and macOS.
pub const ENFILE_NUMBER: i32 = 23;

/// Recovery hint attached to descriptor-exhaustion inspection refusals.
pub const DESCRIPTOR_EXHAUSTION_HINT: &str = "restart the driver service and raise the file-descriptor limit (e.g. `ulimit -n`) before retrying";

/// True when an I/O error reports file-descriptor exhaustion, either via
/// its raw OS error number (EMFILE/ENFILE) or via its message text.
pub fn is_descriptor_exhaustion(error: &std::io::Error) -> bool {
    match error.raw_os_error() {
        Some(code) => code == EMFILE_NUMBER || code == ENFILE_NUMBER,
        None => error
            .to_string()
            .to_ascii_lowercase()
            .contains("too many open files"),
    }
}

/// Best-effort snapshot of the current process's file-descriptor limit as
/// `(soft, hard)`. Returns `None` when the limit cannot be read; callers
/// must treat a missing snapshot as unknown rather than as healthy.
#[cfg(unix)]
pub fn descriptor_limit_snapshot() -> Option<(u64, u64)> {
    let mut limits = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limits.as_mut_ptr()) } != 0 {
        return None;
    }
    let limits = unsafe { limits.assume_init() };
    Some((limits.rlim_cur as u64, limits.rlim_max as u64))
}

/// No descriptor-limit introspection outside Unix; unknown, not healthy.
#[cfg(not(unix))]
pub fn descriptor_limit_snapshot() -> Option<(u64, u64)> {
    None
}

/// Build a fail-closed refusal for a browser-inspection subprocess that
/// could not be started. Ordinary spawn failures keep the historical
/// `"{base}: {error}"` message with no detail. Descriptor exhaustion
/// (EMFILE/ENFILE) keeps the same `BrowserRouteUnavailable` code but names
/// the exhausted resource, the command that could not start, the inspected
/// pid when known, the current descriptor limit when readable, and an
/// actionable recovery hint.
pub fn inspection_spawn_refusal(
    base_message: impl Into<String>,
    command: &str,
    target_pid: Option<i64>,
    error: &std::io::Error,
) -> BrowserRefusal {
    let base = base_message.into();
    if !is_descriptor_exhaustion(error) {
        return BrowserRefusal::new(
            BrowserRefusalCode::BrowserRouteUnavailable,
            format!("{base}: {error}"),
        );
    }
    let message = format!(
        "{base}: {error}; file-descriptor table exhausted while starting `{command}`; {DESCRIPTOR_EXHAUSTION_HINT}"
    );
    let mut detail = serde_json::Map::new();
    detail.insert(
        "resource".to_owned(),
        serde_json::Value::String("file_descriptors".to_owned()),
    );
    detail.insert(
        "command".to_owned(),
        serde_json::Value::String(command.to_owned()),
    );
    if let Some(pid) = target_pid {
        detail.insert(
            "target_pid".to_owned(),
            serde_json::Value::Number(pid.into()),
        );
    }
    if let Some(code) = error.raw_os_error() {
        detail.insert(
            "os_error".to_owned(),
            serde_json::Value::Number(code.into()),
        );
    }
    if let Some((soft, hard)) = descriptor_limit_snapshot() {
        detail.insert(
            "descriptor_limit_soft".to_owned(),
            serde_json::Value::Number(soft.into()),
        );
        detail.insert(
            "descriptor_limit_hard".to_owned(),
            serde_json::Value::Number(hard.into()),
        );
    }
    detail.insert(
        "hint".to_owned(),
        serde_json::Value::String(DESCRIPTOR_EXHAUSTION_HINT.to_owned()),
    );
    BrowserRefusal::new(BrowserRefusalCode::BrowserRouteUnavailable, message)
        .with_detail(Value::Object(detail))
}

impl BrowserRefusal {
    /// Render as a tool result. NOT an MCP protocol error: the call
    /// executed correctly and its outcome is the refusal, so `isError`
    /// stays unset and agents branch on `structuredContent.status`.
    pub fn to_tool_result(&self) -> ToolResult {
        let text = format!("refused ({}): {}", self.code.as_str(), self.message);
        ToolResult::text(text).with_structured(serde_json::json!({
            "status": "refused",
            "refusal": self,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_serializes_to_the_documented_snake_case_string() {
        let all = [
            (
                BrowserRefusalCode::BrowserRouteUnavailable,
                "browser_route_unavailable",
            ),
            (
                BrowserRefusalCode::BrowserRequiresSetup,
                "browser_requires_setup",
            ),
            (
                BrowserRefusalCode::BrowserBindingAmbiguous,
                "browser_binding_ambiguous",
            ),
            (
                BrowserRefusalCode::BrowserBindingStale,
                "browser_binding_stale",
            ),
            (
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "browser_wrong_target_refused",
            ),
            (
                BrowserRefusalCode::BrowserTabRequired,
                "browser_tab_required",
            ),
            (
                BrowserRefusalCode::BrowserTabNotFound,
                "browser_tab_not_found",
            ),
            (BrowserRefusalCode::BrowserRefStale, "browser_ref_stale"),
            (
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                "browser_input_trust_unavailable",
            ),
            (
                BrowserRefusalCode::BrowserEndpointOwnerMismatch,
                "browser_endpoint_owner_mismatch",
            ),
            (
                BrowserRefusalCode::BrowserConsentRequired,
                "browser_consent_required",
            ),
            (
                BrowserRefusalCode::BrowserConsentRevoked,
                "browser_consent_revoked",
            ),
            (
                BrowserRefusalCode::BrowserReconnectExhausted,
                "browser_reconnect_exhausted",
            ),
            (
                BrowserRefusalCode::BrowserInputIncomplete,
                "browser_input_incomplete",
            ),
            (
                BrowserRefusalCode::BrowserActionUnavailable,
                "browser_action_unavailable",
            ),
            (
                BrowserRefusalCode::BrowserOriginOutsideScope,
                "browser_origin_outside_scope",
            ),
        ];
        for (code, wire) in all {
            assert_eq!(code.as_str(), wire);
            // serde and as_str must agree — consumers see the serde form.
            assert_eq!(serde_json::to_value(code).unwrap(), serde_json::json!(wire));
        }
    }

    #[test]
    fn refusal_tool_result_shape_is_stable() {
        let refusal = BrowserRefusal::new(
            BrowserRefusalCode::BrowserRequiresSetup,
            "no owned DevTools endpoint",
        )
        .with_detail(serde_json::json!({"hint": "run browser_prepare"}));
        let result = refusal.to_tool_result();

        // Refusals are outcomes, not protocol errors.
        assert!(result.is_error.is_none());

        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(structured["status"], "refused");
        assert_eq!(structured["refusal"]["code"], "browser_requires_setup");
        assert_eq!(
            structured["refusal"]["message"],
            "no owned DevTools endpoint"
        );
        assert_eq!(
            structured["refusal"]["detail"]["hint"],
            "run browser_prepare"
        );

        // The human-readable line carries the code for text-only clients.
        let text = match &result.content[0] {
            crate::protocol::Content::Text { text, .. } => text.clone(),
            other => panic!("expected text content, got {other:?}"),
        };
        assert!(text.contains("browser_requires_setup"), "{text}");
    }

    #[test]
    fn detail_is_omitted_when_absent() {
        let refusal = BrowserRefusal::new(BrowserRefusalCode::BrowserRefStale, "ref p1:2 not live");
        let v = serde_json::to_value(&refusal).unwrap();
        assert!(
            v.get("detail").is_none(),
            "absent detail must not serialize: {v}"
        );
    }

    #[test]
    fn descriptor_exhaustion_matches_emfile_and_enfile() {
        let emfile = std::io::Error::from_raw_os_error(EMFILE_NUMBER);
        let enfile = std::io::Error::from_raw_os_error(ENFILE_NUMBER);
        assert!(is_descriptor_exhaustion(&emfile));
        assert!(is_descriptor_exhaustion(&enfile));
        assert!(!is_descriptor_exhaustion(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        assert!(!is_descriptor_exhaustion(
            &std::io::Error::from_raw_os_error(2)
        ));
    }

    #[test]
    fn inspection_spawn_refusal_keeps_plain_mapping_for_ordinary_errors() {
        let error = std::io::Error::from_raw_os_error(2);
        let refusal =
            inspection_spawn_refusal("could not inspect browser listeners", "lsof", None, &error);
        assert_eq!(refusal.code, BrowserRefusalCode::BrowserRouteUnavailable);
        assert_eq!(
            refusal.message,
            format!("could not inspect browser listeners: {error}")
        );
        assert!(
            refusal.detail.is_none(),
            "ordinary spawn errors carry no resource detail"
        );
    }

    #[test]
    fn inspection_spawn_refusal_names_resource_and_recovery_for_emfile() {
        let error = std::io::Error::from_raw_os_error(EMFILE_NUMBER);
        let refusal = inspection_spawn_refusal(
            "could not inspect browser process 4242",
            "ps",
            Some(4242),
            &error,
        );
        // Fail-closed: the code is unchanged, so agents keep branching on it.
        assert_eq!(refusal.code, BrowserRefusalCode::BrowserRouteUnavailable);
        assert!(
            refusal
                .message
                .contains("could not inspect browser process 4242"),
            "historical prefix must survive: {}",
            refusal.message
        );
        assert!(
            refusal.message.contains("`ps`") && refusal.message.contains("ulimit -n"),
            "message must name the command and the recovery: {}",
            refusal.message
        );
        let detail = refusal.detail.expect("exhaustion must carry detail");
        assert_eq!(detail["resource"], "file_descriptors");
        assert_eq!(detail["command"], "ps");
        assert_eq!(detail["target_pid"], 4242);
        assert_eq!(detail["os_error"], EMFILE_NUMBER);
        assert_eq!(detail["hint"], DESCRIPTOR_EXHAUSTION_HINT);
    }

    #[test]
    fn descriptor_limit_snapshot_never_panics() {
        let snapshot = descriptor_limit_snapshot();
        if let Some((soft, hard)) = snapshot {
            assert!(soft > 0 && hard >= soft);
        }
    }
}
