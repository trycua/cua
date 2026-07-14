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
//! or raw errors.

use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

const POSTHOG_CAPTURE_URL: &str = "https://eu.i.posthog.com/capture/";
const POSTHOG_API_KEY: &str = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14";
const POSTHOG_TIMEOUT_SECS: u64 = 3;

const HOME_SUBDIRECTORY: &str = ".cua-driver";
const LEGACY_HOME_SUBDIRECTORY: &str = ".cua-driver-rs";
const CONFIG_FILE_NAME: &str = "config.json";
const CONFIG_ENABLED_KEY: &str = "telemetry_enabled";
const TELEMETRY_ID_FILE_NAME: &str = ".telemetry_id";
const INSTALLATION_RECORDED_FILE_NAME: &str = ".installation_recorded";
const RELEASE_RECORDED_DIRECTORY: &str = ".release_installed";

pub const ENV_TELEMETRY_ENABLED: &str = "CUA_DRIVER_RS_TELEMETRY_ENABLED";
const ENV_TELEMETRY_ENABLED_COMPAT: &str = "CUA_TELEMETRY_ENABLED";
const ENV_TELEMETRY_DEBUG: &str = "CUA_DRIVER_RS_TELEMETRY_DEBUG";
const ENV_TELEMETRY_HOME: &str = "CUA_DRIVER_TELEMETRY_HOME";
const ENV_CLI_WRAPPED_CHILD: &str = "CUA_DRIVER_CLI_TELEMETRY_CHILD";
const ENV_CLI_COMPLETION_WORKER: &str = "CUA_DRIVER_CLI_TELEMETRY_WORKER";
const ENV_CLI_COMPLETION_COMMAND: &str = "CUA_DRIVER_CLI_TELEMETRY_COMMAND";
const ENV_CLI_COMPLETION_EXIT_CODE: &str = "CUA_DRIVER_CLI_TELEMETRY_EXIT_CODE";
const ENV_CLI_COMPLETION_DURATION_MS: &str = "CUA_DRIVER_CLI_TELEMETRY_DURATION_MS";
pub const ENV_INSTALL_CHANNEL: &str = "CUA_DRIVER_INSTALL_CHANNEL";
pub const ENV_RELEASE_VERSION: &str = "CUA_DRIVER_RELEASE_VERSION";

pub mod event {
    pub const INSTALLATION_REGISTERED: &str = "cua_driver_installation_registered";
    pub const RELEASE_INSTALLED: &str = "cua_driver_release_installed";
    pub const MCP_START_LEGACY: &str = "cua_driver_mcp";
    pub const SERVE_START_LEGACY: &str = "cua_driver_serve";
    pub const CLI_COMPLETED: &str = "cua_driver_cli_completed";
    pub const MCP_SESSION_STARTED: &str = "cua_driver_mcp_session_started";
    pub const MCP_TOOL_COMPLETED: &str = "cua_driver_mcp_tool_completed";
}

const INSPECTABLE_EVENTS: &[&str] = &[
    event::INSTALLATION_REGISTERED,
    event::RELEASE_INSTALLED,
    event::CLI_COMPLETED,
    event::MCP_SESSION_STARTED,
    event::MCP_TOOL_COMPLETED,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // HTTP is intentionally deferred until session semantics are defined.
pub enum Transport {
    Cli,
    McpStdio,
    McpHttp,
    Unknown,
}

impl Transport {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::McpStdio => "mcp_stdio",
            Self::McpHttp => "mcp_http",
            Self::Unknown => "unknown",
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
    remove_file_if_exists(&home.join(TELEMETRY_ID_FILE_NAME))?;
    remove_file_if_exists(&home.join(INSTALLATION_RECORDED_FILE_NAME))?;
    let releases = home.join(RELEASE_RECORDED_DIRECTORY);
    if releases.exists() {
        std::fs::remove_dir_all(&releases)
            .map_err(|error| format!("failed to remove {}: {error}", releases.display()))?;
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
            ("success", Value::Bool(true)),
            ("exit_class", Value::String("success".into())),
            ("duration_bucket", Value::String("lt_100ms".into())),
        ]),
        event::MCP_SESSION_STARTED => bounded_properties(&[
            ("mcp_client", Value::String("unknown".into())),
            ("protocol_version", Value::String("unknown".into())),
            ("reported_provider", Value::String("unknown".into())),
            ("reported_model", Value::String("unknown".into())),
        ]),
        event::MCP_TOOL_COMPLETED => bounded_properties(&[
            ("tool_name", Value::String("other".into())),
            ("success", Value::Bool(true)),
            ("error_class", Value::String("none".into())),
            ("duration_bucket", Value::String("lt_100ms".into())),
            ("output_type", Value::String("empty".into())),
            ("output_size_bucket", Value::String("0".into())),
        ]),
        _ => Map::new(),
    };
    let transport = if event_name.starts_with("cua_driver_mcp_") {
        Transport::McpStdio
    } else {
        Transport::Cli
    };
    Ok(build_payload(event_name, &properties, &identity, transport))
}

/// Transitional fixed start events for long-running processes only.
pub fn capture_start(event_name: &'static str, transport: Transport) {
    if !matches!(event_name, event::MCP_START_LEGACY | event::SERVE_START_LEGACY) {
        return;
    }
    capture_bounded(event_name, Map::new(), transport);
}

pub fn register_stdio_observer() {
    let _ = cua_driver_core::server::set_stdio_observer(Arc::new(TelemetryObserver));
}

struct TelemetryObserver;

impl cua_driver_core::server::StdioObserver for TelemetryObserver {
    fn on_session_started(&self, metadata: cua_driver_core::protocol::InitializeMetadata) {
        static OBSERVED: AtomicBool = AtomicBool::new(false);
        if OBSERVED.swap(true, Ordering::SeqCst) {
            return;
        }
        let context = metadata.reported_agent_context.unwrap_or_default();
        let capabilities = metadata.capability_flags;
        capture_bounded(
            event::MCP_SESSION_STARTED,
            bounded_properties(&[
                ("mcp_client", Value::String(normalize_client(metadata.client_name.as_deref()))),
                ("mcp_client_version_major", Value::String(version_major(metadata.client_version.as_deref()))),
                ("protocol_version", Value::String(normalize_protocol(metadata.protocol_version.as_deref()))),
                ("capability_tools", Value::Bool(capabilities.tools)),
                ("capability_roots", Value::Bool(capabilities.roots)),
                ("capability_sampling", Value::Bool(capabilities.sampling)),
                ("capability_experimental", Value::Bool(capabilities.experimental)),
                ("capability_elicitation_form", Value::Bool(capabilities.elicitation_form)),
                ("capability_elicitation_url", Value::Bool(capabilities.elicitation_url)),
                ("reported_provider", Value::String(normalize_provider(context.provider.as_deref()))),
                ("reported_model", Value::String(normalize_model(context.model.as_deref()))),
                ("reported_agent", Value::String(normalize_client(context.agent_name.as_deref()))),
                ("reported_agent_version_major", Value::String(version_major(context.agent_version.as_deref()))),
            ]),
            Transport::McpStdio,
        );
    }

    fn on_tool_completed(&self, outcome: cua_driver_core::server::ToolCompletionObservation) {
        use cua_driver_core::server::{DurationBucket, OutputSizeBucket, OutputType, ToolErrorClass};
        let tool_name = if matches!(outcome.error_class, ToolErrorClass::UnknownTool | ToolErrorClass::InvalidParams) {
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
        capture_bounded(
            event::MCP_TOOL_COMPLETED,
            bounded_properties(&[
                ("tool_name", Value::String(tool_name)),
                ("success", Value::Bool(outcome.success)),
                ("error_class", Value::String(error_class.into())),
                ("duration_bucket", Value::String(duration_bucket.into())),
                ("output_type", Value::String(output_type.into())),
                ("output_size_bucket", Value::String(output_size_bucket.into())),
            ]),
            Transport::McpStdio,
        );
    }
}

fn normalize_client(value: Option<&str>) -> String {
    let normalized = value.unwrap_or("").trim().to_ascii_lowercase();
    for (needle, canonical) in [
        ("claude", "claude_code"), ("codex", "codex"), ("cursor", "cursor"),
        ("windsurf", "windsurf"), ("vscode", "vscode"), ("visual studio code", "vscode"),
        ("zed", "zed"), ("openclaw", "openclaw"), ("opencode", "opencode"),
        ("hermes", "hermes"),
    ] {
        if normalized.contains(needle) { return canonical.into(); }
    }
    if normalized == "pi" || normalized.starts_with("pi/") || normalized.starts_with("pi ") {
        return "pi".into();
    }
    if normalized.is_empty() { "unknown".into() } else { "other".into() }
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
    if raw.is_empty() { return "unknown".into(); }
    let category = if raw.starts_with("claude-") {
        if raw.contains("opus") { "claude_opus" }
        else if raw.contains("sonnet") { "claude_sonnet" }
        else if raw.contains("haiku") { "claude_haiku" }
        else { "claude_other" }
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
        if raw.contains("flash") { "gemini_flash" }
        else if raw.contains("pro") { "gemini_pro" }
        else { "gemini_other" }
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
        Some(value @ ("2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25")) => value.to_owned(),
        _ => "unknown".into(),
    }
}

fn version_major(value: Option<&str>) -> String {
    let major = value
        .unwrap_or("")
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())
        .unwrap_or("unknown");
    if major.len() <= 4 { major.to_owned() } else { "unknown".into() }
}

pub fn ensure_first_run_registration() {
    if !is_enabled() || lifecycle_is_current() {
        return;
    }
    eprintln!(
        "Cua Driver sends content-free product telemetry by default. Run `cua-driver telemetry disable` to stop it; `cua-driver telemetry status` shows the current setting."
    );
    capture_install();
}

pub fn capture_install() {
    capture_install_with_poster(post_to_posthog);
}

fn capture_install_with_poster<F>(mut post: F)
where
    F: FnMut(&Value) -> Result<u16, String>,
{
    if !is_enabled() {
        return;
    }
    let Some(home) = telemetry_home_dir() else {
        return;
    };
    let identity = get_or_create_install_id();
    let channel = install_channel();
    let registration_marker = home.join(INSTALLATION_RECORDED_FILE_NAME);

    if !registration_marker.exists() {
        let payload = build_payload(
            event::INSTALLATION_REGISTERED,
            &lifecycle_properties(channel),
            &identity,
            Transport::Cli,
        );
        if post_success(&mut post, &payload, event::INSTALLATION_REGISTERED) {
            if write_marker(&registration_marker).is_err() {
                return;
            }
        } else {
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
    if post_success(&mut post, &payload, event::RELEASE_INSTALLED) {
        let _ = write_marker(&release_marker);
    }
}

fn lifecycle_is_current() -> bool {
    let registered = marker_path(INSTALLATION_RECORDED_FILE_NAME)
        .is_some_and(|path| path.exists());
    let release = release_marker_path(&release_version())
        .is_some_and(|path| path.exists());
    registered && release
}

fn post_success<F>(post: &mut F, payload: &Value, event_name: &str) -> bool
where
    F: FnMut(&Value) -> Result<u16, String>,
{
    match post(payload) {
        Ok(status) if (200..300).contains(&status) => true,
        Ok(status) => {
            debug_log(format_args!("{event_name} returned {status}; marker not written"));
            false
        }
        Err(error) => {
            debug_log(format_args!("{event_name} failed: {error}; marker not written"));
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
    let identity = get_or_create_install_id();
    let payload = build_payload(event_name, &properties, &identity, transport);
    spawn_payload(event_name, payload);
}

/// Record a finite CLI command only after its outcome is known. This is
/// synchronous with a short timeout because one-shot processes may exit before
/// a detached request is delivered.
pub(crate) fn capture_cli_completed(
    command: &'static str,
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
    let exit_class = match exit_code {
        0 => "success",
        1 => "tool_error",
        64 => "invalid_input",
        _ => "other",
    };
    let identity = get_or_create_install_id();
    let payload = build_payload(
        event::CLI_COMPLETED,
        &bounded_properties(&[
            ("command", Value::String(command.into())),
            ("success", Value::Bool(success)),
            ("exit_class", Value::String(exit_class.into())),
            ("duration_bucket", Value::String(duration_bucket(elapsed).into())),
        ]),
        &identity,
        Transport::Cli,
    );
    if let Err(error) = post_to_posthog_with_timeout(&payload, Duration::from_secs(POSTHOG_TIMEOUT_SECS)) {
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
    capture_cli_completed(command, exit_code, elapsed);
    true
}

pub(crate) fn spawn_cli_completion_worker(
    command: &'static str,
    exit_code: i32,
    elapsed: Duration,
) {
    if !is_enabled() {
        return;
    }
    let Ok(executable) = std::env::current_exe() else { return; };
    let _ = std::process::Command::new(executable)
        .env(ENV_CLI_COMPLETION_WORKER, "1")
        .env(ENV_CLI_COMPLETION_COMMAND, fixed_cli_command(command))
        .env(ENV_CLI_COMPLETION_EXIT_CODE, exit_code.to_string())
        .env(ENV_CLI_COMPLETION_DURATION_MS, elapsed.as_millis().to_string())
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
    event_properties.insert("telemetry_schema_version".into(), Value::from(2));
    event_properties
        .entry("product_version")
        .or_insert_with(|| Value::String(current_product_version().into()));
    event_properties.insert("os_family".into(), Value::String(os_family().into()));
    event_properties.insert("os_major".into(), Value::String(os_major()));
    event_properties.insert("arch".into(), Value::String(arch().into()));
    event_properties.insert("is_ci".into(), Value::Bool(is_ci()));
    event_properties.insert("transport".into(), Value::String(transport.as_str().into()));
    event_properties.insert("process_session_id".into(), Value::String(process_session_id()));
    event_properties.insert("id_persisted".into(), Value::Bool(identity.persisted));
    event_properties.insert("$process_person_profile".into(), Value::Bool(false));
    event_properties.insert("$geoip_disable".into(), Value::Bool(true));
    event_properties.insert("$lib".into(), Value::String("cua-driver-rs".into()));
    event_properties.insert("$lib_version".into(), Value::String(current_product_version().into()));

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
        if std::thread::Builder::new().name("cua-telemetry".into()).spawn(task).is_err() {
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
    let mut guard = wait.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    while PENDING_SENDS.load(Ordering::SeqCst) > 0 {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let (next, result) = wait.1.wait_timeout(guard, deadline - now)
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
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_'))
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
    read_config().get(CONFIG_ENABLED_KEY).and_then(Value::as_bool)
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "invalid config path".to_owned())?;
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

fn migrate_legacy_telemetry_home() {
    if std::env::var_os(ENV_TELEMETRY_HOME).is_some() {
        return;
    }
    let Some(root) = home_root() else { return; };
    let legacy = root.join(LEGACY_HOME_SUBDIRECTORY);
    let current = root.join(HOME_SUBDIRECTORY);
    if !legacy.is_dir() { return; }
    let _ = std::fs::create_dir_all(&current);
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
    let value = std::fs::read_to_string(path).ok()?;
    let trimmed = value.trim();
    if uuid::Uuid::parse_str(trimmed).is_ok() {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

fn get_or_create_install_id() -> InstallationIdentity {
    static CACHE: OnceLock<InstallationIdentity> = OnceLock::new();
    CACHE.get_or_init(load_or_create_install_id_uncached).clone()
}

fn load_or_create_install_id_uncached() -> InstallationIdentity {
    if let Some(id) = read_install_id() {
        return InstallationIdentity { id, persisted: true };
    }
    let id = uuid::Uuid::new_v4().to_string();
    let Some(path) = marker_path(TELEMETRY_ID_FILE_NAME) else {
        return InstallationIdentity { id, persisted: false };
    };
    let persisted = path.parent().is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
        && std::fs::write(&path, &id).is_ok();
    InstallationIdentity { id, persisted }
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
    match std::env::var(ENV_INSTALL_CHANNEL).as_deref() {
        Ok("install_script") => "install_script",
        Ok("update_apply") => "update_apply",
        Ok("python_package") => "python_package",
        Ok("manual_binary") => "manual_binary",
        Ok("first_run") => "first_run",
        _ => "first_run",
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
    let raw = os_version();
    let start = raw.find(|character: char| character.is_ascii_digit());
    let Some(start) = start else { return "unknown".into(); };
    raw[start..]
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    let command = std::process::Command::new("sw_vers").arg("-productVersion").output();
    #[cfg(target_os = "windows")]
    let command = std::process::Command::new("cmd").args(["/c", "ver"]).output();
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|contents| contents.lines().find_map(|line| line.strip_prefix("VERSION_ID=").map(str::to_owned)))
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
        "CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS", "GITLAB_CI",
        "JENKINS_URL", "CIRCLECI", "BUILDKITE", "TF_BUILD", "TEAMCITY_VERSION",
        "TRAVIS", "APPVEYOR", "BITBUCKET_BUILD_NUMBER", "CODEBUILD_BUILD_ID",
        "DRONE", "HUDSON_URL", "CI_NAME",
    ];
    VARIABLES.iter().any(|name| std::env::var_os(name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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

    #[test]
    fn precedence_is_environment_then_persisted_then_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|_| {
            assert_eq!(effective_enabled(), (true, "default"));
            set_enabled(false).unwrap();
            assert_eq!(effective_enabled(), (false, "persisted"));
            unsafe { std::env::set_var(ENV_TELEMETRY_ENABLED, "true"); }
            assert_eq!(effective_enabled(), (true, "environment"));
            unsafe { std::env::remove_var(ENV_TELEMETRY_ENABLED); }
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
            unsafe { std::env::set_var(ENV_TELEMETRY_ENABLED, "false"); }
            let mut calls = 0;
            capture_install_with_poster(|_| { calls += 1; Ok(200) });
            assert_eq!(calls, 0);
            assert_eq!(read_install_id().as_deref(), Some(id.as_str()));
            assert!(!home.join(INSTALLATION_RECORDED_FILE_NAME).exists());
            unsafe { std::env::remove_var(ENV_TELEMETRY_ENABLED); }
        });
    }

    #[test]
    fn lifecycle_markers_are_written_only_after_each_2xx() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            let home = root.join(HOME_SUBDIRECTORY);
            let mut calls = 0;
            capture_install_with_poster(|_| {
                calls += 1;
                if calls == 1 { Ok(200) } else { Ok(500) }
            });
            assert!(home.join(INSTALLATION_RECORDED_FILE_NAME).exists());
            assert!(!release_marker_path(current_product_version()).unwrap().exists());

            capture_install_with_poster(|_| Ok(200));
            assert!(release_marker_path(current_product_version()).unwrap().exists());
        });
    }

    #[test]
    fn reset_erases_identity_and_markers_but_preserves_preference() {
        let _guard = ENV_LOCK.lock().unwrap();
        with_isolated_home(|root| {
            set_enabled(false).unwrap();
            let home = root.join(HOME_SUBDIRECTORY);
            std::fs::write(home.join(TELEMETRY_ID_FILE_NAME), uuid::Uuid::new_v4().to_string()).unwrap();
            write_marker(&home.join(INSTALLATION_RECORDED_FILE_NAME)).unwrap();
            write_marker(&release_marker_path("1.2.3").unwrap()).unwrap();
            reset_id().unwrap();
            assert_eq!(persisted_enabled(), Some(false));
            assert!(read_install_id().is_none());
            assert!(!home.join(INSTALLATION_RECORDED_FILE_NAME).exists());
        });
    }

    #[test]
    fn v2_payload_has_exact_content_free_envelope_and_no_client_timestamp() {
        let identity = InstallationIdentity { id: "test-id".into(), persisted: true };
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
            "telemetry_schema_version", "product_version", "os_family", "os_major",
            "arch", "is_ci", "transport", "process_session_id", "id_persisted",
            "$process_person_profile", "$geoip_disable", "$lib", "$lib_version",
            "tool_name", "success",
        ] {
            assert!(properties.contains_key(required), "missing {required}");
        }
        let serialized = serde_json::to_string(&payload).unwrap().to_ascii_lowercase();
        for forbidden in [
            "prompt", "task_text", "arguments", "result_text", "typed_text", "clipboard",
            "screenshot", "accessibility_tree", "window_title", "application_name", "file_path",
            "url", "raw_error", "stack_trace", "initialize_payload",
        ] {
            assert!(!serialized.contains(forbidden), "payload contains {forbidden}: {serialized}");
        }
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
        assert_eq!(strict_release_version("1.2.3-rc.1"), Some("1.2.3-rc.1".into()));
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
