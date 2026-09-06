//! Immutable daemon permission-mode configuration.
//!
//! Capability policy answers whether an operation is inside the configured
//! ceiling. Permission mode answers whether an otherwise-permitted operation
//! also needs trusted human consent. The mode is resolved once at daemon
//! startup and cannot be changed by a tool call or transport argument.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PERMISSION_MODE_ENV: &str = "CUA_DRIVER_PERMISSION_MODE";
pub const DANGEROUS_BYPASS_ENV: &str = "CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS";
pub const DISABLE_UNRESTRICTED_ENV: &str = "CUA_DRIVER_DISABLE_UNRESTRICTED";
pub const RISK_METADATA_VERSION: &str = "1";

static LAUNCH_GRANTS: OnceLock<BTreeSet<String>> = OnceLock::new();

/// Install immutable launcher grants before the action runtime is created.
///
/// These grants are process-local trusted configuration. They are never read
/// from environment variables or accepted from tool arguments.
pub fn configure_launch_grants(grants: &[String]) -> Result<(), String> {
    let mut normalized = BTreeSet::new();
    for grant in grants {
        match grant.trim() {
            "existing-profile" | "existing_profile" => {
                normalized.insert("existing_profile".to_owned());
            }
            "" => return Err("launch grant cannot be empty".to_owned()),
            other => return Err(format!("unknown launch grant '{other}'")),
        }
    }
    LAUNCH_GRANTS
        .set(normalized)
        .map_err(|_| "launch grants were already configured".to_owned())
}

pub fn launch_grant_enabled(grant: &str) -> bool {
    LAUNCH_GRANTS
        .get()
        .is_some_and(|grants| grants.contains(grant))
}

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
    NotExposed,
}

impl RiskEnforcement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::MetadataOnly => "metadata_only",
            Self::NotExposed => "not_exposed",
        }
    }
}

/// Stable, content-free description of one permission-mode enforcement
/// adapter. This inventory is the source for status and tool discovery; it
/// deliberately distinguishes shipped enforcement from reviewed metadata and
/// capabilities that are not exposed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EnforcementAdapterDescriptor {
    pub id: &'static str,
    pub operations: &'static [&'static str],
    pub state: RiskEnforcement,
    pub risk_class: RiskClass,
    pub resource_kind: &'static str,
    pub scope_keys: &'static [&'static str],
    pub grant_type: Option<&'static str>,
    pub idle_ttl_seconds: Option<u64>,
    pub absolute_ttl_seconds: Option<u64>,
    pub authorization_requirement: &'static str,
    pub revocation_triggers: &'static [&'static str],
    pub refusal_code: Option<&'static str>,
    pub authorization_source: &'static str,
    /// Mode-specific truth. `state` remains the standard-mode value for
    /// backward-compatible inventory consumers.
    pub enforcement_by_mode: AdapterEnforcement,
    /// Factored profile behavior selected after hard invariants, configured
    /// policy, and optional capability-manifest scope admission.
    #[serde(rename = "profile_behavior_class")]
    pub profile_behavior: AdapterProfileBehavior,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AdapterEnforcement {
    pub standard: RiskEnforcement,
    pub bounded: RiskEnforcement,
    pub unrestricted: RiskEnforcement,
}

impl AdapterEnforcement {
    pub const fn uniform(state: RiskEnforcement) -> Self {
        Self {
            standard: state,
            bounded: state,
            unrestricted: state,
        }
    }

    pub const fn for_mode(self, mode: PermissionMode) -> RiskEnforcement {
        match mode {
            PermissionMode::Standard => self.standard,
            PermissionMode::Bounded => self.bounded,
            PermissionMode::Unrestricted => self.unrestricted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeBehavior {
    Allow,
    Deny,
    RequireGrant,
    /// Bounded-profile compatibility spelling on the public inventory. The
    /// factored profile class explains that the manifest is a separate scope
    /// ceiling rather than the approval behavior itself.
    #[serde(rename = "manifest")]
    AllowWithoutGrant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterProfileBehavior {
    Routine,
    GrantInStandard,
    BoundedOrUnrestricted,
    UnrestrictedOnly,
    Denied,
}

impl AdapterProfileBehavior {
    pub const fn for_mode(self, mode: PermissionMode) -> ModeBehavior {
        match (self, mode) {
            (Self::Routine, PermissionMode::Standard | PermissionMode::Unrestricted) => {
                ModeBehavior::Allow
            }
            (Self::Routine, PermissionMode::Bounded) => ModeBehavior::AllowWithoutGrant,
            (Self::GrantInStandard, PermissionMode::Standard) => ModeBehavior::RequireGrant,
            (Self::GrantInStandard, PermissionMode::Bounded) => ModeBehavior::AllowWithoutGrant,
            (Self::GrantInStandard, PermissionMode::Unrestricted) => ModeBehavior::Allow,
            (Self::BoundedOrUnrestricted, PermissionMode::Standard) => ModeBehavior::Deny,
            (Self::BoundedOrUnrestricted, PermissionMode::Bounded) => {
                ModeBehavior::AllowWithoutGrant
            }
            (Self::BoundedOrUnrestricted, PermissionMode::Unrestricted) => ModeBehavior::Allow,
            (Self::UnrestrictedOnly, PermissionMode::Unrestricted) => ModeBehavior::Allow,
            (Self::UnrestrictedOnly, PermissionMode::Standard | PermissionMode::Bounded)
            | (Self::Denied, _) => ModeBehavior::Deny,
        }
    }
}

const EXISTING_PROFILE_OPERATIONS: &[&str] = &["browser_prepare[strategy.kind=existing_profile]"];
const ISOLATED_BROWSER_OPERATIONS: &[&str] = &["browser_prepare[profile.mode=isolated]"];
const ISOLATED_BROWSER_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "transport_session",
    "pid",
    "profile_mode",
    "profile_name",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];
const EXISTING_PROFILE_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "transport_session",
    "pid",
    "window_id",
    "process_fingerprint",
    "browser_product",
    "endpoint_owner",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];
const EXISTING_PROFILE_REVOCATION: &[&str] = &[
    "session_end",
    "idle_expiry",
    "absolute_expiry",
    "policy_change",
    "process_identity_change",
    "endpoint_identity_change",
    "daemon_restart",
    "reconnect_budget_exhaustion",
];

const PRIVATE_OBSERVATION_OPERATIONS: &[&str] = &[
    "get_desktop_state",
    "get_accessibility_tree",
    "get_window_state",
    "verify_state",
    "list_apps",
    "list_windows",
    "debug_window_info",
    "get_browser_state",
    "browser_dialog[action=inspect]",
    "page[action=get_text|query_dom]",
    "escalate_session",
    "zoom",
    "start_recording",
];
const PRIVATE_OBSERVATION_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "pid",
    "process_fingerprint",
    "window_id",
    "display_generation",
    "capture_scope",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const COMPUTER_HISTORY_OPERATIONS: &[&str] = &["history_status", "history_query"];
const COMPUTER_HISTORY_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "operation",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const DESKTOP_INPUT_OPERATIONS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "drag",
    "scroll",
    "move_cursor",
    "mouse_button_down",
    "mouse_button_up",
    "mouse_drag",
    "parallel_mouse_drag",
    "replay_trajectory",
    "type_text",
    "type_text_chars",
    "press_key",
    "hotkey",
    "set_value",
    "bring_to_front",
    "set_window_frame",
];
const DESKTOP_INPUT_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "pid",
    "process_fingerprint",
    "window_id",
    "display_generation",
    "delivery_mode_ceiling",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const FILE_TRANSFER_OPERATIONS: &[&str] = &[
    "browser_set_input_files",
    "browser_download",
    "get_desktop_state[with_file_output]",
    "get_window_state[with_file_output]",
    "start_recording",
    "stop_recording",
    "replay_trajectory",
    "install_ffmpeg",
];
const FILE_TRANSFER_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "browser_binding",
    "tab",
    "canonical_path",
    "destination_class",
    "direction",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const CONSEQUENTIAL_OPERATIONS: &[&str] = &["browser_dialog[action=accept|dismiss]"];
const CONSEQUENTIAL_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "browser_binding",
    "tab",
    "origin",
    "action_kind",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const UNBOUNDED_SCRIPT_OPERATIONS: &[&str] = &[
    "page[action=execute_javascript|click_element|insert_text|type_keystrokes|enable_javascript_apple_events]",
];

const LEGACY_PAGE_MUTATING_ACTIONS: &[&str] = &[
    "execute_javascript",
    "click_element",
    "insert_text",
    "type_keystrokes",
    "enable_javascript_apple_events",
];

const BROWSER_BOUND_INPUT_OPERATIONS: &[&str] = &[
    "browser_navigate",
    "browser_click",
    "browser_type",
    "browser_pointer",
];
const BROWSER_BOUND_INPUT_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "browser_binding",
    "tab",
    "origin",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const PROCESS_CONTROL_OPERATIONS: &[&str] = &["kill_app"];
const PROCESS_CONTROL_SCOPE_KEYS: &[&str] = &[
    "daemon_generation",
    "public_session",
    "pid",
    "process_fingerprint",
    "permission_mode",
    "managed_policy_sha256",
    "user_policy_sha256",
];

const OS_PERMISSION_PROMPT_OPERATIONS: &[&str] = &["check_permissions[prompt=true]"];
const DRIVER_CONFIGURATION_OPERATIONS: &[&str] = &["set_config"];

const SESSION_REVOCATION: &[&str] = &[
    "session_end",
    "expiry",
    "policy_change",
    "resource_identity_change",
    "daemon_restart",
];

pub const ENFORCEMENT_ADAPTERS: &[EnforcementAdapterDescriptor] = &[
    EnforcementAdapterDescriptor {
        id: "browser_prepare.isolated",
        operations: ISOLATED_BROWSER_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R1,
        resource_kind: "driver_owned_browser_profile",
        scope_keys: ISOLATED_BROWSER_SCOPE_KEYS,
        grant_type: None,
        idle_ttl_seconds: None,
        absolute_ttl_seconds: None,
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "browser_prepare.existing_profile",
        operations: EXISTING_PROFILE_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "existing_browser_profile",
        scope_keys: EXISTING_PROFILE_SCOPE_KEYS,
        grant_type: Some("existing_profile_session_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "profile_approval_behavior_and_optional_capability_manifest",
        revocation_triggers: EXISTING_PROFILE_REVOCATION,
        refusal_code: Some("browser_consent_required"),
        authorization_source:
            "launch_grant_or_authorization_host_in_standard; unattended_in_bounded; trusted_launch_acknowledgement_in_unrestricted; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::GrantInStandard,
    },
    EnforcementAdapterDescriptor {
        id: "private_observation",
        operations: PRIVATE_OBSERVATION_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "user_window_or_display_observation",
        scope_keys: PRIVATE_OBSERVATION_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "computer_history",
        operations: COMPUTER_HISTORY_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "computer_history_metadata",
        scope_keys: COMPUTER_HISTORY_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "explicit_standard_grant_or_approved_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "authorization_host_in_standard; approved_capability_manifest_in_bounded; trusted_unrestricted_mode",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::GrantInStandard,
    },
    EnforcementAdapterDescriptor {
        id: "desktop_input",
        operations: DESKTOP_INPUT_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R1,
        resource_kind: "user_window_or_display_input",
        scope_keys: DESKTOP_INPUT_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "file_transfer_and_output",
        operations: FILE_TRANSFER_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R3,
        resource_kind: "canonical_file_path_and_destination",
        scope_keys: FILE_TRANSFER_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "browser_consequential_action",
        operations: CONSEQUENTIAL_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R3,
        resource_kind: "typed_browser_consequential_action",
        scope_keys: CONSEQUENTIAL_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(5 * 60),
        absolute_ttl_seconds: Some(30 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "browser_unbounded_script",
        operations: UNBOUNDED_SCRIPT_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R3,
        resource_kind: "unbounded_authenticated_page_script",
        scope_keys: &[],
        grant_type: Some("trusted_launch_mode_gate"),
        idle_ttl_seconds: None,
        absolute_ttl_seconds: None,
        authorization_requirement: "not_grantable_in_standard_or_bounded",
        revocation_triggers: &[],
        refusal_code: Some("unbounded_operation_requires_unrestricted"),
        authorization_source: "unrestricted_mode_with_trusted_launch_time_risk_acknowledgement",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::UnrestrictedOnly,
    },
    EnforcementAdapterDescriptor {
        id: "browser_bound_input",
        operations: BROWSER_BOUND_INPUT_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "authenticated_browser_binding_and_tab_input",
        scope_keys: BROWSER_BOUND_INPUT_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "process_control",
        operations: PROCESS_CONTROL_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R3,
        resource_kind: "exact_process_termination",
        scope_keys: PROCESS_CONTROL_SCOPE_KEYS,
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(2 * 60),
        absolute_ttl_seconds: Some(10 * 60),
        authorization_requirement: "driver_ownership_in_standard; unattended_in_bounded; optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source:
            "driver_ownership_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::BoundedOrUnrestricted,
    },
    EnforcementAdapterDescriptor {
        id: "os_permission_prompt",
        operations: OS_PERMISSION_PROMPT_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "operating_system_permission_prompt",
        scope_keys: &["daemon_generation", "permission_mode"],
        grant_type: None,
        idle_ttl_seconds: None,
        absolute_ttl_seconds: None,
        authorization_requirement: "never_agent_controllable",
        revocation_triggers: &[],
        refusal_code: Some("os_permission_prompt_requires_trusted_host"),
        authorization_source: "trusted_host_setup_outside_the_agent_tool_path",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Denied,
    },
    EnforcementAdapterDescriptor {
        id: "driver_configuration",
        operations: DRIVER_CONFIGURATION_OPERATIONS,
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "persistent_driver_configuration",
        scope_keys: &[
            "daemon_generation",
            "permission_mode",
            "managed_policy_sha256",
            "user_policy_sha256",
        ],
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(5 * 60),
        absolute_ttl_seconds: Some(30 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "clipboard",
        operations: &["clipboard_read", "clipboard_write"],
        state: RiskEnforcement::Active,
        risk_class: RiskClass::R2,
        resource_kind: "system_clipboard",
        scope_keys: &[
            "daemon_generation",
            "public_session",
            "operation",
            "content_kind",
            "permission_mode",
            "managed_policy_sha256",
            "user_policy_sha256",
        ],
        grant_type: Some("protected_resource_grant"),
        idle_ttl_seconds: Some(30 * 60),
        absolute_ttl_seconds: Some(8 * 60 * 60),
        authorization_requirement: "profile_behavior_and_optional_capability_manifest",
        revocation_triggers: SESSION_REVOCATION,
        refusal_code: Some("authorization_required"),
        authorization_source: "built_in_standard; unattended_bounded_profile; trusted_unrestricted_mode; optional_capability_manifest_ceiling",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::Active),
        profile_behavior: AdapterProfileBehavior::Routine,
    },
    EnforcementAdapterDescriptor {
        id: "devices",
        operations: &["microphone", "camera"],
        state: RiskEnforcement::NotExposed,
        risk_class: RiskClass::Unclassified,
        resource_kind: "device_capture",
        scope_keys: &[],
        grant_type: None,
        idle_ttl_seconds: None,
        absolute_ttl_seconds: None,
        authorization_requirement: "required_before_exposure",
        revocation_triggers: &[],
        refusal_code: None,
        authorization_source: "capability_not_exposed",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::NotExposed),
        profile_behavior: AdapterProfileBehavior::Denied,
    },
    EnforcementAdapterDescriptor {
        id: "shell_and_network",
        operations: &["generic_shell", "generic_network"],
        state: RiskEnforcement::NotExposed,
        risk_class: RiskClass::Unclassified,
        resource_kind: "open_ended_external_capability",
        scope_keys: &[],
        grant_type: None,
        idle_ttl_seconds: None,
        absolute_ttl_seconds: None,
        authorization_requirement: "required_before_exposure",
        revocation_triggers: &[],
        refusal_code: None,
        authorization_source: "capability_not_exposed",
        enforcement_by_mode: AdapterEnforcement::uniform(RiskEnforcement::NotExposed),
        profile_behavior: AdapterProfileBehavior::Denied,
    },
];

pub fn enforcement_adapter_inventory_json() -> Value {
    Value::Array(
        ENFORCEMENT_ADAPTERS
            .iter()
            .map(|adapter| {
                let mut value =
                    serde_json::to_value(adapter).expect("static adapter inventory serializes");
                value
                    .as_object_mut()
                    .expect("adapter descriptor serializes as an object")
                    .insert(
                        "behavior_by_mode".to_owned(),
                        serde_json::json!({
                            "standard": adapter.profile_behavior.for_mode(PermissionMode::Standard),
                            "bounded": adapter.profile_behavior.for_mode(PermissionMode::Bounded),
                            "unrestricted": adapter.profile_behavior.for_mode(PermissionMode::Unrestricted),
                        }),
                    );
                value
            })
            .collect(),
    )
}

pub fn adapter_ids_with_state(state: RiskEnforcement) -> Vec<&'static str> {
    ENFORCEMENT_ADAPTERS
        .iter()
        .filter(|adapter| adapter.state == state)
        .map(|adapter| adapter.id)
        .collect()
}

pub fn adapter_ids_with_state_for_mode(
    mode: PermissionMode,
    state: RiskEnforcement,
) -> Vec<&'static str> {
    ENFORCEMENT_ADAPTERS
        .iter()
        .filter(|adapter| adapter.enforcement_by_mode.for_mode(mode) == state)
        .map(|adapter| adapter.id)
        .collect()
}

/// Return every resource adapter that composes the exact call.
///
/// A call may intentionally require more than one adapter: screenshot output
/// is both private observation and a file write; trajectory recording is both
/// observation and file output; arbitrary page script is both consequential
/// and explicitly unbounded. Admission must satisfy the conjunction rather
/// than picking the highest-risk adapter and dropping the other boundary.
pub fn enforcement_adapters_for_call(
    tool: &str,
    args: &Value,
) -> Vec<&'static EnforcementAdapterDescriptor> {
    let mut ids = Vec::new();
    let mut add = |id| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };

    if tool == "browser_prepare"
        && args.pointer("/strategy/kind").and_then(Value::as_str) == Some("existing_profile")
    {
        add("browser_prepare.existing_profile");
    } else if tool == "browser_prepare"
        && matches!(
            args.pointer("/profile/mode").and_then(Value::as_str),
            Some("isolated_new" | "isolated_named")
        )
    {
        add("browser_prepare.isolated");
    }

    if matches!(
        tool,
        "get_desktop_state"
            | "get_accessibility_tree"
            | "get_window_state"
            | "verify_state"
            | "list_apps"
            | "list_windows"
            | "debug_window_info"
            | "get_browser_state"
            | "escalate_session"
            | "zoom"
            | "start_recording"
    ) || (tool == "page"
        && matches!(
            args.get("action").and_then(Value::as_str),
            Some("get_text" | "query_dom")
        ))
        || (tool == "browser_dialog"
            && args.get("action").and_then(Value::as_str) == Some("inspect"))
    {
        add("private_observation");
    }

    if matches!(tool, "history_status" | "history_query") {
        add("computer_history");
    }

    if DESKTOP_INPUT_OPERATIONS.contains(&tool) {
        add("desktop_input");
    }

    if matches!(tool, "clipboard_read" | "clipboard_write") {
        add("clipboard");
    }

    let writes_screenshot = matches!(tool, "get_desktop_state" | "get_window_state")
        && args
            .get("screenshot_out_file")
            .and_then(Value::as_str)
            .is_some_and(|path| !path.is_empty());
    if matches!(
        tool,
        "browser_set_input_files"
            | "browser_download"
            | "start_recording"
            | "stop_recording"
            | "replay_trajectory"
    ) || writes_screenshot
        || (tool == "clipboard_write"
            && (args.get("image_path").and_then(Value::as_str).is_some()
                || args.get("file_path").and_then(Value::as_str).is_some()))
        || (tool == "install_ffmpeg" && args.get("confirm").and_then(Value::as_bool) == Some(true))
    {
        add("file_transfer_and_output");
    }

    if tool == "browser_dialog"
        && matches!(
            args.get("action").and_then(Value::as_str),
            Some("accept" | "dismiss")
        )
    {
        add("browser_consequential_action");
    }

    if tool == "page"
        && args
            .get("action")
            .and_then(Value::as_str)
            .is_some_and(|action| LEGACY_PAGE_MUTATING_ACTIONS.contains(&action))
    {
        add("browser_unbounded_script");
    }

    if BROWSER_BOUND_INPUT_OPERATIONS.contains(&tool) {
        add("browser_bound_input");
    }

    if PROCESS_CONTROL_OPERATIONS.contains(&tool) {
        add("process_control");
    }

    if tool == "check_permissions" && args.get("prompt").and_then(Value::as_bool).unwrap_or(false) {
        add("os_permission_prompt");
    }

    if DRIVER_CONFIGURATION_OPERATIONS.contains(&tool) {
        add("driver_configuration");
    }

    ids.into_iter()
        .map(|id| {
            ENFORCEMENT_ADAPTERS
                .iter()
                .find(|adapter| adapter.id == id)
                .expect("call mapping references a declared enforcement adapter")
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RiskAssessment {
    pub class: RiskClass,
    pub enforcement: RiskEnforcement,
    pub operation_sensitive: bool,
}

fn advertised_enforcement_for(tool: &str) -> RiskEnforcement {
    let selector_matches = |selector: &&str| {
        *selector == tool
            || selector
                .strip_prefix(tool)
                .is_some_and(|suffix| suffix.starts_with('['))
    };
    if ENFORCEMENT_ADAPTERS.iter().any(|adapter| {
        adapter.state == RiskEnforcement::Active && adapter.operations.iter().any(selector_matches)
    }) {
        RiskEnforcement::Active
    } else {
        RiskEnforcement::MetadataOnly
    }
}

/// Highest reviewed risk advertised for a tool. Runtime authorization uses
/// [`classify_tool_call`] so compound tools can narrow to a typed operation.
pub fn advertised_risk_for(tool: &str) -> RiskAssessment {
    let class = match tool {
        // Public driver/OS metadata with no user-content payload.
        "get_screen_size"
        | "get_cursor_position"
        | "get_config"
        | "get_session"
        | "list_sessions"
        | "get_session_state"
        | "get_agent_cursor_state"
        | "get_recording_state"
        | "check_for_update"
        | "health_report"
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
        | "invoke_menu"
        | "launch_app"
        | "bring_to_front"
        | "set_window_frame"
        | "start_session"
        | "end_session"
        | "set_agent_cursor_enabled"
        | "set_agent_cursor_motion"
        | "set_agent_cursor_theme" => RiskClass::R1,

        "clipboard_write" => RiskClass::R1,

        // Surfaces that can reveal or control sensitive local/authenticated
        // state. Active adapters still decide their exact resource scope at
        // the canonical registry boundary before platform dispatch.
        "zoom"
        | "clipboard_read"
        | "list_apps"
        | "list_windows"
        | "debug_window_info"
        | "check_permissions"
        | "get_accessibility_tree"
        | "verify_state"
        | "set_config"
        | "escalate_session"
        | "start_recording"
        | "get_browser_state"
        | "browser_prepare"
        | "browser_navigate"
        | "browser_click"
        | "browser_type"
        | "browser_pointer"
        | "history_status"
        | "history_query" => RiskClass::R2,

        // External/file side effects or generic compound action surfaces.
        "get_desktop_state"
        | "get_window_state"
        | "kill_app"
        | "stop_recording"
        | "replay_trajectory"
        | "install_ffmpeg"
        | "page"
        | "browser_dialog"
        | "browser_set_input_files"
        | "browser_download" => RiskClass::R3,

        _ => RiskClass::Unclassified,
    };
    RiskAssessment {
        class,
        // Tool discovery cannot see the future arguments of a compound tool.
        // Advertise the strongest shipped enforcement for any of its typed
        // operations, then let classify_tool_call narrow the exact request.
        enforcement: advertised_enforcement_for(tool),
        operation_sensitive: matches!(class, RiskClass::R2 | RiskClass::R3 | RiskClass::R4),
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
                    enforcement: RiskEnforcement::Active,
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
                enforcement: if matches!(
                    args.get("action").and_then(Value::as_str),
                    Some("get_text" | "query_dom")
                ) {
                    RiskEnforcement::Active
                } else {
                    RiskEnforcement::MetadataOnly
                },
                operation_sensitive: true,
            }
        }
        "get_desktop_state" | "get_window_state" => RiskAssessment {
            class: if args
                .get("screenshot_out_file")
                .and_then(Value::as_str)
                .is_some_and(|path| !path.is_empty())
            {
                RiskClass::R3
            } else {
                RiskClass::R2
            },
            // Screenshot-to-file composes the active observation and exact
            // file-output adapters at the canonical dispatch boundary.
            enforcement: RiskEnforcement::Active,
            operation_sensitive: true,
        },
        "verify_state" => RiskAssessment {
            class: RiskClass::R2,
            enforcement: RiskEnforcement::Active,
            operation_sensitive: true,
        },
        "get_accessibility_tree"
        | "list_apps"
        | "list_windows"
        | "debug_window_info"
        | "get_browser_state"
        | "escalate_session"
        | "zoom"
        | "clipboard_read" => RiskAssessment {
            class: RiskClass::R2,
            enforcement: RiskEnforcement::Active,
            operation_sensitive: true,
        },
        "history_status" | "history_query" => RiskAssessment {
            class: RiskClass::R2,
            enforcement: RiskEnforcement::Active,
            operation_sensitive: true,
        },
        "check_permissions" => RiskAssessment {
            class: if args.get("prompt").and_then(Value::as_bool).unwrap_or(false) {
                RiskClass::R2
            } else {
                RiskClass::R0
            },
            enforcement: RiskEnforcement::MetadataOnly,
            operation_sensitive: args.get("prompt").and_then(Value::as_bool).unwrap_or(false),
        },
        "browser_set_input_files"
        | "browser_download"
        | "start_recording"
        | "stop_recording"
        | "replay_trajectory" => RiskAssessment {
            class: RiskClass::R3,
            enforcement: RiskEnforcement::Active,
            operation_sensitive: true,
        },
        "clipboard_write" => RiskAssessment {
            class: if args.get("image_path").and_then(Value::as_str).is_some()
                || args.get("file_path").and_then(Value::as_str).is_some()
            {
                RiskClass::R3
            } else {
                RiskClass::R1
            },
            enforcement: RiskEnforcement::Active,
            operation_sensitive: true,
        },
        "install_ffmpeg" => {
            let confirmed = args.get("confirm").and_then(Value::as_bool) == Some(true);
            RiskAssessment {
                class: if confirmed {
                    RiskClass::R3
                } else {
                    RiskClass::R0
                },
                enforcement: if confirmed {
                    RiskEnforcement::Active
                } else {
                    RiskEnforcement::MetadataOnly
                },
                operation_sensitive: confirmed,
            }
        }
        tool if DESKTOP_INPUT_OPERATIONS.contains(&tool) && tool != "replay_trajectory" => {
            let mut risk = advertised_risk_for(tool);
            risk.enforcement = RiskEnforcement::Active;
            risk
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
    let context = crate::session_authorization::configured_registry()
        .and_then(|registry| registry.legacy_context())
        .map_err(crate::policy::AuthorizationError::Loading)?;
    authorize_tool_call_with_context(tool, args, context.as_ref())
}

/// Context-aware authorization entry point. The context must originate from
/// the process-owned registry; caller-supplied session strings or tool
/// arguments are never authorization inputs.
pub fn authorize_tool_call_with_context(
    tool: &str,
    args: &Value,
    context: &crate::session_authorization::EffectiveAuthorizationContext,
) -> Result<RiskAssessment, crate::policy::AuthorizationError> {
    enforce_hard_invariants(tool, args)?;
    if context.is_expired() {
        return Err(crate::policy::AuthorizationError::Denied(
            "authorization context expired".to_owned(),
        ));
    }
    crate::policy::authorize_tool_call(tool, args)?;
    let risk = classify_tool_call(tool, args);
    if risk.class == RiskClass::Unclassified {
        return Err(crate::policy::AuthorizationError::Denied(format!(
            "tool '{tool}' has no reviewed risk classification"
        )));
    }
    let mode = context.mode();
    if mode == PermissionMode::Bounded && context.capability_manifest().is_none() {
        return Err(crate::policy::AuthorizationError::Loading(
            "bounded mode capability manifest is unavailable".to_owned(),
        ));
    }
    if let Some(manifest) = context.capability_manifest() {
        manifest
            .validate_for_mode(mode)
            .map_err(crate::policy::AuthorizationError::Denied)?;
        match manifest.decision(tool) {
            crate::session_manifest::ManifestDecision::Allow => manifest
                .authorize_call(tool, args)
                .map_err(crate::policy::AuthorizationError::Denied)?,
            crate::session_manifest::ManifestDecision::Deny
            | crate::session_manifest::ManifestDecision::Ask => {
                return Err(crate::policy::AuthorizationError::Denied(format!(
                    "capability manifest denies tool '{tool}'"
                )))
            }
            crate::session_manifest::ManifestDecision::Undeclared => {
                return Err(crate::policy::AuthorizationError::Denied(format!(
                    "tool '{tool}' is outside the capability manifest"
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
    // Provider and indicator adapters hosted by the runtime must never become
    // an ordinary process-targeted Cua action. This protects the runtime's own
    // controls from its PID-bound tools. It does not turn an ordinary
    // same-desktop window into a secure-desktop boundary: coordinate-only
    // input from Cua or another automation runtime remains a separate threat.
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
            | "verify_state"
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Standard,
    #[serde(alias = "autonomous")]
    Bounded,
    Unrestricted,
}

impl PermissionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Bounded => "bounded",
            Self::Unrestricted => "unrestricted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PermissionModeError {
    #[error("unknown permission mode '{0}'; expected standard, bounded, or unrestricted")]
    Unknown(String),
    #[error(
        "permission mode unrestricted requires --dangerously-bypass-approvals at trusted daemon startup"
    )]
    MissingDangerAcknowledgement,
    #[error("--dangerously-bypass-approvals cannot be combined with a non-unrestricted --permission-mode")]
    UnexpectedDangerAcknowledgement,
    #[error(
        "permission mode bounded requires --capability-manifest <path> at trusted daemon startup"
    )]
    MissingSessionPolicy,
    #[error("a configured capability manifest requires --approve-capability-manifest at trusted daemon startup")]
    MissingSessionPolicyApproval,
    #[error("--approve-capability-manifest requires --capability-manifest <path>")]
    UnexpectedSessionPolicy,
    #[error("permission mode unrestricted is disabled by managed startup configuration")]
    UnrestrictedDisabled,
}

fn parse_permission_mode(
    configured: Option<&str>,
    dangerous_bypass: bool,
) -> Result<PermissionMode, PermissionModeError> {
    let mode = match configured.unwrap_or("standard") {
        "standard" => PermissionMode::Standard,
        "bounded" | "autonomous" => PermissionMode::Bounded,
        "unrestricted" | "yolo" => PermissionMode::Unrestricted,
        other => return Err(PermissionModeError::Unknown(other.to_owned())),
    };
    match (mode, dangerous_bypass) {
        (PermissionMode::Unrestricted, false) => {
            Err(PermissionModeError::MissingDangerAcknowledgement)
        }
        (PermissionMode::Standard | PermissionMode::Bounded, true) => {
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
    let manifest_configured = crate::session_manifest::capability_manifest_configured();
    let manifest_approved = crate::session_manifest::capability_manifest_approved();
    if mode == PermissionMode::Bounded && !manifest_configured {
        return Err(PermissionModeError::MissingSessionPolicy.into());
    }
    if manifest_configured && !manifest_approved {
        return Err(PermissionModeError::MissingSessionPolicyApproval.into());
    }
    if manifest_approved && !manifest_configured {
        return Err(PermissionModeError::UnexpectedSessionPolicy.into());
    }
    if let Some(manifest) =
        crate::session_manifest::configured_capability_manifest().map_err(anyhow::Error::msg)?
    {
        manifest
            .validate_for_mode(mode)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// Content-free authorization state suitable for status/health output.
pub fn status_json() -> serde_json::Value {
    status_json_with_provider(None)
}

/// Content-free authorization state for one runtime-owned provider.
///
/// Provider identity is runtime state, never a process-global first-writer
/// value: multiple direct runtimes may install different trusted hosts.
pub fn status_json_with_provider(authorization_host: Option<&'static str>) -> serde_json::Value {
    let authorization_host_assurance = match authorization_host {
        Some(_) => "host_provided",
        None => "unavailable",
    };
    let mode = configured_permission_mode();
    let policy = crate::policy::configured_policy();
    let managed_policy = crate::policy::configured_managed_policy();
    let user_policy_sha256 = crate::policy::user_policy_sha256().ok().flatten();
    let managed_policy_sha256 = crate::policy::managed_policy_sha256().ok().flatten();
    let effective_active_risk_enforcement = mode
        .as_ref()
        .map(|mode| adapter_ids_with_state_for_mode(*mode, RiskEnforcement::Active))
        .unwrap_or_default();
    let effective_metadata_only_risk_enforcement = mode
        .as_ref()
        .map(|mode| adapter_ids_with_state_for_mode(*mode, RiskEnforcement::MetadataOnly))
        .unwrap_or_default();
    let effective_not_exposed_risk_enforcement = mode
        .as_ref()
        .map(|mode| adapter_ids_with_state_for_mode(*mode, RiskEnforcement::NotExposed))
        .unwrap_or_default();
    let capability_manifest = crate::session_manifest::configured_capability_manifest();
    let capability_manifest_status = capability_manifest
        .as_ref()
        .ok()
        .and_then(|manifest| *manifest)
        .map(|manifest| {
            let (allow, deny, ask) = manifest.counts();
            serde_json::json!({
                "version": manifest.version(),
                "sha256": manifest.sha256(),
                "expires_unix_ms": manifest.expires_unix_ms(),
                "idle_timeout_seconds": manifest.idle_timeout().map(|duration| duration.as_secs()),
                "expired": manifest.is_expired(),
                "idle_expired": manifest.is_idle_expired(),
                "allow_count": allow,
                "deny_count": deny,
                "ask_count": ask,
            })
        });
    let mut status = serde_json::json!({
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
        "risk_metadata_version": RISK_METADATA_VERSION,
        "active_risk_enforcement": adapter_ids_with_state(RiskEnforcement::Active),
        "metadata_only_risk_enforcement": adapter_ids_with_state(RiskEnforcement::MetadataOnly),
        "not_exposed_risk_enforcement": adapter_ids_with_state(RiskEnforcement::NotExposed),
        "effective_active_risk_enforcement": effective_active_risk_enforcement,
        "effective_metadata_only_risk_enforcement": effective_metadata_only_risk_enforcement,
        "effective_not_exposed_risk_enforcement": effective_not_exposed_risk_enforcement,
        "enforcement_adapters": enforcement_adapter_inventory_json(),
        "authorization_host": authorization_host,
        "authorization_host_assurance": authorization_host_assurance,
        "capability_manifest_configured": crate::session_manifest::capability_manifest_configured(),
        "capability_manifest_approved_at_startup": crate::session_manifest::capability_manifest_approved(),
        "capability_manifest_valid": capability_manifest.is_ok(),
        "capability_manifest": capability_manifest_status,
        // Deprecated status aliases retained for one compatibility window.
        "session_policy_configured": crate::session_manifest::capability_manifest_configured(),
        "session_policy_approved_at_startup": crate::session_manifest::capability_manifest_approved(),
        "session_policy_valid": capability_manifest.is_ok(),
        "session_policy": capability_manifest_status,
    });
    let session_status = crate::session_authorization::status_json();
    if let (Some(target), Some(session_status)) =
        (status.as_object_mut(), session_status.as_object())
    {
        target.extend(session_status.clone());
    }
    status
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
    fn bounded_needs_no_bypass_flag() {
        assert_eq!(
            parse_permission_mode(Some("bounded"), false).unwrap(),
            PermissionMode::Bounded
        );
    }

    #[test]
    fn autonomous_remains_a_compatibility_alias_for_bounded() {
        assert_eq!(
            parse_permission_mode(Some("autonomous"), false).unwrap(),
            PermissionMode::Bounded
        );
        assert_eq!(
            serde_json::from_str::<PermissionMode>("\"autonomous\"").unwrap(),
            PermissionMode::Bounded
        );
        assert_eq!(
            serde_json::to_string(&PermissionMode::Bounded).unwrap(),
            "\"bounded\""
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
    fn browser_prepare_operations_are_actively_enforced() {
        assert_eq!(
            advertised_risk_for("browser_prepare").enforcement,
            RiskEnforcement::Active,
            "tools/list must advertise that at least one browser_prepare operation is actively enforced"
        );
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
        assert_eq!(isolated.enforcement, RiskEnforcement::Active);
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
    fn health_report_is_r0_read_only_diagnostics() {
        let risk = advertised_risk_for("health_report");
        assert_eq!(risk.class, RiskClass::R0);
        assert_eq!(risk.enforcement, RiskEnforcement::MetadataOnly);
        assert!(!risk.operation_sensitive);
    }

    #[test]
    fn history_reads_are_distinct_explicit_history_capabilities() {
        for (tool, capability) in [
            ("history_status", "history.status"),
            ("history_query", "history.query"),
        ] {
            let risk = classify_tool_call(tool, &serde_json::json!({}));
            assert_eq!(risk.class, RiskClass::R2);
            assert_eq!(risk.enforcement, RiskEnforcement::Active);
            assert_eq!(crate::tool::default_capabilities_for(tool), &[capability]);
            assert_eq!(
                enforcement_adapters_for_call(tool, &serde_json::json!({}))
                    .into_iter()
                    .map(|adapter| adapter.id)
                    .collect::<Vec<_>>(),
                vec!["computer_history"]
            );
        }
        let adapter = ENFORCEMENT_ADAPTERS
            .iter()
            .find(|adapter| adapter.id == "computer_history")
            .unwrap();
        assert_eq!(
            adapter.profile_behavior.for_mode(PermissionMode::Standard),
            ModeBehavior::RequireGrant
        );
        assert_eq!(
            adapter.profile_behavior.for_mode(PermissionMode::Bounded),
            ModeBehavior::AllowWithoutGrant
        );
    }

    #[test]
    fn native_menu_invocation_matches_local_gui_action_risk() {
        let risk = advertised_risk_for("invoke_menu");
        assert_eq!(risk.class, RiskClass::R1);
        assert_eq!(risk.enforcement, RiskEnforcement::MetadataOnly);
        assert!(!risk.operation_sensitive);
    }

    #[test]
    fn screenshot_file_output_is_classified_as_egress() {
        for tool in ["get_desktop_state", "get_window_state"] {
            let observation = classify_tool_call(tool, &serde_json::json!({}));
            assert_eq!(observation.class, RiskClass::R2);
            assert_eq!(observation.enforcement, RiskEnforcement::Active);

            let egress = classify_tool_call(
                tool,
                &serde_json::json!({"screenshot_out_file": "/tmp/capture.png"}),
            );
            assert_eq!(egress.class, RiskClass::R3);
            assert_eq!(egress.enforcement, RiskEnforcement::Active);
            assert!(egress.operation_sensitive);
            assert_eq!(advertised_risk_for(tool).class, RiskClass::R3);
        }
    }

    #[test]
    fn clipboard_risk_distinguishes_reads_text_writes_and_local_files() {
        let read = classify_tool_call("clipboard_read", &serde_json::json!({}));
        assert_eq!(read.class, RiskClass::R2);
        assert_eq!(read.enforcement, RiskEnforcement::Active);

        let text = classify_tool_call(
            "clipboard_write",
            &serde_json::json!({"text": "private content"}),
        );
        assert_eq!(text.class, RiskClass::R1);
        assert_eq!(text.enforcement, RiskEnforcement::Active);

        for args in [
            serde_json::json!({"image_path": "/tmp/private-content"}),
            serde_json::json!({"file_path": "/tmp/private-content"}),
        ] {
            let file = classify_tool_call("clipboard_write", &args);
            assert_eq!(file.class, RiskClass::R3);
            assert_eq!(file.enforcement, RiskEnforcement::Active);
        }
    }

    #[test]
    fn verify_state_is_bounded_observation_not_file_egress() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"include_screenshot": true}),
        ] {
            let observation = classify_tool_call("verify_state", &args);
            assert_eq!(observation.class, RiskClass::R2);
            assert_eq!(observation.enforcement, RiskEnforcement::Active);
        }
        assert_eq!(advertised_risk_for("verify_state").class, RiskClass::R2);
    }

    #[test]
    fn os_permission_prompt_is_argument_sensitive() {
        for args in [serde_json::json!({}), serde_json::json!({"prompt": false})] {
            let read_only = classify_tool_call("check_permissions", &args);
            assert_eq!(read_only.class, RiskClass::R0);
            assert!(!read_only.operation_sensitive);
        }
        let prompting =
            classify_tool_call("check_permissions", &serde_json::json!({"prompt": true}));
        assert_eq!(prompting.class, RiskClass::R2);
        assert!(prompting.operation_sensitive);
        assert_eq!(
            advertised_risk_for("check_permissions").class,
            RiskClass::R2
        );
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

    #[test]
    fn enforcement_inventory_is_unique_and_truthful() {
        let mut ids = std::collections::BTreeSet::new();
        for adapter in ENFORCEMENT_ADAPTERS {
            assert!(
                ids.insert(adapter.id),
                "duplicate adapter id {}",
                adapter.id
            );
            assert!(
                !adapter.operations.is_empty(),
                "adapter {} needs at least one operation selector",
                adapter.id
            );
            assert_eq!(
                adapter.state, adapter.enforcement_by_mode.standard,
                "legacy state must remain the standard-mode value for {}",
                adapter.id
            );
        }

        assert_eq!(
            adapter_ids_with_state(RiskEnforcement::Active),
            vec![
                "browser_prepare.isolated",
                "browser_prepare.existing_profile",
                "private_observation",
                "computer_history",
                "desktop_input",
                "file_transfer_and_output",
                "browser_consequential_action",
                "browser_unbounded_script",
                "browser_bound_input",
                "process_control",
                "os_permission_prompt",
                "driver_configuration",
                "clipboard",
            ]
        );
        assert_eq!(
            adapter_ids_with_state(RiskEnforcement::MetadataOnly),
            Vec::<&str>::new()
        );
        assert_eq!(
            adapter_ids_with_state(RiskEnforcement::NotExposed),
            vec!["devices", "shell_and_network"]
        );
        assert_eq!(
            adapter_ids_with_state_for_mode(PermissionMode::Standard, RiskEnforcement::Active),
            vec![
                "browser_prepare.isolated",
                "browser_prepare.existing_profile",
                "private_observation",
                "computer_history",
                "desktop_input",
                "file_transfer_and_output",
                "browser_consequential_action",
                "browser_unbounded_script",
                "browser_bound_input",
                "process_control",
                "os_permission_prompt",
                "driver_configuration",
                "clipboard",
            ]
        );

        let existing = ENFORCEMENT_ADAPTERS
            .iter()
            .find(|adapter| adapter.id == "browser_prepare.existing_profile")
            .unwrap();
        assert_eq!(existing.idle_ttl_seconds, Some(30 * 60));
        assert_eq!(existing.absolute_ttl_seconds, Some(8 * 60 * 60));
        assert_eq!(existing.refusal_code, Some("browser_consent_required"));
    }

    #[test]
    fn adapter_mode_behavior_matches_the_product_contract() {
        let behavior = |id: &str, mode| {
            ENFORCEMENT_ADAPTERS
                .iter()
                .find(|adapter| adapter.id == id)
                .unwrap()
                .profile_behavior
                .for_mode(mode)
        };

        for id in [
            "browser_prepare.isolated",
            "private_observation",
            "desktop_input",
            "file_transfer_and_output",
            "browser_consequential_action",
            "browser_bound_input",
            "driver_configuration",
            "clipboard",
        ] {
            assert_eq!(behavior(id, PermissionMode::Standard), ModeBehavior::Allow);
            assert_eq!(
                behavior(id, PermissionMode::Bounded),
                ModeBehavior::AllowWithoutGrant
            );
            assert_eq!(
                behavior(id, PermissionMode::Unrestricted),
                ModeBehavior::Allow
            );
        }
        assert_eq!(
            behavior("browser_prepare.existing_profile", PermissionMode::Standard),
            ModeBehavior::RequireGrant
        );
        assert_eq!(
            behavior("process_control", PermissionMode::Standard),
            ModeBehavior::Deny
        );
        assert_eq!(
            behavior("browser_unbounded_script", PermissionMode::Bounded),
            ModeBehavior::Deny
        );
        assert_eq!(
            behavior("os_permission_prompt", PermissionMode::Unrestricted),
            ModeBehavior::Deny
        );
    }

    #[test]
    fn adding_a_manifest_is_monotonic_for_standard_and_unrestricted_profile_behavior() {
        let authority = |behavior| match behavior {
            ModeBehavior::Deny => 0,
            ModeBehavior::RequireGrant => 1,
            ModeBehavior::Allow | ModeBehavior::AllowWithoutGrant => 2,
        };

        // Manifest admission is evaluated before the unchanged profile
        // behavior. Exhaust every adapter and every manifest decision: an
        // in-scope allow preserves the profile result, while deny, legacy ask,
        // and undeclared scope all reduce it to deny.
        for mode in [PermissionMode::Standard, PermissionMode::Unrestricted] {
            for adapter in ENFORCEMENT_ADAPTERS {
                let baseline = adapter.profile_behavior.for_mode(mode);
                for manifest_decision in [
                    crate::session_manifest::ManifestDecision::Allow,
                    crate::session_manifest::ManifestDecision::Deny,
                    crate::session_manifest::ManifestDecision::Ask,
                    crate::session_manifest::ManifestDecision::Undeclared,
                ] {
                    let with_manifest = match manifest_decision {
                        crate::session_manifest::ManifestDecision::Allow => baseline,
                        crate::session_manifest::ManifestDecision::Deny
                        | crate::session_manifest::ManifestDecision::Ask
                        | crate::session_manifest::ManifestDecision::Undeclared => {
                            ModeBehavior::Deny
                        }
                    };
                    assert!(
                        authority(with_manifest) <= authority(baseline),
                        "adding {manifest_decision:?} scope widened {} in {mode:?} from {baseline:?} to {with_manifest:?}",
                        adapter.id,
                    );
                }
            }
        }
    }

    #[test]
    fn standard_manifest_scope_is_checked_before_existing_profile_approval() {
        use std::io::Write as _;
        use std::sync::Arc;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "version: 3\nallow:\n  tools: [browser_prepare]\nresources:\n  browser:\n    existing_profiles:\n      - pid: 42\n        window_id: 7"
        )
        .unwrap();
        let manifest = Arc::new(crate::session_manifest::load_manifest(file.path()).unwrap());
        let ceiling = crate::session_authorization::SessionModeCeiling::for_trusted_sessions(
            [PermissionMode::Standard],
            false,
            std::time::Duration::from_secs(60),
            std::time::Duration::from_secs(30),
        )
        .unwrap();
        let context =
            crate::session_authorization::SessionAuthorizationRegistry::with_ceiling(ceiling)
                .compatibility_context(PermissionMode::Standard, Some(manifest))
                .unwrap();

        let call = |window_id| {
            authorize_tool_call_with_context(
                "browser_prepare",
                &serde_json::json!({
                    "pid": 42,
                    "window_id": window_id,
                    "strategy": {"kind": "existing_profile"},
                }),
                &context,
            )
        };
        assert!(call(7).is_ok());
        assert!(call(8)
            .unwrap_err()
            .to_string()
            .contains("outside the capability manifest"));
    }

    #[test]
    fn exact_call_inventory_composes_overlapping_resource_boundaries() {
        let ids = |tool, args: Value| {
            enforcement_adapters_for_call(tool, &args)
                .into_iter()
                .map(|adapter| adapter.id)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            ids(
                "get_desktop_state",
                serde_json::json!({"screenshot_out_file": "/tmp/capture.png"})
            ),
            vec!["private_observation", "file_transfer_and_output"]
        );
        assert_eq!(
            ids("page", serde_json::json!({"action": "execute_javascript"})),
            vec!["browser_unbounded_script"]
        );
        assert!(ids("page", serde_json::json!({})).is_empty());
        assert!(ids("page", serde_json::json!({"action": "unknown"})).is_empty());
        assert_eq!(
            ids(
                "browser_prepare",
                serde_json::json!({"profile": {"mode": "isolated_new"}})
            ),
            vec!["browser_prepare.isolated"]
        );
        assert_eq!(
            ids(
                "browser_prepare",
                serde_json::json!({"strategy": {"kind": "existing_profile"}})
            ),
            vec!["browser_prepare.existing_profile"]
        );
        assert_eq!(
            ids("browser_dialog", serde_json::json!({"action": "accept"})),
            vec!["browser_consequential_action"]
        );
        assert_eq!(
            ids("start_recording", serde_json::json!({})),
            vec!["private_observation", "file_transfer_and_output"]
        );
        assert_eq!(
            ids("escalate_session", serde_json::json!({})),
            vec!["private_observation"]
        );
        assert_eq!(
            ids("type_text_chars", serde_json::json!({})),
            vec!["desktop_input"]
        );
        assert_eq!(
            ids("set_window_frame", serde_json::json!({})),
            vec!["desktop_input"]
        );
        assert_eq!(
            ids("replay_trajectory", serde_json::json!({})),
            vec!["desktop_input", "file_transfer_and_output"]
        );
        assert_eq!(
            ids("install_ffmpeg", serde_json::json!({})),
            Vec::<&str>::new()
        );
        assert_eq!(
            ids("install_ffmpeg", serde_json::json!({"confirm": true})),
            vec!["file_transfer_and_output"]
        );
        assert_eq!(
            ids("get_browser_state", serde_json::json!({})),
            vec!["private_observation"]
        );
        assert_eq!(
            ids("browser_dialog", serde_json::json!({"action": "inspect"})),
            vec!["private_observation"]
        );
        assert_eq!(
            ids("browser_click", serde_json::json!({})),
            vec!["browser_bound_input"]
        );
        assert_eq!(
            ids("kill_app", serde_json::json!({})),
            vec!["process_control"]
        );
        assert_eq!(
            ids("check_permissions", serde_json::json!({"prompt": false})),
            Vec::<&str>::new()
        );
        assert_eq!(
            ids("check_permissions", serde_json::json!({})),
            Vec::<&str>::new()
        );
        assert_eq!(
            ids("set_config", serde_json::json!({})),
            vec!["driver_configuration"]
        );
        assert_eq!(
            ids("clipboard_read", serde_json::json!({})),
            vec!["clipboard"]
        );
        assert_eq!(
            ids("clipboard_write", serde_json::json!({"text": "private"})),
            vec!["clipboard"]
        );
        for args in [
            serde_json::json!({"image_path": "/tmp/private-content"}),
            serde_json::json!({"file_path": "/tmp/private-content"}),
        ] {
            assert_eq!(
                ids("clipboard_write", args),
                vec!["clipboard", "file_transfer_and_output"]
            );
        }
    }

    #[test]
    fn status_derives_adapter_summaries_from_the_inventory() {
        let status = status_json();
        assert_eq!(
            status["active_risk_enforcement"],
            serde_json::json!([
                "browser_prepare.isolated",
                "browser_prepare.existing_profile",
                "private_observation",
                "computer_history",
                "desktop_input",
                "file_transfer_and_output",
                "browser_consequential_action",
                "browser_unbounded_script",
                "browser_bound_input",
                "process_control",
                "os_permission_prompt",
                "driver_configuration",
                "clipboard"
            ])
        );
        assert_eq!(
            status["metadata_only_risk_enforcement"],
            serde_json::json!([])
        );
        assert_eq!(
            status["not_exposed_risk_enforcement"],
            serde_json::json!(["devices", "shell_and_network"])
        );
        assert_eq!(
            status["effective_active_risk_enforcement"],
            serde_json::json!([
                "browser_prepare.isolated",
                "browser_prepare.existing_profile",
                "private_observation",
                "computer_history",
                "desktop_input",
                "file_transfer_and_output",
                "browser_consequential_action",
                "browser_unbounded_script",
                "browser_bound_input",
                "process_control",
                "os_permission_prompt",
                "driver_configuration",
                "clipboard"
            ])
        );
        assert_eq!(
            status["effective_metadata_only_risk_enforcement"],
            status["metadata_only_risk_enforcement"]
        );
        assert_eq!(
            status["enforcement_adapters"],
            enforcement_adapter_inventory_json()
        );
        let existing_profile = status["enforcement_adapters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|adapter| adapter["id"] == "browser_prepare.existing_profile")
            .unwrap();
        assert_eq!(
            existing_profile["profile_behavior_class"],
            "grant_in_standard"
        );
        assert_eq!(
            existing_profile["behavior_by_mode"],
            serde_json::json!({
                "standard": "require_grant",
                "bounded": "manifest",
                "unrestricted": "allow",
            })
        );
        assert_eq!(status["authorization_host_assurance"], "unavailable");
        assert_eq!(
            status_json_with_provider(Some("cua_local_desktop_confirmation_v1"))
                ["authorization_host_assurance"],
            "host_provided"
        );
        assert_eq!(
            status_json_with_provider(Some("external_host_consent_v1"))
                ["authorization_host_assurance"],
            "host_provided"
        );
        assert_eq!(status["effective_session_mode_source"], "process");
        assert_eq!(status["session_mode_delegation_enabled"], false);
        assert_eq!(status["session_mode_delegation_ready"], false);
        assert_eq!(
            status["session_mode_delegation_blocker"],
            "authenticated_action_connection_required"
        );
        assert_eq!(
            status["daemon_authorization_ceiling"]["allowed_modes"],
            serde_json::json!(["standard"])
        );
    }
}
