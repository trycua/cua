//! Immutable daemon permission-mode configuration.
//!
//! Capability policy answers whether an operation is inside the configured
//! ceiling. Permission mode answers whether an otherwise-permitted operation
//! also needs trusted human consent. The mode is resolved once at daemon
//! startup and cannot be changed by a tool call or transport argument.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PERMISSION_MODE_ENV: &str = "CUA_DRIVER_PERMISSION_MODE";
pub const DANGEROUS_BYPASS_ENV: &str = "CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS";
pub const DISABLE_UNRESTRICTED_ENV: &str = "CUA_DRIVER_DISABLE_UNRESTRICTED";
pub const LEGACY_EXISTING_PROFILE_APPROVAL_ENV: &str =
    "CUA_DRIVER_ALLOW_LEGACY_EXISTING_PROFILE_APPROVAL";
pub const RISK_METADATA_VERSION: &str = "1";

/// Reviewed risk attached to a typed tool operation. `Unclassified` is a
/// fail-closed sentinel and is never an executable risk tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    R0,
    R1,
    R2,
    R3,
    R4,
    Unclassified,
}

impl RiskClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::R0 => "r0",
            Self::R1 => "r1",
            Self::R2 => "r2",
            Self::R3 => "r3",
            Self::R4 => "r4",
            Self::Unclassified => "unclassified",
        }
    }
}

/// Whether the resource adapter currently enforces the consent/grant
/// semantics for this exact operation. Metadata-only entries are visible to
/// clients and policy authors but intentionally retain compatibility until
/// their resource, output, egress, and revocation adapters are complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskEnforcement {
    Active,
    MetadataOnly,
}

impl RiskEnforcement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::MetadataOnly => "metadata_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskAssessment {
    pub class: RiskClass,
    pub enforcement: RiskEnforcement,
    pub operation_sensitive: bool,
}

/// Highest reviewed risk advertised for a tool. Runtime authorization uses
/// [`classify_tool_call`] so compound tools can narrow to a typed operation.
pub fn advertised_risk_for(tool: &str) -> RiskAssessment {
    let class = match tool {
        // Public driver/OS metadata with no user-content payload.
        "get_screen_size"
        | "get_cursor_position"
        | "check_permissions"
        | "get_config"
        | "get_session_state"
        | "get_agent_cursor_state"
        | "get_recording_state"
        | "check_for_update"
        | "probe" => RiskClass::R0,

        // Local reversible control and lifecycle operations.
        "click"
        | "double_click"
        | "right_click"
        | "drag"
        | "scroll"
        | "move_cursor"
        | "mouse_button_down"
        | "mouse_button_up"
        | "mouse_drag"
        | "parallel_mouse_drag"
        | "type_text"
        | "type_text_chars"
        | "press_key"
        | "hotkey"
        | "set_value"
        | "launch_app"
        | "list_apps"
        | "kill_app"
        | "list_windows"
        | "bring_to_front"
        | "debug_window_info"
        | "start_session"
        | "end_session"
        | "set_agent_cursor_enabled"
        | "set_agent_cursor_motion"
        | "set_agent_cursor_style"
        | "stop_recording"
        | "replay_trajectory" => RiskClass::R1,

        // Surfaces that can reveal or control sensitive local/authenticated
        // state. Most remain metadata-only until their resource adapters ship.
        "zoom"
        | "get_desktop_state"
        | "get_accessibility_tree"
        | "get_window_state"
        | "set_config"
        | "escalate_session"
        | "start_recording"
        | "get_browser_state"
        | "browser_prepare"
        | "browser_navigate"
        | "browser_click"
        | "browser_type"
        | "browser_pointer" => RiskClass::R2,

        // External/file side effects or generic compound action surfaces.
        "install_ffmpeg"
        | "page"
        | "browser_dialog"
        | "browser_set_input_files"
        | "browser_download" => RiskClass::R3,

        _ => RiskClass::Unclassified,
    };
    RiskAssessment {
        class,
        enforcement: RiskEnforcement::MetadataOnly,
        operation_sensitive: matches!(tool, "browser_prepare" | "browser_dialog" | "page"),
    }
}

/// Risk for the exact typed operation available at the canonical dispatch
/// boundary. No page text, selectors, typed text, paths, or other content is
/// retained by the returned value.
pub fn classify_tool_call(tool: &str, args: &Value) -> RiskAssessment {
    match tool {
        "browser_prepare" => {
            let existing =
                args.pointer("/strategy/kind").and_then(Value::as_str) == Some("existing_profile");
            if existing {
                RiskAssessment {
                    class: RiskClass::R2,
                    enforcement: RiskEnforcement::Active,
                    operation_sensitive: true,
                }
            } else {
                RiskAssessment {
                    class: RiskClass::R1,
                    enforcement: RiskEnforcement::MetadataOnly,
                    operation_sensitive: true,
                }
            }
        }
        "browser_dialog" => {
            let class = match args.get("action").and_then(Value::as_str) {
                Some("inspect") => RiskClass::R2,
                Some("accept" | "dismiss") => RiskClass::R3,
                _ => RiskClass::R3,
            };
            RiskAssessment {
                class,
                enforcement: RiskEnforcement::MetadataOnly,
                operation_sensitive: true,
            }
        }
        "page" => {
            let class = match args.get("action").and_then(Value::as_str) {
                Some("get_text" | "query_dom") => RiskClass::R2,
                _ => RiskClass::R3,
            };
            RiskAssessment {
                class,
                enforcement: RiskEnforcement::MetadataOnly,
                operation_sensitive: true,
            }
        }
        _ => advertised_risk_for(tool),
    }
}

pub fn risk_metadata_json(tool: &str) -> Value {
    let risk = advertised_risk_for(tool);
    serde_json::json!({
        "class": risk.class.as_str(),
        "enforcement": risk.enforcement.as_str(),
        "operation_sensitive": risk.operation_sensitive,
        "version": RISK_METADATA_VERSION,
    })
}

/// Canonical daemon-side authorization entry point. Capability policy is
/// evaluated first. Unknown/unreviewed tools then fail closed before registry
/// dispatch; migrated resource adapters perform their typed grant check after
/// this coordinator admits the call into the adapter.
pub fn authorize_tool_call(
    tool: &str,
    args: &Value,
) -> Result<RiskAssessment, crate::policy::AuthorizationError> {
    enforce_hard_invariants(tool, args)?;
    crate::policy::authorize_tool_call(tool, args)?;
    let risk = classify_tool_call(tool, args);
    if risk.class == RiskClass::Unclassified {
        return Err(crate::policy::AuthorizationError::Denied(format!(
            "tool '{tool}' has no reviewed risk classification"
        )));
    }
    let mode = configured_permission_mode().map_err(crate::policy::AuthorizationError::Loading)?;
    if mode == PermissionMode::Autonomous {
        let manifest = crate::session_manifest::configured_session_manifest()
            .map_err(crate::policy::AuthorizationError::Loading)?
            .ok_or_else(|| {
                crate::policy::AuthorizationError::Loading(
                    "autonomous session policy is unavailable".to_owned(),
                )
            })?;
        if manifest.is_expired() {
            return Err(crate::policy::AuthorizationError::Denied(
                "autonomous session policy expired".to_owned(),
            ));
        }
        match manifest.decision(tool) {
            crate::session_manifest::ManifestDecision::Allow => {
                manifest
                    .authorize_call(tool, args)
                    .map_err(crate::policy::AuthorizationError::Denied)?;
                manifest
                    .authorize_dispatch()
                    .map_err(crate::policy::AuthorizationError::Denied)?;
            }
            crate::session_manifest::ManifestDecision::Deny => {
                return Err(crate::policy::AuthorizationError::Denied(format!(
                    "autonomous session policy denies tool '{tool}'"
                )))
            }
            crate::session_manifest::ManifestDecision::Ask => {
                return Err(crate::policy::AuthorizationError::Denied(format!(
                    "autonomous session policy requires protected approval for tool '{tool}'; unattended dispatch cannot auto-accept"
                )))
            }
            crate::session_manifest::ManifestDecision::Undeclared => {
                return Err(crate::policy::AuthorizationError::Denied(format!(
                    "tool '{tool}' is outside the autonomous session policy"
                )))
            }
        }
    }
    Ok(risk)
}

fn enforce_hard_invariants(
    tool: &str,
    args: &Value,
) -> Result<(), crate::policy::AuthorizationError> {
    // Provider and indicator adapters hosted by the daemon must never become
    // an ordinary target. Coordinate-only desktop input cannot prove a target
    // PID and therefore is not a protected-UI route; certified providers must
    // render outside that surface and independently reject synthetic input.
    let process_targeting_tool = matches!(
        tool,
        "click"
            | "double_click"
            | "right_click"
            | "drag"
            | "scroll"
            | "type_text"
            | "type_text_chars"
            | "press_key"
            | "hotkey"
            | "set_value"
            | "kill_app"
            | "bring_to_front"
            | "get_accessibility_tree"
            | "get_window_state"
            | "page"
            | "browser_prepare"
    );
    if process_targeting_tool
        && args.get("pid").and_then(Value::as_i64) == Some(i64::from(std::process::id()))
    {
        return Err(crate::policy::AuthorizationError::Denied(
            "Cua Driver refuses operations that target its own authorization process".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Standard,
    Autonomous,
    Unrestricted,
}

impl PermissionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Autonomous => "autonomous",
            Self::Unrestricted => "unrestricted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PermissionModeError {
    #[error("unknown permission mode '{0}'; expected standard, autonomous, or unrestricted")]
    Unknown(String),
    #[error(
        "permission mode unrestricted requires --dangerously-bypass-approvals at trusted daemon startup"
    )]
    MissingDangerAcknowledgement,
    #[error("--dangerously-bypass-approvals is valid only with --permission-mode unrestricted")]
    UnexpectedDangerAcknowledgement,
    #[error(
        "permission mode autonomous requires --session-policy <path> at trusted daemon startup"
    )]
    MissingSessionPolicy,
    #[error(
        "permission mode autonomous requires --approve-session-policy at trusted daemon startup"
    )]
    MissingSessionPolicyApproval,
    #[error("--session-policy/--approve-session-policy are valid only with --permission-mode autonomous")]
    UnexpectedSessionPolicy,
    #[error("permission mode unrestricted is disabled by managed startup configuration")]
    UnrestrictedDisabled,
    #[error(
        "--allow-legacy-existing-profile-approval is valid only with --permission-mode standard"
    )]
    UnexpectedLegacyApproval,
}

fn parse_permission_mode(
    configured: Option<&str>,
    dangerous_bypass: bool,
) -> Result<PermissionMode, PermissionModeError> {
    let mode = match configured.unwrap_or("standard") {
        "standard" => PermissionMode::Standard,
        "autonomous" => PermissionMode::Autonomous,
        "unrestricted" | "yolo" => PermissionMode::Unrestricted,
        other => return Err(PermissionModeError::Unknown(other.to_owned())),
    };
    match (mode, dangerous_bypass) {
        (PermissionMode::Unrestricted, false) => {
            Err(PermissionModeError::MissingDangerAcknowledgement)
        }
        (PermissionMode::Standard | PermissionMode::Autonomous, true) => {
            Err(PermissionModeError::UnexpectedDangerAcknowledgement)
        }
        _ => Ok(mode),
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Temporary migration escape hatch for the same-user-writable browser
/// approval artifact. It is deliberately outside the public tool protocol and
/// must never be described as protected human consent.
pub fn legacy_existing_profile_approval_enabled() -> bool {
    // Core's browser unit fixtures exercise the legacy artifact's binding,
    // setup, reconnect, and cleanup mechanics directly. Integration tests
    // compile this crate without `cfg(test)` and own the production-default
    // refusal contract.
    #[cfg(test)]
    return true;

    #[cfg(not(test))]
    env_flag(LEGACY_EXISTING_PROFILE_APPROVAL_ENV)
}

static CONFIGURED_PERMISSION_MODE: OnceLock<Result<PermissionMode, String>> = OnceLock::new();

/// Return the immutable process permission mode. An unset mode is `standard`.
pub fn configured_permission_mode() -> Result<PermissionMode, String> {
    CONFIGURED_PERMISSION_MODE
        .get_or_init(|| {
            let configured = std::env::var(PERMISSION_MODE_ENV).ok();
            parse_permission_mode(configured.as_deref(), env_flag(DANGEROUS_BYPASS_ENV))
                .map_err(|error| error.to_string())
        })
        .clone()
}

/// Validate every immutable authorization input before an action endpoint is
/// bound. An unset user policy remains compatible; an explicitly configured
/// invalid policy or mode is fatal.
pub fn validate_startup_authorization() -> anyhow::Result<()> {
    crate::policy::validate_configured_policy()?;
    let mode = configured_permission_mode().map_err(anyhow::Error::msg)?;
    if mode == PermissionMode::Unrestricted && env_flag(DISABLE_UNRESTRICTED_ENV) {
        return Err(PermissionModeError::UnrestrictedDisabled.into());
    }
    if mode != PermissionMode::Standard && env_flag(LEGACY_EXISTING_PROFILE_APPROVAL_ENV) {
        return Err(PermissionModeError::UnexpectedLegacyApproval.into());
    }
    let session_policy_configured =
        std::env::var_os(crate::session_manifest::SESSION_POLICY_FILE_ENV).is_some();
    let session_policy_approved = env_flag(crate::session_manifest::SESSION_POLICY_APPROVED_ENV);
    match mode {
        PermissionMode::Autonomous => {
            if !session_policy_configured {
                return Err(PermissionModeError::MissingSessionPolicy.into());
            }
            if !session_policy_approved {
                return Err(PermissionModeError::MissingSessionPolicyApproval.into());
            }
            let manifest = crate::session_manifest::configured_session_manifest()
                .map_err(anyhow::Error::msg)?
                .ok_or(PermissionModeError::MissingSessionPolicy)?;
            if manifest.is_expired() {
                anyhow::bail!("autonomous session policy is already expired");
            }
        }
        PermissionMode::Standard | PermissionMode::Unrestricted => {
            if session_policy_configured || session_policy_approved {
                return Err(PermissionModeError::UnexpectedSessionPolicy.into());
            }
        }
    }
    Ok(())
}

/// Content-free authorization state suitable for status/health output.
pub fn status_json() -> serde_json::Value {
    let mode = configured_permission_mode();
    let policy = crate::policy::configured_policy();
    let managed_policy = crate::policy::configured_managed_policy();
    let user_policy_sha256 = crate::policy::user_policy_sha256().ok().flatten();
    let managed_policy_sha256 = crate::policy::managed_policy_sha256().ok().flatten();
    let session_policy = crate::session_manifest::configured_session_manifest();
    let session_policy_status = session_policy
        .as_ref()
        .ok()
        .and_then(|manifest| *manifest)
        .map(|manifest| {
            let (allow, deny, ask) = manifest.counts();
            serde_json::json!({
                "sha256": manifest.sha256(),
                "expires_unix_ms": manifest.expires_unix_ms(),
                "idle_timeout_seconds": manifest.idle_timeout().as_secs(),
                "allow_count": allow,
                "deny_count": deny,
                "ask_count": ask,
            })
        });
    serde_json::json!({
        "permission_mode": mode.as_ref().map(|mode| mode.as_str()).ok(),
        "permission_mode_valid": mode.is_ok(),
        "permission_mode_source": if std::env::var_os(PERMISSION_MODE_ENV).is_some() {
            "trusted_startup_configuration"
        } else {
            "built_in_default"
        },
        "unrestricted_disabled_by_admin": env_flag(DISABLE_UNRESTRICTED_ENV),
        "user_policy_configured": std::env::var_os(crate::policy::POLICY_FILE_ENV).is_some(),
        "user_policy_active": matches!(policy, Ok(Some(_))),
        "user_policy_valid": policy.is_ok(),
        "user_policy_sha256": user_policy_sha256,
        "managed_policy_configured": std::env::var_os(crate::policy::MANAGED_POLICY_FILE_ENV).is_some(),
        "managed_policy_active": matches!(managed_policy, Ok(Some(_))),
        "managed_policy_valid": managed_policy.is_ok(),
        "managed_policy_sha256": managed_policy_sha256,
        "built_in_ceiling": "reviewed_tool_and_risk_map_v1",
        "legacy_existing_profile_approval": legacy_existing_profile_approval_enabled(),
        "risk_metadata_version": RISK_METADATA_VERSION,
        "active_risk_enforcement": ["browser_prepare.existing_profile"],
        "protected_consent_collector": crate::consent::configured_provider_id(),
        "session_policy_configured": std::env::var_os(crate::session_manifest::SESSION_POLICY_FILE_ENV).is_some(),
        "session_policy_approved_at_startup": env_flag(crate::session_manifest::SESSION_POLICY_APPROVED_ENV),
        "session_policy_valid": session_policy.is_ok(),
        "session_policy": session_policy_status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_is_the_default() {
        assert_eq!(
            parse_permission_mode(None, false).unwrap(),
            PermissionMode::Standard
        );
    }

    #[test]
    fn autonomous_needs_no_bypass_flag() {
        assert_eq!(
            parse_permission_mode(Some("autonomous"), false).unwrap(),
            PermissionMode::Autonomous
        );
    }

    #[test]
    fn unrestricted_requires_deliberate_acknowledgement() {
        assert_eq!(
            parse_permission_mode(Some("unrestricted"), false),
            Err(PermissionModeError::MissingDangerAcknowledgement)
        );
        assert_eq!(
            parse_permission_mode(Some("unrestricted"), true).unwrap(),
            PermissionMode::Unrestricted
        );
    }

    #[test]
    fn danger_flag_cannot_weaken_another_mode() {
        assert_eq!(
            parse_permission_mode(Some("standard"), true),
            Err(PermissionModeError::UnexpectedDangerAcknowledgement)
        );
    }

    #[test]
    fn yolo_is_only_an_alias_for_explicit_unrestricted() {
        assert_eq!(
            parse_permission_mode(Some("yolo"), true).unwrap(),
            PermissionMode::Unrestricted
        );
    }

    #[test]
    fn existing_profile_is_the_first_actively_enforced_risk_operation() {
        let existing = classify_tool_call(
            "browser_prepare",
            &serde_json::json!({"strategy": {"kind": "existing_profile"}}),
        );
        assert_eq!(existing.class, RiskClass::R2);
        assert_eq!(existing.enforcement, RiskEnforcement::Active);

        let isolated = classify_tool_call(
            "browser_prepare",
            &serde_json::json!({"profile": {"mode": "isolated_new"}}),
        );
        assert_eq!(isolated.class, RiskClass::R1);
        assert_eq!(isolated.enforcement, RiskEnforcement::MetadataOnly);
    }

    #[test]
    fn unknown_tools_are_unclassified_and_denied() {
        let error = authorize_tool_call("new_unreviewed_tool", &serde_json::json!({}))
            .expect_err("unknown risk must fail closed");
        assert!(error
            .to_string()
            .contains("no reviewed risk classification"));
    }

    #[test]
    fn process_targeted_tools_cannot_target_the_authorization_daemon() {
        let error = authorize_tool_call(
            "click",
            &serde_json::json!({"pid": std::process::id(), "x": 1, "y": 1}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("authorization process"));
    }
}
