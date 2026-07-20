//! Content-free product telemetry for Cua Driver.
//!
//! Routine telemetry is enabled by default, with precedence
//! `environment override -> persisted preference -> enabled`. Disabling
//! telemetry stops every request while retaining the local installation ID.
//! Normal uninstall also retains telemetry state; `telemetry reset-id` and
//! `uninstall --purge` are the explicit identity-erasure paths.
//!
//! Event builders in this module accept only fixed event names and bounded
//! properties. They never receive prompts, tool arguments/results, typed text,
//! screenshots, accessibility trees, application/window names, URLs, paths,
//! raw cursor identifiers/configuration values, or raw errors.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const POSTHOG_CAPTURE_URL: &str = "https://eu.i.posthog.com/capture/";
const POSTHOG_API_KEY: &str = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14";
const POSTHOG_TIMEOUT_SECS: u64 = 3;
const LIFECYCLE_RETRY_BACKOFF_SECS: u64 = 15 * 60;
const TOOL_TELEMETRY_LIMIT_PER_HOUR: usize = 1_000;
const TOOL_TELEMETRY_WINDOW: Duration = Duration::from_secs(60 * 60);

const HOME_SUBDIRECTORY: &str = ".cua-driver";
const LEGACY_HOME_SUBDIRECTORY: &str = ".cua-driver-rs";
const CONFIG_FILE_NAME: &str = "config.json";
const CONFIG_ENABLED_KEY: &str = "telemetry_enabled";
const TELEMETRY_ID_FILE_NAME: &str = ".telemetry_id";
const TELEMETRY_IDENTITY_LOCK_FILE_NAME: &str = ".telemetry_identity.lock";
const TELEMETRY_LIFECYCLE_LOCK_FILE_NAME: &str = ".telemetry_lifecycle.lock";
const TELEMETRY_RETRY_AFTER_FILE_NAME: &str = ".telemetry_retry_after";
const TELEMETRY_INSTALL_CHANNEL_FILE_NAME: &str = ".telemetry_install_channel";
const INSTALLATION_RECORDED_FILE_NAME: &str = ".installation_recorded";
const RELEASE_RECORDED_DIRECTORY: &str = ".release_installed";

pub const ENV_TELEMETRY_ENABLED: &str = "CUA_DRIVER_RS_TELEMETRY_ENABLED";
const ENV_TELEMETRY_ENABLED_COMPAT: &str = "CUA_TELEMETRY_ENABLED";
const ENV_TELEMETRY_DEBUG: &str = "CUA_DRIVER_RS_TELEMETRY_DEBUG";
const ENV_TELEMETRY_SYNTHETIC: &str = "CUA_DRIVER_TELEMETRY_SYNTHETIC";
const ENV_TELEMETRY_HOME: &str = "CUA_DRIVER_TELEMETRY_HOME";
const ENV_CLI_WRAPPED_CHILD: &str = "CUA_DRIVER_CLI_TELEMETRY_CHILD";
const ENV_CLI_COMPLETION_WORKER: &str = "CUA_DRIVER_CLI_TELEMETRY_WORKER";
const ENV_CLI_COMPLETION_COMMAND: &str = "CUA_DRIVER_CLI_TELEMETRY_COMMAND";
const ENV_CLI_COMPLETION_TOOL: &str = "CUA_DRIVER_CLI_TELEMETRY_TOOL";
const ENV_CLI_COMPLETION_COMPUTER_ACTION: &str = "CUA_DRIVER_CLI_TELEMETRY_COMPUTER_ACTION";
const ENV_CLI_COMPLETION_OPERATION: &str = "CUA_DRIVER_CLI_TELEMETRY_OPERATION";
const ENV_CLI_COMPLETION_CLIENT_KIND: &str = "CUA_DRIVER_CLI_TELEMETRY_CLIENT_KIND";
const ENV_CLI_COMPLETION_EXIT_CODE: &str = "CUA_DRIVER_CLI_TELEMETRY_EXIT_CODE";
const ENV_CLI_COMPLETION_DURATION_MS: &str = "CUA_DRIVER_CLI_TELEMETRY_DURATION_MS";
const ENV_LIFECYCLE_WORKER: &str = "CUA_DRIVER_LIFECYCLE_TELEMETRY_WORKER";
pub const ENV_INSTALL_CHANNEL: &str = "CUA_DRIVER_INSTALL_CHANNEL";
pub const ENV_RELEASE_VERSION: &str = "CUA_DRIVER_RELEASE_VERSION";

pub mod event {
    pub const INSTALLATION_REGISTERED: &str = "cua_driver_installation_registered";
    pub const RELEASE_INSTALLED: &str = "cua_driver_release_installed";
    pub const SERVE_START_LEGACY: &str = "cua_driver_serve";
    pub const CLI_COMPLETED: &str = "cua_driver_cli_completed";
    pub const MCP_SESSION_STARTED: &str = "cua_driver_mcp_session_started";
    pub const MCP_TOOL_COMPLETED: &str = "cua_driver_mcp_tool_completed";
    pub const MCP_STARTUP_COMPLETED: &str = "cua_driver_mcp_startup_completed";
    pub const AGENT_SESSION_STARTED: &str = "cua_driver_agent_session_started";
    pub const AGENT_SESSION_ENDED: &str = "cua_driver_agent_session_ended";
    pub const PERMISSIONS_GATE_STARTED: &str = "cua_driver_permissions_gate_started";
    pub const PERMISSIONS_GATE_DISMISSED: &str = "cua_driver_permissions_gate_dismissed";
    pub const PERMISSIONS_GATE_COMPLETED: &str = "cua_driver_permissions_gate_completed";
}

const INSPECTABLE_EVENTS: &[&str] = &[
    event::INSTALLATION_REGISTERED,
    event::RELEASE_INSTALLED,
    event::SERVE_START_LEGACY,
    event::CLI_COMPLETED,
    event::MCP_SESSION_STARTED,
    event::MCP_TOOL_COMPLETED,
    event::MCP_STARTUP_COMPLETED,
    event::AGENT_SESSION_STARTED,
    event::AGENT_SESSION_ENDED,
    event::PERMISSIONS_GATE_STARTED,
    event::PERMISSIONS_GATE_DISMISSED,
    event::PERMISSIONS_GATE_COMPLETED,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Cli,
    Daemon,
    McpStdio,
    McpHttp,
}

impl Transport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Daemon => "daemon",
            Self::McpStdio => "mcp_stdio",
            Self::McpHttp => "mcp_http",
        }
    }
}

#[derive(Clone, Debug)]
struct InstallationIdentity {
    id: String,
    persisted: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TelemetryStatus {
    pub enabled: bool,
    pub source: &'static str,
    pub installation_id_present: bool,
    pub installation_id: Option<String>,
    pub registration_recorded: bool,
    pub current_release_recorded: bool,
}

pub fn is_enabled() -> bool {
    effective_enabled().0
}

fn effective_enabled() -> (bool, &'static str) {
    if let Some(value) = parse_env_bool(ENV_TELEMETRY_ENABLED) {
        return (value, "environment");
    }
    if let Some(value) = parse_env_bool(ENV_TELEMETRY_ENABLED_COMPAT) {
        return (value, "environment_compat");
    }
    if let Some(value) = persisted_enabled() {
        return (value, "persisted");
    }
    (true, "default")
}

pub fn status() -> TelemetryStatus {
    let (enabled, source) = effective_enabled();
    let id = read_install_id();
    TelemetryStatus {
        enabled,
        source,
        installation_id_present: id.is_some(),
        installation_id: id.as_deref().map(redact_id),
        registration_recorded: marker_path(INSTALLATION_RECORDED_FILE_NAME)
            .is_some_and(|path| path.exists()),
        current_release_recorded: release_marker_path(current_product_version())
            .is_some_and(|path| path.exists()),
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "home directory is unavailable".to_owned())?;
    let mut config = read_config();
    let object = config
        .as_object_mut()
        .ok_or_else(|| "telemetry config is not a JSON object".to_owned())?;
    object.insert(CONFIG_ENABLED_KEY.to_owned(), Value::Bool(enabled));
    write_json_atomic(&path, &config)
}

pub fn reset_id() -> Result<(), String> {
    let Some(home) = telemetry_home_dir() else {
        return Ok(());
    };
    let legacy = if std::env::var_os(ENV_TELEMETRY_HOME).is_none() {
        home_root().map(|root| root.join(LEGACY_HOME_SUBDIRECTORY))
    } else {
        None
    };
    reset_id_in_homes(&home, legacy.as_deref())
}

fn reset_id_in_homes(home: &Path, legacy: Option<&Path>) -> Result<(), String> {
    let lifecycle_lock = open_lock_file(&home, TELEMETRY_LIFECYCLE_LOCK_FILE_NAME)?;
    lifecycle_lock
        .lock()
        .map_err(|error| format!("failed to lock telemetry lifecycle: {error}"))?;
    let identity_lock = open_lock_file(&home, TELEMETRY_IDENTITY_LOCK_FILE_NAME)?;
    identity_lock
        .lock()
        .map_err(|error| format!("failed to lock telemetry identity: {error}"))?;
    remove_file_if_exists(&home.join(TELEMETRY_ID_FILE_NAME))?;
    remove_file_if_exists(&home.join(INSTALLATION_RECORDED_FILE_NAME))?;
    remove_file_if_exists(&home.join(TELEMETRY_RETRY_AFTER_FILE_NAME))?;
    remove_file_if_exists(&home.join(TELEMETRY_INSTALL_CHANNEL_FILE_NAME))?;
    let releases = home.join(RELEASE_RECORDED_DIRECTORY);
    if releases.exists() {
        std::fs::remove_dir_all(&releases)
            .map_err(|error| format!("failed to remove {}: {error}", releases.display()))?;
    }
    if let Some(legacy) = legacy {
        remove_file_if_exists(&legacy.join(TELEMETRY_ID_FILE_NAME))?;
        remove_file_if_exists(&legacy.join(INSTALLATION_RECORDED_FILE_NAME))?;
        remove_file_if_exists(&legacy.join(TELEMETRY_RETRY_AFTER_FILE_NAME))?;
        remove_file_if_exists(&legacy.join(TELEMETRY_INSTALL_CHANNEL_FILE_NAME))?;
        let legacy_releases = legacy.join(RELEASE_RECORDED_DIRECTORY);
        if legacy_releases.exists() {
            std::fs::remove_dir_all(&legacy_releases).map_err(|error| {
                format!("failed to remove {}: {error}", legacy_releases.display())
            })?;
        }
    }
    Ok(())
}

pub fn inspect_event(event_name: &str) -> Result<Value, String> {
    if !INSPECTABLE_EVENTS.contains(&event_name) {
        return Err(format!(
            "unknown telemetry event; expected one of: {}",
            INSPECTABLE_EVENTS.join(", ")
        ));
    }
    let identity = InstallationIdentity {
        id: "redacted-installation-id".to_owned(),
        persisted: read_install_id().is_some(),
    };
    let properties = match event_name {
        event::INSTALLATION_REGISTERED | event::RELEASE_INSTALLED => {
            lifecycle_properties(install_channel())
        }
        event::CLI_COMPLETED => bounded_properties(&[
            ("command", Value::String("other".into())),
            ("tool_name", Value::String("not_applicable".into())),
            ("operation", Value::String("not_applicable".into())),
            ("computer_action", Value::Bool(false)),
            ("client_kind", Value::String("not_applicable".into())),
            ("success", Value::Bool(true)),
            ("exit_class", Value::String("success".into())),
            ("duration_bucket", Value::String("lt_100ms".into())),
        ]),
        event::MCP_SESSION_STARTED => bounded_properties(&[
            ("mcp_client", Value::String("unknown".into())),
            ("mcp_client_version_major", Value::String("unknown".into())),
            ("protocol_version", Value::String("unknown".into())),
            ("capability_tools", Value::Bool(false)),
            ("capability_roots", Value::Bool(false)),
            ("capability_sampling", Value::Bool(false)),
            ("capability_experimental", Value::Bool(false)),
            ("capability_elicitation_form", Value::Bool(false)),
            ("capability_elicitation_url", Value::Bool(false)),
            ("reported_provider", Value::String("unknown".into())),
            ("reported_model", Value::String("unknown".into())),
            ("reported_agent", Value::String("unknown".into())),
            (
                "reported_agent_version_major",
                Value::String("unknown".into()),
            ),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        event::MCP_TOOL_COMPLETED => bounded_properties(&[
            ("tool_name", Value::String("other".into())),
            ("operation", Value::String("not_applicable".into())),
            ("computer_action", Value::Bool(false)),
            ("success", Value::Bool(true)),
            ("error_class", Value::String("none".into())),
            ("refusal_code", Value::String("none".into())),
            ("duration_bucket", Value::String("lt_10ms".into())),
            ("output_type", Value::String("empty".into())),
            ("output_size_bucket", Value::String("0".into())),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        event::MCP_STARTUP_COMPLETED => bounded_properties(&[
            ("path", Value::String("daemon_proxy".into())),
            ("daemon", Value::String("already_running".into())),
            ("success", Value::Bool(true)),
            ("duration_bucket", Value::String("lt_100ms".into())),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        event::AGENT_SESSION_STARTED => bounded_properties(&[
            ("declaration", Value::String("start_session".into())),
            ("revived", Value::Bool(false)),
            ("concurrent_sessions_bucket", Value::String("1".into())),
            ("entry_transport", Value::String("mcp_stdio".into())),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        event::AGENT_SESSION_ENDED => bounded_properties(&[
            ("end_reason", Value::String("explicit".into())),
            ("duration_bucket", Value::String("lt_10s".into())),
            ("tool_count_bucket", Value::String("1_4".into())),
            ("computer_action_count_bucket", Value::String("1_4".into())),
            ("error_count_bucket", Value::String("0".into())),
            ("browser_refusal_count_bucket", Value::String("0".into())),
            ("had_successful_tool", Value::Bool(true)),
            ("had_successful_computer_action", Value::Bool(true)),
            ("used_page", Value::Bool(false)),
            ("used_browser", Value::Bool(true)),
            ("used_cursor_tools", Value::Bool(false)),
            ("used_recording", Value::Bool(false)),
            ("used_config_write", Value::Bool(false)),
            ("cursor_outcome_observed", Value::Bool(true)),
            ("cursor_overlay_enabled_at_end", Value::Bool(true)),
            ("cursor_style", Value::String("default".into())),
            (
                "cursor_color_source",
                Value::String("automatic_palette".into()),
            ),
            ("cursor_label_set", Value::Bool(false)),
            ("cursor_motion_customized", Value::Bool(false)),
            ("multi_cursor_bucket", Value::String("1".into())),
            ("observed_multiple_transports", Value::Bool(false)),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        event::PERMISSIONS_GATE_STARTED => bounded_properties(&[
            ("missing_accessibility", Value::Bool(true)),
            ("missing_screen_recording", Value::Bool(true)),
        ]),
        event::PERMISSIONS_GATE_DISMISSED => bounded_properties(&[
            ("missing_accessibility", Value::Bool(true)),
            ("missing_screen_recording", Value::Bool(true)),
            ("duration_bucket", Value::String("2s_9_999ms".into())),
        ]),
        event::PERMISSIONS_GATE_COMPLETED => bounded_properties(&[
            ("missing_accessibility", Value::Bool(true)),
            ("missing_screen_recording", Value::Bool(true)),
            ("panel_shown", Value::Bool(true)),
            ("dismissed", Value::Bool(false)),
            ("resolution", Value::String("granted".into())),
            ("duration_bucket", Value::String("2s_9_999ms".into())),
        ]),
        _ => Map::new(),
    };
    let transport = match event_name {
        event::MCP_SESSION_STARTED
        | event::MCP_TOOL_COMPLETED
        | event::MCP_STARTUP_COMPLETED
        | event::AGENT_SESSION_STARTED
        | event::AGENT_SESSION_ENDED => Transport::McpStdio,
        event::SERVE_START_LEGACY
        | event::PERMISSIONS_GATE_STARTED
        | event::PERMISSIONS_GATE_DISMISSED
        | event::PERMISSIONS_GATE_COMPLETED => Transport::Daemon,
        _ => Transport::Cli,
    };
    Ok(build_payload(event_name, &properties, &identity, transport))
}

/// Transitional fixed start events for long-running processes only.
pub fn capture_start(event_name: &'static str, transport: Transport) {
    if event_name != event::SERVE_START_LEGACY {
        return;
    }
    capture_bounded(event_name, Map::new(), transport);
}

pub fn register_stdio_observer() {
    let _ = cua_driver_core::server::set_stdio_observer(Arc::new(TelemetryObserver));
    let _ = cua_driver_core::session::set_session_observer(Arc::new(TelemetryObserver));
}

fn execution_mode() -> &'static str {
    if cua_driver_core::embedded_mode() {
        "embedded"
    } else {
        "standalone"
    }
}

struct TelemetryObserver;

impl cua_driver_core::server::StdioObserver for TelemetryObserver {
    fn on_session_started(&self, metadata: cua_driver_core::protocol::InitializeMetadata) {
        capture_mcp_session_started(metadata, Transport::McpStdio);
    }

    fn on_tool_completed(&self, outcome: cua_driver_core::server::ToolCompletionObservation) {
        capture_tool_completed(outcome, Transport::McpStdio);
    }
}

const MAX_TRACKED_AGENT_SESSIONS: usize = 256;
static AGENT_SESSIONS: OnceLock<Mutex<HashMap<String, AgentSessionState>>> = OnceLock::new();

struct AgentSessionState {
    started: std::time::Instant,
    entry_transport: Transport,
    transport_bits: u8,
    tool_count: u64,
    computer_action_count: u64,
    error_count: u64,
    browser_refusal_count: u64,
    had_successful_tool: bool,
    had_successful_computer_action: bool,
    used_page: bool,
    used_browser: bool,
    used_cursor_tools: bool,
    used_recording: bool,
    used_config_write: bool,
}

impl AgentSessionState {
    fn new(entry_transport: Transport) -> Self {
        Self {
            started: std::time::Instant::now(),
            entry_transport,
            transport_bits: transport_bit(entry_transport),
            tool_count: 0,
            computer_action_count: 0,
            error_count: 0,
            browser_refusal_count: 0,
            had_successful_tool: false,
            had_successful_computer_action: false,
            used_page: false,
            used_browser: false,
            used_cursor_tools: false,
            used_recording: false,
            used_config_write: false,
        }
    }

    fn observe(
        &mut self,
        transport: Transport,
        computer_action: bool,
        outcome: &cua_driver_core::server::ToolCompletionObservation,
    ) {
        self.transport_bits |= transport_bit(transport);
        let tool_name = outcome.tool_name.as_str();
        if matches!(tool_name, "start_session" | "end_session") {
            return;
        }
        let used_browser = matches!(
            tool_name,
            "get_browser_state"
                | "browser_prepare"
                | "browser_navigate"
                | "browser_click"
                | "browser_type"
                | "browser_dialog"
                | "browser_set_input_files"
                | "browser_download"
                | "browser_pointer"
        );
        let refused = outcome.refusal_code.is_refusal();
        let completed_computer_action = computer_action && !refused;
        self.tool_count = self.tool_count.saturating_add(1);
        self.computer_action_count = self
            .computer_action_count
            .saturating_add(u64::from(completed_computer_action));
        self.error_count = self.error_count.saturating_add(u64::from(!outcome.success));
        self.browser_refusal_count = self
            .browser_refusal_count
            .saturating_add(u64::from(used_browser && refused));
        self.had_successful_tool |= outcome.success;
        self.had_successful_computer_action |= outcome.success && completed_computer_action;
        self.used_page |= tool_name == "page";
        self.used_browser |= used_browser;
        self.used_cursor_tools |= matches!(
            tool_name,
            "set_agent_cursor_enabled"
                | "set_agent_cursor_motion"
                | "set_agent_cursor_style"
                | "get_agent_cursor_state"
        );
        self.used_recording |= matches!(
            tool_name,
            "start_recording" | "stop_recording" | "replay_trajectory"
        );
        self.used_config_write |= tool_name == "set_config";
    }

    fn ended_properties(
        &self,
        reason: cua_driver_core::session::SessionEndReason,
        cursor: Option<cua_driver_core::session::CursorOutcomeObservation>,
    ) -> Map<String, Value> {
        use cua_driver_core::session::{CursorColorSource, CursorStyleCategory, SessionEndReason};
        let end_reason = match reason {
            SessionEndReason::Explicit => "explicit",
            SessionEndReason::IdleTimeout => "idle_timeout",
            SessionEndReason::ProcessExit => "process_exit",
            SessionEndReason::Unknown => "unknown",
        };
        let cursor = cursor.unwrap_or(cua_driver_core::session::CursorOutcomeObservation {
            observed: false,
            enabled: false,
            style: CursorStyleCategory::Unknown,
            color_source: CursorColorSource::Unknown,
            label_set: false,
            motion_customized: false,
            active_cursor_count: 0,
        });
        let cursor_style = match cursor.style {
            CursorStyleCategory::Default => "default",
            CursorStyleCategory::BuiltinArrow => "builtin_arrow",
            CursorStyleCategory::BuiltinTeardrop => "builtin_teardrop",
            CursorStyleCategory::CustomIcon => "custom_icon",
            CursorStyleCategory::Unknown => "unknown",
        };
        let cursor_color_source = match cursor.color_source {
            CursorColorSource::AutomaticPalette => "automatic_palette",
            CursorColorSource::Custom => "custom",
            CursorColorSource::Unknown => "unknown",
        };
        bounded_properties(&[
            ("end_reason", Value::String(end_reason.into())),
            (
                "duration_bucket",
                Value::String(agent_session_duration_bucket(self.started.elapsed()).into()),
            ),
            (
                "tool_count_bucket",
                Value::String(agent_session_count_bucket(self.tool_count).into()),
            ),
            (
                "computer_action_count_bucket",
                Value::String(agent_session_count_bucket(self.computer_action_count).into()),
            ),
            (
                "error_count_bucket",
                Value::String(agent_session_error_bucket(self.error_count).into()),
            ),
            (
                "browser_refusal_count_bucket",
                Value::String(agent_session_error_bucket(self.browser_refusal_count).into()),
            ),
            ("had_successful_tool", Value::Bool(self.had_successful_tool)),
            (
                "had_successful_computer_action",
                Value::Bool(self.had_successful_computer_action),
            ),
            ("used_page", Value::Bool(self.used_page)),
            ("used_browser", Value::Bool(self.used_browser)),
            ("used_cursor_tools", Value::Bool(self.used_cursor_tools)),
            ("used_recording", Value::Bool(self.used_recording)),
            ("used_config_write", Value::Bool(self.used_config_write)),
            ("cursor_outcome_observed", Value::Bool(cursor.observed)),
            ("cursor_overlay_enabled_at_end", Value::Bool(cursor.enabled)),
            ("cursor_style", Value::String(cursor_style.into())),
            (
                "cursor_color_source",
                Value::String(cursor_color_source.into()),
            ),
            ("cursor_label_set", Value::Bool(cursor.label_set)),
            (
                "cursor_motion_customized",
                Value::Bool(cursor.motion_customized),
            ),
            (
                "multi_cursor_bucket",
                Value::String(cursor_count_bucket(cursor.active_cursor_count).into()),
            ),
            (
                "observed_multiple_transports",
                Value::Bool(self.transport_bits.count_ones() > 1),
            ),
            ("execution_mode", Value::String(execution_mode().into())),
        ])
    }
}

fn transport_bit(transport: Transport) -> u8 {
    match transport {
        Transport::Cli => 1,
        Transport::Daemon => 2,
        Transport::McpStdio => 4,
        Transport::McpHttp => 8,
    }
}

fn session_transport(transport: cua_driver_core::session::SessionTransport) -> Transport {
    match transport {
        cua_driver_core::session::SessionTransport::Cli => Transport::Cli,
        cua_driver_core::session::SessionTransport::Daemon => Transport::Daemon,
        cua_driver_core::session::SessionTransport::McpStdio => Transport::McpStdio,
        cua_driver_core::session::SessionTransport::McpHttp => Transport::McpHttp,
    }
}

fn concurrent_sessions_bucket(count: usize) -> &'static str {
    match count {
        0 | 1 => "1",
        2 => "2",
        3..=5 => "3_5",
        _ => "gte_6",
    }
}

fn cursor_count_bucket(count: usize) -> &'static str {
    match count {
        0 | 1 => "1",
        2 => "2",
        3..=5 => "3_5",
        _ => "gte_6",
    }
}

fn agent_session_duration_bucket(duration: Duration) -> &'static str {
    match duration.as_secs() {
        0..=9 => "lt_10s",
        10..=59 => "10_59s",
        60..=299 => "1_4min",
        300..=1799 => "5_29min",
        _ => "gte_30min",
    }
}

fn agent_session_count_bucket(count: u64) -> &'static str {
    match count {
        0 => "0",
        1..=4 => "1_4",
        5..=19 => "5_19",
        20..=99 => "20_99",
        _ => "gte_100",
    }
}

fn agent_session_error_bucket(count: u64) -> &'static str {
    match count {
        0 => "0",
        1 => "1",
        2..=4 => "2_4",
        _ => "gte_5",
    }
}

impl cua_driver_core::session::SessionObserver for TelemetryObserver {
    fn on_session_started(
        &self,
        session_id: &str,
        observation: cua_driver_core::session::SessionStartObservation,
    ) {
        let sessions = AGENT_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()));
        if !is_enabled() {
            sessions.lock().unwrap().remove(session_id);
            return;
        }
        let transport = session_transport(observation.transport);
        let concurrent = {
            let mut sessions = sessions.lock().unwrap();
            if let Some(existing) = sessions.get_mut(session_id) {
                existing.transport_bits |= transport_bit(transport);
                return;
            }
            if sessions.len() >= MAX_TRACKED_AGENT_SESSIONS {
                return;
            }
            sessions.insert(session_id.to_owned(), AgentSessionState::new(transport));
            sessions.len()
        };
        let declaration = match observation.declaration {
            cua_driver_core::session::SessionDeclaration::StartSession => "start_session",
            cua_driver_core::session::SessionDeclaration::ImplicitFirstAction => {
                "implicit_first_action"
            }
        };
        capture_bounded(
            event::AGENT_SESSION_STARTED,
            bounded_properties(&[
                ("declaration", Value::String(declaration.into())),
                ("revived", Value::Bool(observation.revived)),
                (
                    "concurrent_sessions_bucket",
                    Value::String(concurrent_sessions_bucket(concurrent).into()),
                ),
                ("entry_transport", Value::String(transport.as_str().into())),
                ("execution_mode", Value::String(execution_mode().into())),
            ]),
            transport,
        );
    }

    fn on_tool_completed(
        &self,
        session_id: &str,
        transport: cua_driver_core::session::SessionTransport,
        computer_action: bool,
        outcome: &cua_driver_core::server::ToolCompletionObservation,
    ) {
        let sessions = AGENT_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut sessions = sessions.lock().unwrap();
        if !is_enabled() {
            sessions.remove(session_id);
            return;
        }
        if let Some(state) = sessions.get_mut(session_id) {
            state.observe(session_transport(transport), computer_action, outcome);
        }
    }

    fn on_session_ended(
        &self,
        session_id: &str,
        reason: cua_driver_core::session::SessionEndReason,
        cursor: Option<cua_driver_core::session::CursorOutcomeObservation>,
    ) {
        let state = AGENT_SESSIONS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap()
            .remove(session_id);
        if !is_enabled() {
            return;
        }
        let Some(state) = state else {
            return;
        };
        let transport = state.entry_transport;
        capture_bounded(
            event::AGENT_SESSION_ENDED,
            state.ended_properties(reason, cursor),
            transport,
        );
    }
}

static MCP_SESSION_OBSERVED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn mcp_session_observation_key(transport: Transport, mcp_client: &str) -> String {
    match transport {
        Transport::McpHttp => format!("mcp_http:{mcp_client}"),
        _ => transport.as_str().to_owned(),
    }
}

pub(crate) fn capture_mcp_session_started(
    metadata: cua_driver_core::protocol::InitializeMetadata,
    transport: Transport,
) {
    if !is_enabled() {
        return;
    }
    let mcp_client = normalize_client(metadata.client_name.as_deref());
    let observation_key = mcp_session_observation_key(transport, &mcp_client);
    let observed = MCP_SESSION_OBSERVED.get_or_init(|| Mutex::new(HashSet::new()));
    if !observed.lock().unwrap().insert(observation_key) {
        return;
    }

    let context = metadata.reported_agent_context.unwrap_or_default();
    let capabilities = metadata.capability_flags;
    capture_bounded(
        event::MCP_SESSION_STARTED,
        bounded_properties(&[
            ("mcp_client", Value::String(mcp_client)),
            (
                "mcp_client_version_major",
                Value::String(version_major(metadata.client_version.as_deref())),
            ),
            (
                "protocol_version",
                Value::String(normalize_protocol(metadata.protocol_version.as_deref())),
            ),
            ("capability_tools", Value::Bool(capabilities.tools)),
            ("capability_roots", Value::Bool(capabilities.roots)),
            ("capability_sampling", Value::Bool(capabilities.sampling)),
            (
                "capability_experimental",
                Value::Bool(capabilities.experimental),
            ),
            (
                "capability_elicitation_form",
                Value::Bool(capabilities.elicitation_form),
            ),
            (
                "capability_elicitation_url",
                Value::Bool(capabilities.elicitation_url),
            ),
            (
                "reported_provider",
                Value::String(normalize_provider(context.provider.as_deref())),
            ),
            (
                "reported_model",
                Value::String(normalize_model(context.model.as_deref())),
            ),
            (
                "reported_agent",
                Value::String(normalize_client(context.agent_name.as_deref())),
            ),
            (
                "reported_agent_version_major",
                Value::String(version_major(context.agent_version.as_deref())),
            ),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        transport,
    );
}

pub(crate) fn capture_tool_completed(
    outcome: cua_driver_core::server::ToolCompletionObservation,
    transport: Transport,
) {
    let is_first_value_candidate =
        outcome.computer_action && outcome.success && !outcome.refusal_code.is_refusal();
    if !is_enabled() || !should_capture_tool_completion(Instant::now(), is_first_value_candidate) {
        return;
    }
    let mut properties = tool_completion_properties(outcome);
    properties.insert(
        "execution_mode".into(),
        Value::String(execution_mode().into()),
    );
    capture_bounded(event::MCP_TOOL_COMPLETED, properties, transport);
}

#[derive(Debug)]
struct ToolTelemetryRateLimit {
    window_started: Instant,
    captured: usize,
    captured_value_event: bool,
}

impl ToolTelemetryRateLimit {
    fn new(now: Instant) -> Self {
        Self {
            window_started: now,
            captured: 0,
            captured_value_event: false,
        }
    }

    fn allow(&mut self, now: Instant, is_value_event: bool) -> bool {
        if now.duration_since(self.window_started) >= TOOL_TELEMETRY_WINDOW {
            self.window_started = now;
            self.captured = 0;
            self.captured_value_event = false;
        }
        if self.captured >= TOOL_TELEMETRY_LIMIT_PER_HOUR {
            if !is_value_event || self.captured_value_event {
                return false;
            }
            self.captured_value_event = true;
            return true;
        }
        self.captured += 1;
        self.captured_value_event |= is_value_event;
        true
    }
}

static TOOL_TELEMETRY_RATE_LIMIT: OnceLock<Mutex<ToolTelemetryRateLimit>> = OnceLock::new();

fn should_capture_tool_completion(now: Instant, is_value_event: bool) -> bool {
    TOOL_TELEMETRY_RATE_LIMIT
        .get_or_init(|| Mutex::new(ToolTelemetryRateLimit::new(now)))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .allow(now, is_value_event)
}

fn tool_completion_properties(
    outcome: cua_driver_core::server::ToolCompletionObservation,
) -> Map<String, Value> {
    use cua_driver_core::server::{DurationBucket, OutputSizeBucket, OutputType, ToolErrorClass};
    let tool_name = if matches!(
        outcome.error_class,
        ToolErrorClass::UnknownTool | ToolErrorClass::InvalidParams
    ) {
        "other".to_owned()
    } else if outcome.tool_name == "type_text_chars" {
        "type_text".to_owned()
    } else {
        outcome.tool_name
    };
    let error_class = match outcome.error_class {
        ToolErrorClass::None => "none",
        ToolErrorClass::InvalidParams => "invalid_params",
        ToolErrorClass::UnknownTool => "unknown_tool",
        ToolErrorClass::PermissionDenied => "permission_denied",
        ToolErrorClass::BackgroundUnavailable => "background_unavailable",
        ToolErrorClass::TransportError => "transport_error",
        ToolErrorClass::InternalError => "internal_error",
    };
    let duration_bucket = match outcome.duration_bucket {
        DurationBucket::Under10Ms => "lt_10ms",
        DurationBucket::Ms10To49 => "10_49ms",
        DurationBucket::Ms50To249 => "50_249ms",
        DurationBucket::Ms250To999 => "250_999ms",
        DurationBucket::Ms1000To4999 => "1_4s",
        DurationBucket::Ms5000OrMore => "gte_5s",
    };
    let output_type = match outcome.output_type {
        OutputType::Empty => "empty",
        OutputType::Text => "text",
        OutputType::Image => "image",
        OutputType::Mixed => "mixed",
        OutputType::Unknown => "unknown",
    };
    let output_size_bucket = match outcome.output_size_bucket {
        OutputSizeBucket::Empty => "0",
        OutputSizeBucket::Under1KiB => "lt_1kib",
        OutputSizeBucket::KiB1To9 => "1_9kib",
        OutputSizeBucket::KiB10To99 => "10_99kib",
        OutputSizeBucket::KiB100To1023 => "100_1023kib",
        OutputSizeBucket::MiB1OrMore => "gte_1mib",
    };
    bounded_properties(&[
        ("tool_name", Value::String(tool_name)),
        (
            "operation",
            Value::String(outcome.operation.as_str().into()),
        ),
        ("computer_action", Value::Bool(outcome.computer_action)),
        ("success", Value::Bool(outcome.success)),
        ("error_class", Value::String(error_class.into())),
        (
            "refusal_code",
            Value::String(outcome.refusal_code.as_str().into()),
        ),
        ("duration_bucket", Value::String(duration_bucket.into())),
        ("output_type", Value::String(output_type.into())),
        (
            "output_size_bucket",
            Value::String(output_size_bucket.into()),
        ),
    ])
}

pub(crate) fn capture_mcp_startup_completed(
    path: &'static str,
    daemon: &'static str,
    success: bool,
    elapsed: Duration,
) {
    let path = match path {
        "daemon_proxy" => "daemon_proxy",
        _ => "unknown",
    };
    let daemon = match daemon {
        "not_applicable" => "not_applicable",
        "already_running" => "already_running",
        "launched" => "launched",
        "launch_failed" => "launch_failed",
        "launch_timeout" => "launch_timeout",
        "unreachable" => "unreachable",
        "unsupported_relaunch" => "unsupported_relaunch",
        _ => "unknown",
    };
    capture_bounded(
        event::MCP_STARTUP_COMPLETED,
        bounded_properties(&[
            ("path", Value::String(path.into())),
            ("daemon", Value::String(daemon.into())),
            ("success", Value::Bool(success)),
            (
                "duration_bucket",
                Value::String(duration_bucket(elapsed).into()),
            ),
            ("execution_mode", Value::String(execution_mode().into())),
        ]),
        Transport::McpStdio,
    );
}

pub(crate) fn capture_permissions_gate_completed(
    missing_accessibility: bool,
    missing_screen_recording: bool,
    panel_shown: bool,
    dismissed: bool,
    resolution: &'static str,
    elapsed: Duration,
) {
    let resolution = match resolution {
        "granted" => "granted",
        "dismissed_then_granted" => "dismissed_then_granted",
        "timeout" => "timeout",
        _ => "unknown",
    };
    capture_bounded(
        event::PERMISSIONS_GATE_COMPLETED,
        bounded_properties(&[
            ("missing_accessibility", Value::Bool(missing_accessibility)),
            (
                "missing_screen_recording",
                Value::Bool(missing_screen_recording),
            ),
            ("panel_shown", Value::Bool(panel_shown)),
            ("dismissed", Value::Bool(dismissed)),
            ("resolution", Value::String(resolution.into())),
            (
                "duration_bucket",
                Value::String(duration_bucket(elapsed).into()),
            ),
        ]),
        Transport::Daemon,
    );
}

pub(crate) const fn permissions_gate_resolution(
    gate_failed: bool,
    dismissed: bool,
) -> &'static str {
    if gate_failed {
        "timeout"
    } else if dismissed {
        "dismissed_then_granted"
    } else {
        "granted"
    }
}

pub(crate) fn capture_permissions_gate_started(
    missing_accessibility: bool,
    missing_screen_recording: bool,
) {
    capture_bounded(
        event::PERMISSIONS_GATE_STARTED,
        bounded_properties(&[
            ("missing_accessibility", Value::Bool(missing_accessibility)),
            (
                "missing_screen_recording",
                Value::Bool(missing_screen_recording),
            ),
        ]),
        Transport::Daemon,
    );
}

pub(crate) fn capture_permissions_gate_dismissed(
    missing_accessibility: bool,
    missing_screen_recording: bool,
    elapsed: Duration,
) {
    capture_bounded(
        event::PERMISSIONS_GATE_DISMISSED,
        bounded_properties(&[
            ("missing_accessibility", Value::Bool(missing_accessibility)),
            (
                "missing_screen_recording",
                Value::Bool(missing_screen_recording),
            ),
            (
                "duration_bucket",
                Value::String(duration_bucket(elapsed).into()),
            ),
        ]),
        Transport::Daemon,
    );
}

fn normalize_client(value: Option<&str>) -> String {
    let normalized = value.unwrap_or("").trim().to_ascii_lowercase();
    for (needle, canonical) in [
        ("claude", "claude_code"),
        ("codex", "codex"),
        ("cursor", "cursor"),
        ("windsurf", "windsurf"),
        ("vscode", "vscode"),
        ("visual studio code", "vscode"),
        ("zed", "zed"),
        ("openclaw", "openclaw"),
        ("opencode", "opencode"),
        ("hermes", "hermes"),
    ] {
        if normalized.contains(needle) {
            return canonical.into();
        }
    }
    if normalized == "pi" || normalized.starts_with("pi/") || normalized.starts_with("pi ") {
        return "pi".into();
    }
    if normalized.is_empty() {
        "unknown".into()
    } else {
        "other".into()
    }
}

fn normalize_provider(value: Option<&str>) -> String {
    match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "anthropic" => "anthropic".into(),
        "openai" => "openai".into(),
        "google" | "google-ai" | "google_vertex" => "google".into(),
        "xai" | "x.ai" => "xai".into(),
        "amazon" | "aws" | "bedrock" => "amazon".into(),
        "microsoft" | "azure" | "azure-openai" => "microsoft".into(),
        "" => "unknown".into(),
        _ => "custom".into(),
    }
}

fn normalize_model(value: Option<&str>) -> String {
    let raw = value.unwrap_or("").trim().to_ascii_lowercase();
    if raw.is_empty() {
        return "unknown".into();
    }
    let category = if raw.starts_with("claude-") {
        if raw.contains("opus") {
            "claude_opus"
        } else if raw.contains("sonnet") {
            "claude_sonnet"
        } else if raw.contains("haiku") {
            "claude_haiku"
        } else {
            "claude_other"
        }
    } else if raw.starts_with("gpt-5") {
        "gpt_5"
    } else if raw.starts_with("gpt-4.1") {
        "gpt_4_1"
    } else if raw.starts_with("gpt-4o") {
        "gpt_4o"
    } else if raw.starts_with("gpt-4") {
        "gpt_4"
    } else if raw == "o1" || raw.starts_with("o1-") {
        "openai_o1"
    } else if raw == "o3" || raw.starts_with("o3-") {
        "openai_o3"
    } else if raw == "o4" || raw.starts_with("o4-") {
        "openai_o4"
    } else if raw.starts_with("gemini-") {
        if raw.contains("flash") {
            "gemini_flash"
        } else if raw.contains("pro") {
            "gemini_pro"
        } else {
            "gemini_other"
        }
    } else if raw.starts_with("grok-") {
        "grok"
    } else if raw.starts_with("nova-") {
        "amazon_nova"
    } else if raw.starts_with("phi-") {
        "microsoft_phi"
    } else {
        "custom"
    };
    category.into()
}

fn normalize_protocol(value: Option<&str>) -> String {
    match value {
        Some(value @ ("2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25")) => {
            value.to_owned()
        }
        _ => "unknown".into(),
    }
}

fn version_major(value: Option<&str>) -> String {
    let major = value
        .unwrap_or("")
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .unwrap_or("unknown");
    if major.len() <= 4 {
        major.to_owned()
    } else {
        "unknown".into()
    }
}

pub fn ensure_first_run_registration() {
    if !is_enabled() || lifecycle_is_current() || lifecycle_retry_deferred() {
        return;
    }
    let Some(home) = telemetry_home_dir() else {
        return;
    };
    let Some(_lifecycle_lock) = try_lifecycle_lock(&home) else {
        return;
    };
    if !is_enabled() || lifecycle_retry_deferred() {
        return;
    }
    if lifecycle_is_current() {
        let _ = remove_file_if_exists(&home.join(TELEMETRY_INSTALL_CHANNEL_FILE_NAME));
        return;
    }
    eprintln!(
        "Cua Driver sends content-free product telemetry by default. Run `cua-driver telemetry disable` to stop it; `cua-driver telemetry status` shows the current setting."
    );
    capture_install_locked(&home, &mut post_to_posthog);
}

/// Run lifecycle delivery in a hidden worker before CLI parsing. The normal
/// process only spawns this worker, so a blackholed telemetry endpoint cannot
/// delay MCP initialization, daemon startup, or finite CLI commands.
pub(crate) fn run_lifecycle_worker_if_requested() -> bool {
    if !parse_env_bool(ENV_LIFECYCLE_WORKER).unwrap_or(false) {
        return false;
    }
    ensure_first_run_registration();
    true
}

pub(crate) fn spawn_first_run_registration_worker() {
    if !is_enabled() || lifecycle_is_current() || lifecycle_retry_deferred() {
        return;
    }
    eprintln!(
        "Cua Driver sends content-free product telemetry by default. Run `cua-driver telemetry disable` to stop it; `cua-driver telemetry status` shows the current setting."
    );
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(executable)
        .env(ENV_LIFECYCLE_WORKER, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn capture_install() {
    capture_install_with_poster(post_to_posthog);
}

fn capture_install_with_poster<F>(mut post: F)
where
    F: FnMut(&Value) -> Result<u16, String>,
{
    if !is_enabled() || lifecycle_retry_deferred() {
        return;
    }
    let Some(home) = telemetry_home_dir() else {
        return;
    };
    let Some(_lifecycle_lock) = try_lifecycle_lock(&home) else {
        return;
    };
    if !is_enabled() || lifecycle_is_current() || lifecycle_retry_deferred() {
        return;
    }
    capture_install_locked(&home, &mut post);
}

fn capture_install_locked<F>(home: &Path, post: &mut F)
where
    F: FnMut(&Value) -> Result<u16, String>,
{
    let Some(identity) = get_or_create_install_id() else {
        return;
    };
    let channel = install_channel();
    let registration_marker = home.join(INSTALLATION_RECORDED_FILE_NAME);

    if !registration_marker.exists() {
        let payload = build_payload(
            event::INSTALLATION_REGISTERED,
            &lifecycle_properties(channel),
            &identity,
            Transport::Cli,
        );
        if post_success(post, &payload, event::INSTALLATION_REGISTERED) {
            if write_marker(&registration_marker).is_err() {
                defer_lifecycle_retry(home);
                return;
            }
        } else {
            defer_lifecycle_retry(home);
            return;
        }
    }

    let version = release_version();
    let Some(release_marker) = release_marker_path(&version) else {
        return;
    };
    if release_marker.exists() {
        return;
    }
    let payload = build_payload(
        event::RELEASE_INSTALLED,
        &lifecycle_properties(channel),
        &identity,
        Transport::Cli,
    );
    if post_success(post, &payload, event::RELEASE_INSTALLED) {
        if write_marker(&release_marker).is_ok() {
            let _ = remove_file_if_exists(&home.join(TELEMETRY_RETRY_AFTER_FILE_NAME));
            let _ = remove_file_if_exists(&home.join(TELEMETRY_INSTALL_CHANNEL_FILE_NAME));
        } else {
            defer_lifecycle_retry(home);
        }
    } else {
        defer_lifecycle_retry(home);
    }
}

fn lifecycle_is_current() -> bool {
    let registered = marker_path(INSTALLATION_RECORDED_FILE_NAME).is_some_and(|path| path.exists());
    let release = release_marker_path(&release_version()).is_some_and(|path| path.exists());
    registered && release
}

fn lifecycle_retry_deferred() -> bool {
    let Some(path) = marker_path(TELEMETRY_RETRY_AFTER_FILE_NAME) else {
        return false;
    };
    let Some(retry_after) = std::fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
    else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(retry_after);
    retry_after > now && retry_after.saturating_sub(now) <= LIFECYCLE_RETRY_BACKOFF_SECS
}

fn defer_lifecycle_retry(home: &Path) {
    let retry_after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| {
            duration
                .as_secs()
                .saturating_add(LIFECYCLE_RETRY_BACKOFF_SECS)
        });
    if let Ok(retry_after) = retry_after {
        let _ = std::fs::write(
            home.join(TELEMETRY_RETRY_AFTER_FILE_NAME),
            retry_after.to_string(),
        );
    }
}

fn post_success<F>(post: &mut F, payload: &Value, event_name: &str) -> bool
where
    F: FnMut(&Value) -> Result<u16, String>,
{
    match post(payload) {
        Ok(status) if (200..300).contains(&status) => true,
        Ok(status) => {
            debug_log(format_args!(
                "{event_name} returned {status}; marker not written"
            ));
            false
        }
        Err(error) => {
            debug_log(format_args!(
                "{event_name} failed: {error}; marker not written"
            ));
            false
        }
    }
}

pub(crate) fn capture_bounded(
    event_name: &'static str,
    properties: Map<String, Value>,
    transport: Transport,
) {
    if !is_enabled() {
        return;
    }
    let Some(identity) = get_or_create_install_id() else {
        return;
    };
    let payload = build_payload(event_name, &properties, &identity, transport);
    spawn_payload(event_name, payload);
}

/// Record a finite CLI command only after its outcome is known. This is
/// synchronous with a short timeout because one-shot processes may exit before
/// a detached request is delivered.
pub(crate) fn capture_cli_completed(
    command: &'static str,
    tool_name: Option<&str>,
    computer_action: bool,
    operation: &str,
    client_kind: &str,
    exit_code: i32,
    elapsed: Duration,
) {
    if !is_enabled() {
        return;
    }
    let command = match command {
        "list_tools" => "list_tools",
        "describe" => "describe",
        "mcp_config" => "mcp_config",
        "manifest" => "manifest",
        "call" => "call",
        "stop" => "stop",
        "status" => "status",
        "recording" => "recording",
        "dump_docs" => "dump_docs",
        "update" => "update",
        "check_update" => "check_update",
        "doctor" => "doctor",
        "diagnose" => "diagnose",
        "permissions" => "permissions",
        "autostart" => "autostart",
        "skills" => "skills",
        "config" => "config",
        _ => "other",
    };
    let success = exit_code == 0;
    let tool_name = if command == "call" {
        fixed_tool_name(tool_name.unwrap_or(""))
    } else {
        "not_applicable".into()
    };
    let operation = fixed_cli_operation(command, operation);
    let client_kind = fixed_cli_client_kind(command, client_kind);
    let exit_class = match exit_code {
        0 => "success",
        1 => "tool_error",
        64 => "invalid_input",
        _ => "other",
    };
    let Some(identity) = get_or_create_install_id() else {
        return;
    };
    let payload = build_payload(
        event::CLI_COMPLETED,
        &bounded_properties(&[
            ("command", Value::String(command.into())),
            ("tool_name", Value::String(tool_name.into())),
            ("operation", Value::String(operation.into())),
            (
                "computer_action",
                Value::Bool(command == "call" && computer_action),
            ),
            ("client_kind", Value::String(client_kind.into())),
            ("success", Value::Bool(success)),
            ("exit_class", Value::String(exit_class.into())),
            (
                "duration_bucket",
                Value::String(duration_bucket(elapsed).into()),
            ),
        ]),
        &identity,
        Transport::Cli,
    );
    if let Err(error) =
        post_to_posthog_with_timeout(&payload, Duration::from_secs(POSTHOG_TIMEOUT_SECS))
    {
        debug_log(format_args!("{} failed: {error}", event::CLI_COMPLETED));
    }
}

pub(crate) fn is_wrapped_cli_child() -> bool {
    parse_env_bool(ENV_CLI_WRAPPED_CHILD).unwrap_or(false)
}

pub(crate) fn cli_wrapped_child_env() -> &'static str {
    ENV_CLI_WRAPPED_CHILD
}

/// Run the hidden delivery worker before CLI parsing. The foreground parent
/// never waits on this process, so telemetry delivery cannot add network
/// latency to one-shot commands.
pub(crate) fn run_cli_completion_worker_if_requested() -> bool {
    if !parse_env_bool(ENV_CLI_COMPLETION_WORKER).unwrap_or(false) {
        return false;
    }
    let command = std::env::var(ENV_CLI_COMPLETION_COMMAND).unwrap_or_default();
    let exit_code = std::env::var(ENV_CLI_COMPLETION_EXIT_CODE)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(1);
    let elapsed = std::env::var(ENV_CLI_COMPLETION_DURATION_MS)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default();
    let command = fixed_cli_command(&command);
    let tool_name = std::env::var(ENV_CLI_COMPLETION_TOOL).ok();
    let computer_action = parse_env_bool(ENV_CLI_COMPLETION_COMPUTER_ACTION).unwrap_or(false);
    let operation = std::env::var(ENV_CLI_COMPLETION_OPERATION).unwrap_or_default();
    let client_kind = std::env::var(ENV_CLI_COMPLETION_CLIENT_KIND).unwrap_or_default();
    capture_cli_completed(
        command,
        tool_name.as_deref(),
        computer_action,
        &operation,
        &client_kind,
        exit_code,
        elapsed,
    );
    true
}

pub(crate) fn spawn_cli_completion_worker(
    command: &'static str,
    tool_name: Option<&str>,
    computer_action: bool,
    operation: &str,
    client_kind: &str,
    exit_code: i32,
    elapsed: Duration,
) {
    if !is_enabled() {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let mut worker = std::process::Command::new(executable);
    worker
        .env(ENV_CLI_COMPLETION_WORKER, "1")
        .env(ENV_CLI_COMPLETION_COMMAND, fixed_cli_command(command))
        .env(
            ENV_CLI_COMPLETION_COMPUTER_ACTION,
            if computer_action { "1" } else { "0" },
        )
        .env(
            ENV_CLI_COMPLETION_OPERATION,
            fixed_cli_operation(command, operation),
        )
        .env(
            ENV_CLI_COMPLETION_CLIENT_KIND,
            fixed_cli_client_kind(command, client_kind),
        )
        .env(ENV_CLI_COMPLETION_EXIT_CODE, exit_code.to_string())
        .env(
            ENV_CLI_COMPLETION_DURATION_MS,
            elapsed.as_millis().to_string(),
        );
    if let Some(tool_name) = tool_name {
        worker.env(ENV_CLI_COMPLETION_TOOL, fixed_tool_name(tool_name));
    }
    let _ = worker
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn fixed_cli_command(command: &str) -> &'static str {
    match command {
        "list_tools" => "list_tools",
        "describe" => "describe",
        "mcp_config" => "mcp_config",
        "manifest" => "manifest",
        "call" => "call",
        "stop" => "stop",
        "status" => "status",
        "recording" => "recording",
        "dump_docs" => "dump_docs",
        "update" => "update",
        "check_update" => "check_update",
        "doctor" => "doctor",
        "diagnose" => "diagnose",
        "permissions" => "permissions",
        "autostart" => "autostart",
        "skills" => "skills",
        "config" => "config",
        _ => "other",
    }
}

fn fixed_tool_name(tool_name: &str) -> String {
    if tool_name == "type_text_chars" {
        return "type_text".into();
    }
    if cua_driver_core::tool::default_capabilities_for(tool_name).is_empty() {
        "other".into()
    } else {
        // Every canonical tool is asserted by cua-driver-core tests to have a
        // capability entry. Returning the original value is therefore safe:
        // arbitrary argv can reach this branch only when it matches that
        // compile-time registry vocabulary exactly.
        tool_name.to_owned()
    }
}

fn fixed_cli_operation(command: &str, operation: &str) -> &'static str {
    match command {
        "recording" => match operation {
            "start" => "start",
            "stop" => "stop",
            "status" => "status",
            "render" => "render",
            _ => "other",
        },
        "permissions" => match operation {
            "status" => "status",
            "grant" => "grant",
            _ => "other",
        },
        "config" => match operation {
            "show" => "show",
            "get" => "get",
            "set" => "set",
            "reset" => "reset",
            _ => "other",
        },
        "autostart" => match operation {
            "enable" => "enable",
            "disable" => "disable",
            "status" => "status",
            "kick" => "kick",
            _ => "other",
        },
        "skills" => match operation {
            "install" => "install",
            "update" => "update",
            "uninstall" => "uninstall",
            "status" => "status",
            "path" => "path",
            _ => "other",
        },
        "update" => match operation {
            "apply" => "apply",
            "check_only" => "check_only",
            _ => "other",
        },
        _ => "not_applicable",
    }
}

fn fixed_cli_client_kind(command: &str, client_kind: &str) -> &'static str {
    if command != "mcp_config" {
        return "not_applicable";
    }
    match client_kind {
        "generic" => "generic",
        "claude_code" => "claude_code",
        "codex" => "codex",
        "cursor" => "cursor",
        "openclaw" => "openclaw",
        "opencode" => "opencode",
        "hermes" => "hermes",
        "pi" => "pi",
        "antigravity" => "antigravity",
        "qwen_code" => "qwen_code",
        "factory_droid" => "factory_droid",
        "zcode" => "zcode",
        _ => "other",
    }
}

pub(crate) fn bounded_properties(entries: &[(&str, Value)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

fn lifecycle_properties(channel: &'static str) -> Map<String, Value> {
    bounded_properties(&[
        ("install_channel", Value::String(channel.to_owned())),
        ("product_version", Value::String(release_version())),
    ])
}

fn build_payload(
    event_name: &str,
    properties: &Map<String, Value>,
    identity: &InstallationIdentity,
    transport: Transport,
) -> Value {
    let mut event_properties = properties.clone();
    event_properties.insert("telemetry_schema_version".into(), Value::from(3));
    event_properties
        .entry("product_version")
        .or_insert_with(|| Value::String(current_product_version().into()));
    event_properties.insert("os_family".into(), Value::String(os_family().into()));
    event_properties.insert("os_major".into(), Value::String(os_major()));
    event_properties.insert("arch".into(), Value::String(arch().into()));
    event_properties.insert("is_ci".into(), Value::Bool(is_ci()));
    event_properties.insert(
        "is_synthetic".into(),
        Value::Bool(parse_env_bool(ENV_TELEMETRY_SYNTHETIC).unwrap_or(false)),
    );
    event_properties.insert("transport".into(), Value::String(transport.as_str().into()));
    event_properties.insert(
        "process_session_id".into(),
        Value::String(process_session_id()),
    );
    event_properties.insert("id_persisted".into(), Value::Bool(identity.persisted));
    event_properties.insert("$process_person_profile".into(), Value::Bool(false));
    event_properties.insert("$lib".into(), Value::String("cua-driver-rs".into()));
    event_properties.insert(
        "$lib_version".into(),
        Value::String(current_product_version().into()),
    );

    serde_json::json!({
        "api_key": POSTHOG_API_KEY,
        "event": event_name,
        "distinct_id": identity.id,
        "properties": event_properties,
    })
}

fn spawn_payload(event_name: &'static str, payload: Value) {
    PENDING_SENDS.fetch_add(1, Ordering::SeqCst);
    let task = move || {
        let _pending = PendingSendGuard;
        if let Err(error) = post_to_posthog(&payload) {
            debug_log(format_args!("{event_name} failed: {error}"));
        }
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(task);
    } else {
        if std::thread::Builder::new()
            .name("cua-telemetry".into())
            .spawn(task)
            .is_err()
        {
            finish_pending_send();
        }
    }
}

static PENDING_SENDS: AtomicUsize = AtomicUsize::new(0);
static PENDING_WAIT: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();

struct PendingSendGuard;

impl Drop for PendingSendGuard {
    fn drop(&mut self) {
        finish_pending_send();
    }
}

fn finish_pending_send() {
    if let Some((lock, ready)) = PENDING_WAIT.get() {
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        PENDING_SENDS.fetch_sub(1, Ordering::SeqCst);
        ready.notify_all();
    } else {
        PENDING_SENDS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Give already-enqueued telemetry a bounded chance to finish when a
/// long-running MCP process is about to force-exit.
pub(crate) fn flush_pending(timeout: Duration) {
    if PENDING_SENDS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let wait = PENDING_WAIT.get_or_init(|| (Mutex::new(()), Condvar::new()));
    let deadline = std::time::Instant::now() + timeout;
    let mut guard = wait
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while PENDING_SENDS.load(Ordering::SeqCst) > 0 {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let (next, result) = wait
            .1
            .wait_timeout(guard, deadline - now)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard = next;
        if result.timed_out() {
            break;
        }
    }
}

fn post_to_posthog(payload: &Value) -> Result<u16, String> {
    post_to_posthog_with_timeout(payload, Duration::from_secs(POSTHOG_TIMEOUT_SECS))
}

fn post_to_posthog_with_timeout(payload: &Value, timeout: Duration) -> Result<u16, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .new_agent();
    match agent
        .post(POSTHOG_CAPTURE_URL)
        .header("Content-Type", "application/json")
        .send_json(payload)
    {
        Ok(response) => Ok(response.status().as_u16()),
        Err(error) => Err(error.to_string()),
    }
}

fn telemetry_home_dir() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(ENV_TELEMETRY_HOME) {
        return Some(PathBuf::from(path));
    }
    home_root().map(|home| home.join(HOME_SUBDIRECTORY))
}

fn home_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn config_path() -> Option<PathBuf> {
    telemetry_home_dir().map(|home| home.join(CONFIG_FILE_NAME))
}

fn marker_path(name: &str) -> Option<PathBuf> {
    telemetry_home_dir().map(|home| home.join(name))
}

fn release_marker_path(version: &str) -> Option<PathBuf> {
    let safe: String = version
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
        .take(80)
        .collect();
    telemetry_home_dir().map(|home| home.join(RELEASE_RECORDED_DIRECTORY).join(safe))
}

fn read_config() -> Value {
    config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn persisted_enabled() -> Option<bool> {
    read_config()
        .get(CONFIG_ENABLED_KEY)
        .and_then(Value::as_bool)
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid config path".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, json)
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| format!("failed to replace {}: {error}", path.display()))?;
    }
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("failed to replace {}: {error}", path.display()))
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to remove {}: {error}", path.display())),
    }
}

fn write_marker(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, "1")
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn open_lock_file(home: &Path, name: &str) -> Result<File, String> {
    std::fs::create_dir_all(home)
        .map_err(|error| format!("failed to create {}: {error}", home.display()))?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(home.join(name))
        .map_err(|error| format!("failed to open telemetry state lock: {error}"))
}

fn try_lifecycle_lock(home: &Path) -> Option<File> {
    let file = match open_lock_file(home, TELEMETRY_LIFECYCLE_LOCK_FILE_NAME) {
        Ok(file) => file,
        Err(error) => {
            debug_log(format_args!("lifecycle lock unavailable: {error}"));
            return None;
        }
    };
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(TryLockError::WouldBlock) => {
            debug_log(format_args!("lifecycle registration already in progress"));
            None
        }
        Err(TryLockError::Error(error)) => {
            debug_log(format_args!("failed to lock telemetry lifecycle: {error}"));
            None
        }
    }
}

fn migrate_legacy_telemetry_home() {
    if std::env::var_os(ENV_TELEMETRY_HOME).is_some() {
        return;
    }
    let Some(root) = home_root() else {
        return;
    };
    let legacy = root.join(LEGACY_HOME_SUBDIRECTORY);
    let current = root.join(HOME_SUBDIRECTORY);
    if !legacy.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(&current);
    let Ok(identity_lock) = open_lock_file(&current, TELEMETRY_IDENTITY_LOCK_FILE_NAME) else {
        return;
    };
    if identity_lock.lock().is_err() {
        return;
    }
    for name in [TELEMETRY_ID_FILE_NAME, INSTALLATION_RECORDED_FILE_NAME] {
        let source = legacy.join(name);
        let destination = current.join(name);
        if source.exists() && !destination.exists() {
            let _ = std::fs::rename(&source, &destination);
        }
    }
    let _ = std::fs::remove_dir(&legacy);
}

fn read_install_id() -> Option<String> {
    migrate_legacy_telemetry_home();
    let path = marker_path(TELEMETRY_ID_FILE_NAME)?;
    read_install_id_path(&path)
}

fn read_install_id_path(path: &Path) -> Option<String> {
    let value = std::fs::read_to_string(path).ok()?;
    let trimmed = value.trim();
    if uuid::Uuid::parse_str(trimmed).is_ok() {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

fn get_or_create_install_id() -> Option<InstallationIdentity> {
    load_or_create_install_id_uncached()
}

fn load_or_create_install_id_uncached() -> Option<InstallationIdentity> {
    if let Some(id) = read_install_id() {
        return Some(InstallationIdentity {
            id,
            persisted: true,
        });
    }
    let Some(path) = marker_path(TELEMETRY_ID_FILE_NAME) else {
        static EPHEMERAL_ID: OnceLock<String> = OnceLock::new();
        return Some(InstallationIdentity {
            id: EPHEMERAL_ID
                .get_or_init(|| uuid::Uuid::new_v4().to_string())
                .clone(),
            persisted: false,
        });
    };
    let candidate = uuid::Uuid::new_v4().to_string();
    match persist_install_id_if_absent(&path, &candidate) {
        Ok(id) => Some(InstallationIdentity {
            id,
            persisted: true,
        }),
        Err(error) => {
            debug_log(format_args!("installation identity unavailable: {error}"));
            None
        }
    }
}

fn persist_install_id_if_absent(path: &Path, candidate: &str) -> Result<String, String> {
    let home = path
        .parent()
        .ok_or_else(|| "invalid telemetry identity path".to_owned())?;
    let identity_lock = open_lock_file(home, TELEMETRY_IDENTITY_LOCK_FILE_NAME)?;
    identity_lock
        .lock()
        .map_err(|error| format!("failed to lock telemetry identity: {error}"))?;

    if let Some(id) = read_install_id_path(path) {
        return Ok(id);
    }

    let temporary = home.join(format!(".telemetry-id-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create(&temporary)
            .map_err(|error| format!("failed to write telemetry identity: {error}"))?;
        file.write_all(candidate.as_bytes())
            .map_err(|error| format!("failed to write telemetry identity: {error}"))?;
        file.sync_data()
            .map_err(|error| format!("failed to sync telemetry identity: {error}"))?;

        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path)
                .map_err(|error| format!("failed to replace telemetry identity: {error}"))?;
        }
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("failed to persist telemetry identity: {error}"))?;
        Ok(candidate.to_owned())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn redact_id(id: &str) -> String {
    format!("{}…", id.chars().take(8).collect::<String>())
}

fn current_product_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn release_version() -> String {
    std::env::var(ENV_RELEASE_VERSION)
        .ok()
        .and_then(|value| strict_release_version(&value))
        .unwrap_or_else(|| current_product_version().to_owned())
}

fn strict_release_version(raw: &str) -> Option<String> {
    let value = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
    if value.is_empty() || value.len() > 64 || value.contains('+') {
        return None;
    }
    let (core, prerelease) = value
        .split_once('-')
        .map_or((value, None), |(core, suffix)| (core, Some(suffix)));
    let components: Vec<&str> = core.split('.').collect();
    if components.len() != 3
        || components.iter().any(|part| {
            part.is_empty() || part.len() > 6 || !part.chars().all(|c| c.is_ascii_digit())
        })
    {
        return None;
    }
    if prerelease.is_some_and(|suffix| {
        suffix.is_empty()
            || suffix.len() > 32
            || suffix.split('.').any(|part| part.is_empty())
            || !suffix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    }) {
        return None;
    }
    Some(value.to_owned())
}

fn duration_bucket(elapsed: Duration) -> &'static str {
    match elapsed.as_millis() {
        0..=99 => "lt_100ms",
        100..=499 => "100_499ms",
        500..=1_999 => "500ms_1_999ms",
        2_000..=9_999 => "2s_9_999ms",
        _ => "gte_10s",
    }
}

fn install_channel() -> &'static str {
    std::env::var(ENV_INSTALL_CHANNEL)
        .ok()
        .and_then(|value| normalize_install_channel(&value))
        .or_else(|| {
            marker_path(TELEMETRY_INSTALL_CHANNEL_FILE_NAME)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|value| normalize_install_channel(&value))
        })
        .unwrap_or("first_run")
}

fn normalize_install_channel(value: &str) -> Option<&'static str> {
    match value.trim() {
        "install_script" => Some("install_script"),
        "update_apply" => Some("update_apply"),
        "python_package" => Some("python_package"),
        "manual_binary" => Some("manual_binary"),
        "first_run" => Some("first_run"),
        _ => None,
    }
}

fn process_session_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| uuid::Uuid::new_v4().to_string()).clone()
}

fn parse_env_bool(name: &str) -> Option<bool> {
    let value = std::env::var(name).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => Some(false),
        "1" | "true" | "yes" | "on" => Some(true),
        _ => None,
    }
}

fn debug_enabled() -> bool {
    parse_env_bool(ENV_TELEMETRY_DEBUG).unwrap_or(false)
}

fn debug_log(args: std::fmt::Arguments<'_>) {
    if debug_enabled() {
        eprintln!("[telemetry] {args}");
    } else {
        tracing::debug!(target: "cua_driver::telemetry", "{args}");
    }
}

fn os_family() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        _ => "other",
    }
}

fn os_major() -> String {
    static OS_MAJOR: OnceLock<String> = OnceLock::new();
    OS_MAJOR
        .get_or_init(|| {
            let raw = os_version();
            let start = raw.find(|character: char| character.is_ascii_digit());
            let Some(start) = start else {
                return "unknown".into();
            };
            raw[start..]
                .split(|character: char| !character.is_ascii_digit())
                .next()
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_owned()
        })
        .clone()
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    let command = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output();
    #[cfg(target_os = "windows")]
    let command = std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .output();
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|contents| {
                contents
                    .lines()
                    .find_map(|line| line.strip_prefix("VERSION_ID=").map(str::to_owned))
            })
            .map(|value| value.trim_matches('"').to_owned())
            .unwrap_or_else(|| "unknown".into());
    }
    #[cfg(not(target_os = "linux"))]
    command
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => "other",
    }
}

fn is_ci() -> bool {
    const VARIABLES: &[&str] = &[
        "CI",
        "CONTINUOUS_INTEGRATION",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "JENKINS_URL",
        "CIRCLECI",
        "BUILDKITE",
        "TF_BUILD",
        "TEAMCITY_VERSION",
        "TRAVIS",
        "APPVEYOR",
        "BITBUCKET_BUILD_NUMBER",
        "CODEBUILD_BUILD_ID",
        "DRONE",
        "HUDSON_URL",
        "CI_NAME",
    ];
    VARIABLES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};
    use std::sync::Mutex;
    use std::time::Instant;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const TEST_CHILD_KIND: &str = "CUA_DRIVER_TELEMETRY_TEST_CHILD_KIND";
    const TEST_CHILD_ROOT: &str = "CUA_DRIVER_TELEMETRY_TEST_CHILD_ROOT";
    const TEST_CHILD_INDEX: &str = "CUA_DRIVER_TELEMETRY_TEST_CHILD_INDEX";
    const TEST_CHILD_CANDIDATE: &str = "CUA_DRIVER_TELEMETRY_TEST_CHILD_CANDIDATE";

    fn with_isolated_home<R>(test: impl FnOnce(&Path) -> R) -> R {
        let root = std::env::temp_dir().join(format!("cua-telemetry-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let telemetry_home = root.join(HOME_SUBDIRECTORY);
        let old_telemetry_home = std::env::var_os(ENV_TELEMETRY_HOME);
        unsafe {
            std::env::set_var(ENV_TELEMETRY_HOME, &telemetry_home);
            std::env::remove_var(ENV_TELEMETRY_ENABLED);
            std::env::remove_var(ENV_TELEMETRY_ENABLED_COMPAT);
        }
        let result = test(&root);
        let _ = std::fs::remove_dir_all(&root);
        unsafe {
            match old_telemetry_home {
                Some(value) => std::env::set_var(ENV_TELEMETRY_HOME, value),
                None => std::env::remove_var(ENV_TELEMETRY_HOME),
            }
        }
        result
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }

    fn wait_for_children_ready(directory: &Path, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let ready = std::fs::read_dir(directory)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("ready-"))
                .count();
            if ready == count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn wait_for_child(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait().expect("poll test child") {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.try_wait().expect("poll test child")
    }

    fn spawn_test_child(kind: &str, root: &Path, index: usize) -> Child {
        Command::new(std::env::current_exe().expect("current test executable"))
            .arg(match kind {
                "identity" => "telemetry::tests::identity_process_child",
                "lifecycle-winner" | "lifecycle-contender" => {
                    "telemetry::tests::lifecycle_process_child"
                }
                _ => panic!("unknown test child kind"),
            })
            .arg("--exact")
            .arg("--nocapture")
            .env(TEST_CHILD_KIND, kind)
            .env(TEST_CHILD_ROOT, root)
            .env(TEST_CHILD_INDEX, index.to_string())
            .env(TEST_CHILD_CANDIDATE, uuid::Uuid::new_v4().to_string())
            .spawn()
            .expect("spawn telemetry test child")
    }

    #[test]
    fn identity_process_child() {
        if std::env::var(TEST_CHILD_KIND).ok().as_deref() != Some("identity") {
            return;
        }
        let root = PathBuf::from(std::env::var_os(TEST_CHILD_ROOT).expect("test child root"));
        let index = std::env::var(TEST_CHILD_INDEX).expect("test child index");
        let candidate = std::env::var(TEST_CHILD_CANDIDATE).expect("test child candidate");
        std::fs::write(root.join(format!("ready-{index}")), "1").expect("signal ready");
        assert!(wait_for_path(&root.join("go"), Duration::from_secs(15)));
        let path = marker_path(TELEMETRY_ID_FILE_NAME).expect("identity path");
        let id = persist_install_id_if_absent(&path, &candidate).expect("persist identity");
        std::fs::write(root.join(format!("result-{index}")), id).expect("write result");
    }

    #[test]
    fn lifecycle_process_child() {
        let Ok(kind) = std::env::var(TEST_CHILD_KIND) else {
            return;
        };
        if !matches!(kind.as_str(), "lifecycle-winner" | "lifecycle-contender") {
            return;
        }
        let root = PathBuf::from(std::env::var_os(TEST_CHILD_ROOT).expect("test child root"));
        let index = std::env::var(TEST_CHILD_INDEX).expect("test child index");
        let mut call = 0usize;
        capture_install_with_poster(|payload| {
            call += 1;
            std::fs::write(
                root.join(format!("event-{index}-{call}.json")),
                serde_json::to_vec(payload).expect("serialize captured event"),
            )
            .expect("write captured event");
            if kind == "lifecycle-winner" && call == 1 {
                std::fs::write(root.join("winner-ready"), "1").expect("signal winner ready");
                if !wait_for_path(&root.join("release-winner"), Duration::from_secs(15)) {
                    return Err("parent did not release lifecycle winner".to_owned());
                }
            }
            Ok(200)
        });
        std::fs::write(
            root.join(format!("observed-id-{index}")),
            read_install_id().unwrap_or_default(),
        )
        .expect("write observed identity");
    }

    #[test]
    fn installation_identity_creation_is_atomic_across_processes() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            const CHILDREN: usize = 8;
            let mut children: Vec<_> = (0..CHILDREN)
                .map(|index| spawn_test_child("identity", root, index))
                .collect();
            assert!(wait_for_children_ready(
                root,
                CHILDREN,
                Duration::from_secs(15)
            ));
            std::fs::write(root.join("go"), "1").unwrap();
            for child in &mut children {
                let status = wait_for_child(child, Duration::from_secs(15));
                if status.is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                assert!(
                    status.is_some_and(|status| status.success()),
                    "identity child must finish successfully"
                );
            }

            let persisted = read_install_id().expect("persisted identity");
            assert!(uuid::Uuid::parse_str(&persisted).is_ok());
            for index in 0..CHILDREN {
                let observed = std::fs::read_to_string(root.join(format!("result-{index}")))
                    .expect("child result");
                assert_eq!(observed, persisted);
            }
        });
    }

    #[test]
    fn lifecycle_registration_is_single_writer_and_non_blocking_across_processes() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            let mut winner = spawn_test_child("lifecycle-winner", root, 0);
            assert!(wait_for_path(
                &root.join("winner-ready"),
                Duration::from_secs(15)
            ));

            let mut contender = spawn_test_child("lifecycle-contender", root, 1);
            let contender_status = wait_for_child(&mut contender, Duration::from_secs(5));
            if contender_status.is_none() {
                let _ = std::fs::write(root.join("release-winner"), "1");
                let _ = contender.kill();
                let _ = winner.kill();
            }
            assert!(
                contender_status.is_some_and(|status| status.success()),
                "contender must skip a busy lifecycle lock without waiting for network delivery"
            );

            std::fs::write(root.join("release-winner"), "1").unwrap();
            let winner_status = wait_for_child(&mut winner, Duration::from_secs(15));
            if winner_status.is_none() {
                let _ = winner.kill();
                let _ = winner.wait();
            }
            assert!(
                winner_status.is_some_and(|status| status.success()),
                "lifecycle winner must finish successfully"
            );

            let mut events = std::fs::read_dir(root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("event-"))
                .map(|entry| {
                    serde_json::from_slice::<Value>(&std::fs::read(entry.path()).unwrap()).unwrap()
                })
                .collect::<Vec<_>>();
            events.sort_by_key(|event| event["event"].as_str().unwrap_or_default().to_owned());
            assert_eq!(events.len(), 2);
            assert_eq!(
                events
                    .iter()
                    .filter(|item| item["event"] == event::INSTALLATION_REGISTERED)
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|item| item["event"] == event::RELEASE_INSTALLED)
                    .count(),
                1
            );
            let persisted = read_install_id().expect("persisted identity");
            assert!(events.iter().all(|item| item["distinct_id"] == persisted));
            for index in 0..=1 {
                assert_eq!(
                    std::fs::read_to_string(root.join(format!("observed-id-{index}"))).unwrap(),
                    persisted
                );
            }
            assert!(root
                .join(HOME_SUBDIRECTORY)
                .join(INSTALLATION_RECORDED_FILE_NAME)
                .exists());
            assert!(release_marker_path(current_product_version())
                .unwrap()
                .exists());
        });
    }

    #[test]
    fn precedence_is_environment_then_persisted_then_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            assert_eq!(effective_enabled(), (true, "default"));
            set_enabled(false).unwrap();
            assert_eq!(effective_enabled(), (false, "persisted"));
            unsafe {
                std::env::set_var(ENV_TELEMETRY_ENABLED, "true");
            }
            assert_eq!(effective_enabled(), (true, "environment"));
            unsafe {
                std::env::remove_var(ENV_TELEMETRY_ENABLED);
            }
        });
    }

    #[test]
    fn persisted_install_channel_prevents_first_run_attribution_race() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            let home = root.join(HOME_SUBDIRECTORY);
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(
                home.join(TELEMETRY_INSTALL_CHANNEL_FILE_NAME),
                "install_script",
            )
            .unwrap();
            assert_eq!(install_channel(), "install_script");

            unsafe {
                std::env::set_var(ENV_INSTALL_CHANNEL, "update_apply");
            }
            assert_eq!(install_channel(), "update_apply");
            unsafe {
                std::env::remove_var(ENV_INSTALL_CHANNEL);
            }

            std::fs::write(home.join(TELEMETRY_INSTALL_CHANNEL_FILE_NAME), "invalid").unwrap();
            assert_eq!(install_channel(), "first_run");
        });
    }

    #[test]
    fn disabled_install_makes_no_request_or_marker_and_keeps_existing_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            let home = root.join(HOME_SUBDIRECTORY);
            std::fs::create_dir_all(&home).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            std::fs::write(home.join(TELEMETRY_ID_FILE_NAME), &id).unwrap();
            set_enabled(false).unwrap();
            unsafe {
                std::env::set_var(ENV_TELEMETRY_ENABLED, "false");
            }
            let mut calls = 0;
            capture_install_with_poster(|_| {
                calls += 1;
                Ok(200)
            });
            assert_eq!(calls, 0);
            assert_eq!(read_install_id().as_deref(), Some(id.as_str()));
            assert!(!home.join(INSTALLATION_RECORDED_FILE_NAME).exists());
            assert!(!home.join(TELEMETRY_RETRY_AFTER_FILE_NAME).exists());
            unsafe {
                std::env::remove_var(ENV_TELEMETRY_ENABLED);
            }
        });
    }

    #[test]
    fn disabled_permissions_events_do_not_create_an_installation_id() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            set_enabled(false).unwrap();
            capture_permissions_gate_started(true, false);
            capture_permissions_gate_dismissed(true, false, Duration::from_secs(3));
            capture_permissions_gate_completed(
                true,
                false,
                true,
                true,
                "dismissed_then_granted",
                Duration::from_secs(3),
            );
            assert!(read_install_id().is_none());
        });
    }

    #[test]
    fn permissions_resolution_covers_grant_dismissal_recovery_and_timeout() {
        assert_eq!(permissions_gate_resolution(false, false), "granted");
        assert_eq!(
            permissions_gate_resolution(false, true),
            "dismissed_then_granted"
        );
        assert_eq!(permissions_gate_resolution(true, false), "timeout");
        assert_eq!(permissions_gate_resolution(true, true), "timeout");
    }

    #[test]
    fn lifecycle_markers_are_written_only_after_each_2xx() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            let home = root.join(HOME_SUBDIRECTORY);
            let mut calls = 0;
            capture_install_with_poster(|_| {
                calls += 1;
                if calls == 1 {
                    Ok(200)
                } else {
                    Ok(500)
                }
            });
            assert!(home.join(INSTALLATION_RECORDED_FILE_NAME).exists());
            assert!(!release_marker_path(current_product_version())
                .unwrap()
                .exists());

            remove_file_if_exists(&home.join(TELEMETRY_RETRY_AFTER_FILE_NAME)).unwrap();
            capture_install_with_poster(|_| Ok(200));
            assert!(release_marker_path(current_product_version())
                .unwrap()
                .exists());
        });
    }

    #[test]
    fn failed_lifecycle_delivery_defers_retries_without_marking_success() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            let home = root.join(HOME_SUBDIRECTORY);
            let mut calls = 0;
            capture_install_with_poster(|_| {
                calls += 1;
                Ok(503)
            });
            assert_eq!(calls, 1);
            assert!(lifecycle_retry_deferred());
            assert!(!home.join(INSTALLATION_RECORDED_FILE_NAME).exists());

            capture_install_with_poster(|_| {
                calls += 1;
                Ok(200)
            });
            assert_eq!(
                calls, 1,
                "retry backoff must prevent per-command network stalls"
            );
        });
    }

    #[test]
    fn reset_erases_identity_and_markers_but_preserves_preference() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            set_enabled(false).unwrap();
            let home = root.join(HOME_SUBDIRECTORY);
            std::fs::write(
                home.join(TELEMETRY_ID_FILE_NAME),
                uuid::Uuid::new_v4().to_string(),
            )
            .unwrap();
            write_marker(&home.join(INSTALLATION_RECORDED_FILE_NAME)).unwrap();
            write_marker(&release_marker_path("1.2.3").unwrap()).unwrap();
            reset_id().unwrap();
            assert_eq!(persisted_enabled(), Some(false));
            assert!(read_install_id().is_none());
            assert!(!home.join(INSTALLATION_RECORDED_FILE_NAME).exists());
        });
    }

    #[test]
    fn reset_erases_legacy_identity_before_migration_can_restore_it() {
        let _guard = ENV_LOCK.lock().unwrap();
        let root =
            std::env::temp_dir().join(format!("cua-telemetry-reset-{}", uuid::Uuid::new_v4()));
        let current = root.join(HOME_SUBDIRECTORY);
        let legacy = root.join(LEGACY_HOME_SUBDIRECTORY);
        std::fs::create_dir_all(&legacy).unwrap();
        let legacy_id = uuid::Uuid::new_v4().to_string();
        std::fs::write(legacy.join(TELEMETRY_ID_FILE_NAME), &legacy_id).unwrap();
        std::fs::write(legacy.join(INSTALLATION_RECORDED_FILE_NAME), "1").unwrap();

        reset_id_in_homes(&current, Some(&legacy)).unwrap();
        assert!(!current.join(TELEMETRY_ID_FILE_NAME).exists());
        assert!(!legacy.join(TELEMETRY_ID_FILE_NAME).exists());
        assert!(!legacy.join(INSTALLATION_RECORDED_FILE_NAME).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn v3_payload_allows_server_geoip_without_sending_an_ip_or_client_timestamp() {
        let _guard = ENV_LOCK.lock().unwrap();
        let identity = InstallationIdentity {
            id: "test-id".into(),
            persisted: true,
        };
        let payload = build_payload(
            event::MCP_TOOL_COMPLETED,
            &bounded_properties(&[
                ("tool_name", Value::String("click".into())),
                ("success", Value::Bool(true)),
            ]),
            &identity,
            Transport::McpStdio,
        );
        assert!(payload.get("timestamp").is_none());
        let properties = payload["properties"].as_object().unwrap();
        for required in [
            "telemetry_schema_version",
            "product_version",
            "os_family",
            "os_major",
            "arch",
            "is_ci",
            "is_synthetic",
            "transport",
            "process_session_id",
            "id_persisted",
            "$process_person_profile",
            "$lib",
            "$lib_version",
            "tool_name",
            "success",
        ] {
            assert!(properties.contains_key(required), "missing {required}");
        }
        assert_eq!(properties["telemetry_schema_version"], 3);
        assert_eq!(properties["is_synthetic"], false);
        assert!(!properties.contains_key("$geoip_disable"));
        assert!(!properties.contains_key("$ip"));
        let serialized = serde_json::to_string(&payload)
            .unwrap()
            .to_ascii_lowercase();
        for forbidden in [
            "prompt",
            "task_text",
            "arguments",
            "result_text",
            "typed_text",
            "clipboard",
            "screenshot",
            "accessibility_tree",
            "window_title",
            "application_name",
            "file_path",
            "url",
            "raw_error",
            "stack_trace",
            "initialize_payload",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "payload contains {forbidden}: {serialized}"
            );
        }
    }

    #[test]
    fn synthetic_marker_is_explicit_and_common_to_all_events() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var_os(ENV_TELEMETRY_SYNTHETIC);
        unsafe {
            std::env::set_var(ENV_TELEMETRY_SYNTHETIC, "true");
        }
        let payload = inspect_event(event::MCP_TOOL_COMPLETED).unwrap();
        assert_eq!(payload["properties"]["is_synthetic"], true);
        unsafe {
            match original {
                Some(value) => std::env::set_var(ENV_TELEMETRY_SYNTHETIC, value),
                None => std::env::remove_var(ENV_TELEMETRY_SYNTHETIC),
            }
        }
    }

    #[test]
    fn inspect_events_match_the_emitted_bounded_schema() {
        let session = inspect_event(event::MCP_SESSION_STARTED).unwrap();
        let session_properties = session["properties"].as_object().unwrap();
        for field in [
            "mcp_client",
            "mcp_client_version_major",
            "protocol_version",
            "capability_tools",
            "capability_roots",
            "capability_sampling",
            "capability_experimental",
            "capability_elicitation_form",
            "capability_elicitation_url",
            "reported_provider",
            "reported_model",
            "reported_agent",
            "reported_agent_version_major",
            "execution_mode",
        ] {
            assert!(session_properties.contains_key(field), "missing {field}");
        }

        let tool = inspect_event(event::MCP_TOOL_COMPLETED).unwrap();
        assert_eq!(tool["properties"]["duration_bucket"], "lt_10ms");
        assert_eq!(tool["properties"]["operation"], "not_applicable");
        assert_eq!(tool["properties"]["refusal_code"], "none");
        assert_eq!(tool["properties"]["computer_action"], false);
        assert!(matches!(
            tool["properties"]["execution_mode"].as_str(),
            Some("embedded" | "standalone")
        ));
        let mcp_start = inspect_event(event::MCP_STARTUP_COMPLETED).unwrap();
        assert_eq!(mcp_start["properties"]["transport"], "mcp_stdio");
        assert_eq!(mcp_start["properties"]["path"], "daemon_proxy");
        assert_eq!(
            mcp_start["properties"]["execution_mode"],
            tool["properties"]["execution_mode"]
        );
        let permissions_started = inspect_event(event::PERMISSIONS_GATE_STARTED).unwrap();
        assert_eq!(permissions_started["properties"]["transport"], "daemon");
        assert_eq!(
            permissions_started["properties"]["missing_accessibility"],
            true
        );
        assert_eq!(
            permissions_started["properties"]["missing_screen_recording"],
            true
        );
        assert!(permissions_started["properties"]
            .get("duration_bucket")
            .is_none());
        let permissions_dismissed = inspect_event(event::PERMISSIONS_GATE_DISMISSED).unwrap();
        assert_eq!(permissions_dismissed["properties"]["transport"], "daemon");
        assert_eq!(
            permissions_dismissed["properties"]["duration_bucket"],
            "2s_9_999ms"
        );
        let permissions = inspect_event(event::PERMISSIONS_GATE_COMPLETED).unwrap();
        assert_eq!(permissions["properties"]["resolution"], "granted");
        for payload in [permissions_started, permissions_dismissed, permissions] {
            assert_eq!(payload["properties"]["telemetry_schema_version"], 3);
            assert!(payload.get("timestamp").is_none());
            let serialized = serde_json::to_string(&payload)
                .unwrap()
                .to_ascii_lowercase();
            for forbidden in [
                "prompt",
                "arguments",
                "result_text",
                "typed_text",
                "screenshot",
                "accessibility_tree",
                "window_title",
                "application_name",
                "file_path",
                "socket",
                "raw_error",
                "stack_trace",
                "$ip",
            ] {
                assert!(
                    !serialized.contains(forbidden),
                    "permissions payload contains {forbidden}: {serialized}"
                );
            }
        }
        let serve_start = inspect_event(event::SERVE_START_LEGACY).unwrap();
        assert_eq!(serve_start["properties"]["transport"], "daemon");
    }

    #[test]
    fn session_observation_keys_are_bounded_by_transport_and_client_category() {
        assert_eq!(
            mcp_session_observation_key(Transport::McpStdio, "claude_code"),
            "mcp_stdio"
        );
        assert_eq!(
            mcp_session_observation_key(Transport::McpHttp, "claude_code"),
            "mcp_http:claude_code"
        );
        assert_eq!(
            mcp_session_observation_key(Transport::McpHttp, "other"),
            "mcp_http:other"
        );
    }

    #[test]
    fn execution_mode_requires_the_exact_embedded_sentinel() {
        let _guard = ENV_LOCK.lock().unwrap();
        let name = cua_driver_core::EMBEDDED_ENV;
        let original = std::env::var_os(name);
        unsafe {
            std::env::remove_var(name);
        }
        assert_eq!(execution_mode(), "standalone");
        unsafe {
            std::env::set_var(name, "1");
        }
        assert_eq!(execution_mode(), "embedded");
        unsafe {
            std::env::set_var(name, "true");
        }
        assert_eq!(execution_mode(), "standalone");
        unsafe {
            match original {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }

    #[test]
    fn agent_session_aggregate_is_bounded_content_free_and_multi_transport() {
        use cua_driver_core::server::{
            DurationBucket, OutputSizeBucket, OutputType, ToolCompletionObservation, ToolErrorClass,
        };

        let mut state = AgentSessionState::new(Transport::McpStdio);
        state.started = std::time::Instant::now() - Duration::from_secs(75);
        state.observe(
            Transport::McpHttp,
            true,
            &ToolCompletionObservation {
                tool_name: "click".into(),
                operation: cua_driver_core::server::ToolOperation::NotApplicable,
                computer_action: true,
                success: true,
                error_class: ToolErrorClass::None,
                refusal_code: cua_driver_core::server::ToolRefusalCode::None,
                duration_bucket: DurationBucket::Under10Ms,
                output_type: OutputType::Text,
                output_size_bucket: OutputSizeBucket::Under1KiB,
            },
        );
        state.observe(
            Transport::McpHttp,
            false,
            &ToolCompletionObservation {
                tool_name: "page".into(),
                operation: cua_driver_core::server::ToolOperation::QueryDom,
                computer_action: false,
                success: false,
                error_class: ToolErrorClass::InternalError,
                refusal_code: cua_driver_core::server::ToolRefusalCode::None,
                duration_bucket: DurationBucket::Ms50To249,
                output_type: OutputType::Empty,
                output_size_bucket: OutputSizeBucket::Empty,
            },
        );
        let properties = state.ended_properties(
            cua_driver_core::session::SessionEndReason::Explicit,
            Some(cua_driver_core::session::CursorOutcomeObservation {
                observed: true,
                enabled: true,
                style: cua_driver_core::session::CursorStyleCategory::CustomIcon,
                color_source: cua_driver_core::session::CursorColorSource::Custom,
                label_set: true,
                motion_customized: true,
                active_cursor_count: 3,
            }),
        );
        assert_eq!(properties["duration_bucket"], "1_4min");
        assert_eq!(properties["tool_count_bucket"], "1_4");
        assert_eq!(properties["computer_action_count_bucket"], "1_4");
        assert_eq!(properties["error_count_bucket"], "1");
        assert_eq!(properties["had_successful_tool"], true);
        assert_eq!(properties["had_successful_computer_action"], true);
        assert_eq!(properties["used_page"], true);
        assert_eq!(properties["used_browser"], false);
        assert_eq!(properties["browser_refusal_count_bucket"], "0");
        assert_eq!(properties["cursor_style"], "custom_icon");
        assert_eq!(properties["cursor_color_source"], "custom");
        assert_eq!(properties["cursor_label_set"], true);
        assert_eq!(properties["cursor_motion_customized"], true);
        assert_eq!(properties["multi_cursor_bucket"], "3_5");
        assert_eq!(properties["observed_multiple_transports"], true);

        let serialized = serde_json::to_string(&properties)
            .unwrap()
            .to_ascii_lowercase();
        for forbidden in [
            "private-session-id",
            "arguments",
            "selector",
            "typed_text",
            "screenshot",
            "window_title",
            "file_path",
            "raw_error",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "aggregate contains {forbidden}: {serialized}"
            );
        }
    }

    #[test]
    fn browser_refusal_is_bounded_and_not_a_completed_computer_action() {
        use cua_driver_core::server::{
            DurationBucket, OutputSizeBucket, OutputType, ToolCompletionObservation,
            ToolErrorClass, ToolOperation, ToolRefusalCode,
        };

        let outcome = ToolCompletionObservation {
            tool_name: "browser_click".into(),
            operation: ToolOperation::BrowserClickTrusted,
            computer_action: true,
            success: true,
            error_class: ToolErrorClass::None,
            refusal_code: ToolRefusalCode::BrowserInputTrustUnavailable,
            duration_bucket: DurationBucket::Ms50To249,
            output_type: OutputType::Text,
            output_size_bucket: OutputSizeBucket::Under1KiB,
        };
        let event_properties = tool_completion_properties(outcome.clone());
        assert_eq!(event_properties["success"], true);
        assert_eq!(event_properties["error_class"], "none");
        assert_eq!(
            event_properties["refusal_code"],
            "browser_input_trust_unavailable"
        );
        assert_eq!(event_properties["operation"], "browser_click_trusted");

        let mut state = AgentSessionState::new(Transport::McpStdio);
        state.observe(Transport::McpStdio, true, &outcome);
        let session_properties =
            state.ended_properties(cua_driver_core::session::SessionEndReason::Explicit, None);
        assert_eq!(session_properties["computer_action_count_bucket"], "0");
        assert_eq!(session_properties["error_count_bucket"], "0");
        assert_eq!(session_properties["browser_refusal_count_bucket"], "1");
        assert_eq!(session_properties["had_successful_tool"], true);
        assert_eq!(session_properties["had_successful_computer_action"], false);
        assert_eq!(session_properties["used_browser"], true);
    }

    #[test]
    fn every_browser_tool_marks_the_session_without_retaining_arguments() {
        use cua_driver_core::server::{
            DurationBucket, OutputSizeBucket, OutputType, ToolCompletionObservation,
            ToolErrorClass, ToolOperation, ToolRefusalCode,
        };

        for (tool_name, operation) in [
            ("browser_dialog", ToolOperation::BrowserDialogAccept),
            (
                "browser_set_input_files",
                ToolOperation::BrowserSetInputFiles,
            ),
            ("browser_download", ToolOperation::BrowserDownload),
            (
                "browser_pointer",
                ToolOperation::BrowserPointerDoubleClickDomEvent,
            ),
        ] {
            let mut state = AgentSessionState::new(Transport::McpStdio);
            state.observe(
                Transport::McpStdio,
                true,
                &ToolCompletionObservation {
                    tool_name: tool_name.into(),
                    operation,
                    computer_action: true,
                    success: true,
                    error_class: ToolErrorClass::None,
                    refusal_code: ToolRefusalCode::None,
                    duration_bucket: DurationBucket::Under10Ms,
                    output_type: OutputType::Text,
                    output_size_bucket: OutputSizeBucket::Under1KiB,
                },
            );
            let properties =
                state.ended_properties(cua_driver_core::session::SessionEndReason::Explicit, None);
            assert_eq!(properties["used_browser"], true, "tool={tool_name}");
            assert_eq!(
                properties["had_successful_computer_action"], true,
                "tool={tool_name}"
            );
            let serialized = serde_json::to_string(&properties).unwrap();
            assert!(!serialized.contains("destination_root"));
            assert!(!serialized.contains("prompt_text"));
            assert!(!serialized.contains("file_paths"));
        }
    }

    #[test]
    fn agent_session_buckets_have_fixed_boundaries() {
        for (count, expected) in [
            (0, "0"),
            (1, "1_4"),
            (4, "1_4"),
            (5, "5_19"),
            (19, "5_19"),
            (20, "20_99"),
            (99, "20_99"),
            (100, "gte_100"),
        ] {
            assert_eq!(agent_session_count_bucket(count), expected);
        }
        for (count, expected) in [(0, "0"), (1, "1"), (2, "2_4"), (4, "2_4"), (5, "gte_5")] {
            assert_eq!(agent_session_error_bucket(count), expected);
        }
    }

    #[test]
    fn agent_session_inspect_samples_expose_only_the_contract() {
        for event_name in [event::AGENT_SESSION_STARTED, event::AGENT_SESSION_ENDED] {
            let payload = inspect_event(event_name).unwrap();
            assert_eq!(payload["properties"]["telemetry_schema_version"], 3);
            let properties = payload["properties"].as_object().unwrap();
            for forbidden_key in ["session", "session_id", "agent_session_id", "cursor_id"] {
                assert!(!properties.contains_key(forbidden_key));
            }
            let serialized = serde_json::to_string(&payload)
                .unwrap()
                .to_ascii_lowercase();
            for forbidden in [
                "arguments",
                "result_text",
                "file_path",
                "socket",
                "raw_error",
                "$ip",
            ] {
                assert!(
                    !serialized.contains(forbidden),
                    "inspect payload contains {forbidden}: {serialized}"
                );
            }
        }
    }

    #[test]
    fn disabled_agent_session_observation_creates_no_state_or_identity() {
        use cua_driver_core::server::{
            DurationBucket, OutputSizeBucket, OutputType, ToolCompletionObservation,
            ToolErrorClass, ToolOperation, ToolRefusalCode,
        };
        use cua_driver_core::session::{
            SessionDeclaration, SessionObserver, SessionStartObservation, SessionTransport,
        };

        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            set_enabled(false).unwrap();
            let session_id = "private-disabled-session-telemetry-test";
            TelemetryObserver.on_session_started(
                session_id,
                SessionStartObservation {
                    declaration: SessionDeclaration::StartSession,
                    revived: false,
                    transport: SessionTransport::McpStdio,
                },
            );
            capture_tool_completed(
                ToolCompletionObservation {
                    tool_name: "browser_prepare".into(),
                    operation: ToolOperation::BrowserPrepareExistingProfile,
                    computer_action: false,
                    success: true,
                    error_class: ToolErrorClass::None,
                    refusal_code: ToolRefusalCode::BrowserConsentRevoked,
                    duration_bucket: DurationBucket::Ms50To249,
                    output_type: OutputType::Text,
                    output_size_bucket: OutputSizeBucket::Under1KiB,
                },
                Transport::McpStdio,
            );
            assert!(!AGENT_SESSIONS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap()
                .contains_key(session_id));
            assert!(read_install_id().is_none());
        });
    }

    #[test]
    fn tool_completion_rate_limit_resets_after_one_hour() {
        let started = Instant::now();
        let mut limit = ToolTelemetryRateLimit::new(started);

        for _ in 0..TOOL_TELEMETRY_LIMIT_PER_HOUR {
            assert!(limit.allow(started, false));
        }
        assert!(!limit.allow(started, false));
        assert!(limit.allow(started, true));
        assert!(!limit.allow(started, true));
        assert!(limit.allow(started + TOOL_TELEMETRY_WINDOW, false));
    }

    #[test]
    fn http_tool_completion_reaches_the_final_payload_as_mcp_http() {
        use cua_driver_core::server::{
            DurationBucket, OutputSizeBucket, OutputType, ToolCompletionObservation, ToolErrorClass,
        };

        let properties = tool_completion_properties(ToolCompletionObservation {
            tool_name: "click".into(),
            operation: cua_driver_core::server::ToolOperation::NotApplicable,
            computer_action: true,
            success: true,
            error_class: ToolErrorClass::None,
            refusal_code: cua_driver_core::server::ToolRefusalCode::None,
            duration_bucket: DurationBucket::Under10Ms,
            output_type: OutputType::Text,
            output_size_bucket: OutputSizeBucket::Under1KiB,
        });
        let payload = build_payload(
            event::MCP_TOOL_COMPLETED,
            &properties,
            &InstallationIdentity {
                id: "test-id".into(),
                persisted: true,
            },
            Transport::McpHttp,
        );

        assert_eq!(payload["properties"]["transport"], "mcp_http");
        assert_eq!(payload["properties"]["tool_name"], "click");
        assert_eq!(payload["properties"]["operation"], "not_applicable");
        assert_eq!(payload["properties"]["computer_action"], true);
        assert_eq!(payload["properties"]["success"], true);
    }

    #[test]
    fn compound_tool_operation_reaches_payload_as_a_fixed_value() {
        use cua_driver_core::server::{
            DurationBucket, OutputSizeBucket, OutputType, ToolCompletionObservation,
            ToolErrorClass, ToolOperation,
        };
        let properties = tool_completion_properties(ToolCompletionObservation {
            tool_name: "page".into(),
            operation: ToolOperation::InsertText,
            computer_action: true,
            success: true,
            error_class: ToolErrorClass::None,
            refusal_code: cua_driver_core::server::ToolRefusalCode::None,
            duration_bucket: DurationBucket::Under10Ms,
            output_type: OutputType::Text,
            output_size_bucket: OutputSizeBucket::Under1KiB,
        });
        assert_eq!(properties["operation"], "insert_text");
        assert_eq!(properties["computer_action"], true);
        let serialized = serde_json::to_string(&properties).unwrap();
        for forbidden in ["selector", "private typed text", "script"] {
            assert!(!serialized.contains(forbidden));
        }
    }

    #[test]
    fn cli_tool_names_are_fixed_and_content_free() {
        assert_eq!(fixed_tool_name("click"), "click");
        assert_eq!(fixed_tool_name("type_text_chars"), "type_text");
        assert_eq!(fixed_tool_name("../../private/secret"), "other");
        assert_eq!(fixed_tool_name("https://example.com/private"), "other");
    }

    #[test]
    fn cli_operations_and_client_kinds_are_revalidated_in_the_worker() {
        assert_eq!(fixed_cli_operation("recording", "start"), "start");
        assert_eq!(fixed_cli_operation("recording", "/private/path"), "other");
        assert_eq!(fixed_cli_operation("doctor", "start"), "not_applicable");
        assert_eq!(
            fixed_cli_client_kind("mcp_config", "claude_code"),
            "claude_code"
        );
        assert_eq!(
            fixed_cli_client_kind("mcp_config", "/private/client"),
            "other"
        );
        assert_eq!(
            fixed_cli_client_kind("doctor", "claude_code"),
            "not_applicable"
        );
    }

    #[test]
    fn process_session_id_is_stable_and_not_written_to_disk() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            assert_eq!(process_session_id(), process_session_id());
            let on_disk = std::fs::read_dir(root.join(HOME_SUBDIRECTORY))
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().contains("session"));
            assert!(!on_disk);
        });
    }

    #[test]
    fn release_version_is_bounded_and_safe_for_payloads_and_marker_paths() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os(ENV_RELEASE_VERSION);
        unsafe {
            std::env::set_var(
                ENV_RELEASE_VERSION,
                "v1.2.3/../../private path?token=secret-and-more",
            );
        }
        assert_eq!(release_version(), current_product_version());
        assert!(!release_version().contains("secret"));
        unsafe {
            match previous {
                Some(value) => std::env::set_var(ENV_RELEASE_VERSION, value),
                None => std::env::remove_var(ENV_RELEASE_VERSION),
            }
        }
    }

    #[test]
    fn cli_duration_buckets_have_fixed_boundaries() {
        assert_eq!(duration_bucket(Duration::from_millis(99)), "lt_100ms");
        assert_eq!(duration_bucket(Duration::from_millis(100)), "100_499ms");
        assert_eq!(duration_bucket(Duration::from_millis(500)), "500ms_1_999ms");
        assert_eq!(duration_bucket(Duration::from_secs(2)), "2s_9_999ms");
        assert_eq!(duration_bucket(Duration::from_secs(10)), "gte_10s");
    }

    #[test]
    fn release_versions_accept_strict_semver_and_drop_leading_v() {
        assert_eq!(strict_release_version("v1.2.3"), Some("1.2.3".into()));
        assert_eq!(
            strict_release_version("1.2.3-rc.1"),
            Some("1.2.3-rc.1".into())
        );
        assert_eq!(strict_release_version("1.2"), None);
        assert_eq!(strict_release_version("1.2.3+private"), None);
    }

    #[test]
    fn reported_models_are_coarse_fixed_categories() {
        assert_eq!(normalize_model(Some("claude-opus-4-1")), "claude_opus");
        assert_eq!(normalize_model(Some("gpt-5.2-codex")), "gpt_5");
        assert_eq!(normalize_model(Some("gpt-private_secret")), "custom");
        assert!(!normalize_model(Some("gpt-private_secret")).contains("secret"));
    }

    #[test]
    fn pending_send_flush_waits_for_bounded_delivery() {
        let _guard = ENV_LOCK.lock().unwrap();
        assert_eq!(PENDING_SENDS.load(Ordering::SeqCst), 0);
        PENDING_SENDS.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            finish_pending_send();
        });
        flush_pending(Duration::from_secs(1));
        assert_eq!(PENDING_SENDS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cli_command_values_are_fixed() {
        assert_eq!(fixed_cli_command("call"), "call");
        assert_eq!(fixed_cli_command("private-customer-command"), "other");
    }
}
