//! Anonymous usage-tracking client — a Rust port of the Swift
//! `TelemetryClient` (`libs/cua-driver/Sources/CuaDriverCore/Telemetry/
//! TelemetryClient.swift`).
//!
//! Same PostHog HTTP-capture pattern, per-install UUID, env-override
//! support, and installation-record-once behavior as the Swift reference.
//!
//! ## Differences from Swift
//!
//! - **Install ID path** is `~/.cua-driver-rs/.telemetry_id` (Swift uses
//!   `~/.cua-driver/.telemetry_id`). Deliberately independent so a user
//!   who opts out of one binary still gets a fresh distinct_id on the other.
//! - **Opt-out env var** is `CUA_DRIVER_RS_TELEMETRY_ENABLED=false` (Swift
//!   uses `CUA_DRIVER_TELEMETRY_ENABLED`). Same reason — opting out of one
//!   port must not silence the other.
//! - **`$lib`** is reported as `cua-driver-rs` so dashboards can split
//!   Rust vs Swift adoption per-event without inspecting `os`/`arch`.
//! - **No persisted config flag.** Swift falls back to a YAML
//!   `telemetryEnabled` setting; Rust honours only the env var (telemetry
//!   defaults to enabled, env-var opt-out).  Matches Rust's existing
//!   config surface — there's no `ConfigStore.loadSync()` analogue yet,
//!   and YAGNI suggests waiting until someone actually requests it.
//!
//! ## Privacy posture (identical to Swift)
//!
//! We send: driver version, OS name, OS version, CPU arch, CI-environment
//! flag, and a stable per-install UUID. We do **NOT** send: usernames,
//! file paths, command arguments, tool args, or anything user-typed.

use std::path::PathBuf;
use std::sync::OnceLock;

// ── Constants ────────────────────────────────────────────────────────────

/// PostHog ingest endpoint (EU region — matches Swift exactly so dashboards
/// aggregate Rust + Swift events cleanly).
const POSTHOG_CAPTURE_URL: &str = "https://eu.i.posthog.com/capture/";

/// Public PostHog project key. Public by design — keys can only ingest
/// events, not read them. Matches Swift `TelemetryClient.Constants.apiKey`.
const POSTHOG_API_KEY: &str = "phc_eSkLnbLxsnYFaXksif1ksbrNzYlJShr35miFLDppF14";

/// `~/.cua-driver-rs/` subdirectory (Rust-specific — Swift uses `.cua-driver`).
const HOME_SUBDIRECTORY: &str = ".cua-driver-rs";

/// Filename inside the home subdirectory holding the per-install UUID.
const TELEMETRY_ID_FILE_NAME: &str = ".telemetry_id";

/// Marker file written after the install event has been recorded once.
const INSTALLATION_RECORDED_FILE_NAME: &str = ".installation_recorded";

/// Env var disabling telemetry. Accepts `0|false|no|off` to disable,
/// `1|true|yes|on` to enable. Default: enabled (matches Swift).
const ENV_TELEMETRY_ENABLED: &str = "CUA_DRIVER_RS_TELEMETRY_ENABLED";

/// Env var enabling debug logging to stderr (mirrors Swift
/// `CUA_DRIVER_TELEMETRY_DEBUG`).
const ENV_TELEMETRY_DEBUG: &str = "CUA_DRIVER_RS_TELEMETRY_DEBUG";

/// Fire-and-forget POST timeout. PostHog ingest must never delay a CLI call.
const POSTHOG_TIMEOUT_SECS: u64 = 3;

// ── Canonical event names ────────────────────────────────────────────────

/// Event names, mirrored 1:1 from Swift `TelemetryEvent`. Keeping the
/// exact strings is load-bearing for dashboards (Rust + Swift aggregate
/// under the same event name).
pub mod event {
    /// One-time install ping. Sent regardless of opt-out (see
    /// `capture_install`); all other events respect the env flag.
    pub const INSTALL: &str = "cua_driver_install";

    // CLI entry points
    pub const MCP: &str = "cua_driver_mcp";
    pub const SERVE: &str = "cua_driver_serve";
    pub const STOP: &str = "cua_driver_stop";
    pub const STATUS: &str = "cua_driver_status";
    pub const CALL: &str = "cua_driver_call";
    pub const LIST_TOOLS: &str = "cua_driver_list_tools";
    pub const DESCRIBE: &str = "cua_driver_describe";
    pub const RECORDING: &str = "cua_driver_recording";
    pub const CONFIG: &str = "cua_driver_config";
    /// Bare-launch (no args) event — Swift uses it from
    /// `runFirstLaunchGUI`. Rust has no GUI surface yet but ships the
    /// constant for future parity.
    #[allow(dead_code)]
    pub const GUI_LAUNCH: &str = "cua_driver_gui_launch";

    /// Prefix for per-MCP-tool events. `API_PREFIX + tool_name` produces
    /// e.g. `cua_driver_api_click`.
    pub const API_PREFIX: &str = "cua_driver_api_";
}

// ── Public API ───────────────────────────────────────────────────────────

/// Whether telemetry is enabled in this process. Env var is the only
/// signal in the Rust port (no persisted config flag — see module docs).
///
/// Defaults to **enabled** when the env var is unset (matches Swift, which
/// defaults `telemetryEnabled` to `true` in its config).
pub fn is_enabled() -> bool {
    parse_env_bool(ENV_TELEMETRY_ENABLED).unwrap_or(true)
}

/// Record a telemetry event. No-op when telemetry is disabled.
///
/// Fire-and-forget: the HTTP POST runs on a `tokio::spawn`-ed background
/// task with a short timeout. The caller never blocks and never sees an
/// error from telemetry — only `tracing::debug!` on failure.
///
/// `properties` is merged on top of the default envelope (version / OS /
/// arch / etc.). Pass `None` for the common case.
pub fn capture(event_name: &str, properties: Option<serde_json::Value>) {
    if !is_enabled() {
        return;
    }
    spawn_capture(event_name.to_owned(), properties, /*bypass_opt_out*/ false);
}

/// Record the one-time `cua_driver_install` event. Guarded by a
/// `.installation_recorded` marker file so it only fires once per install,
/// and **bypasses the opt-out check** — this is the only path that does so.
///
/// Rationale: we count adoption (one ping per install) so the Rust port's
/// install metric is comparable to the Swift port's. Every subsequent
/// event respects the opt-out normally.
///
/// Unlike [`capture`], this path posts **synchronously** so the marker
/// file is only written when the POST actually succeeds. A failed POST
/// (network, PostHog outage, timeout) leaves the marker absent so the
/// next launch retries — without this, a single bad network at install
/// time would silently drop the only adoption signal we have.
pub fn capture_install() {
    capture_install_with_poster(post_to_posthog);
}

/// Internal seam for [`capture_install`] so tests can inject a fake
/// HTTP poster. Returns immediately if the marker already exists or if
/// the HOME directory cannot be resolved.
fn capture_install_with_poster<F>(post: F)
where
    F: FnOnce(&serde_json::Value) -> Result<u16, String>,
{
    let home_dir = match telemetry_home_dir() {
        Some(p) => p,
        None => return,
    };
    let marker_path = home_dir.join(INSTALLATION_RECORDED_FILE_NAME);
    if marker_path.exists() {
        // Already recorded — silent no-op (matches Swift).
        return;
    }

    // Build the payload directly (we still bypass the opt-out check here,
    // see module docs + capture_install rationale) and POST synchronously.
    let distinct_id = get_or_create_install_id();
    let payload = build_payload(event::INSTALL, None, &distinct_id);
    let debug = debug_enabled();
    if debug {
        eprintln!("[telemetry] sending event: {} (sync)", event::INSTALL);
    }

    match post(&payload) {
        Ok(status) if (200..300).contains(&status) => {
            if debug {
                eprintln!("[telemetry] {} status: {status}", event::INSTALL);
            }
        }
        Ok(status) => {
            // Non-2xx: treat as failure so the next launch retries. PostHog
            // returns 200 on accepted capture; anything else (4xx auth /
            // payload error, 5xx outage) is not a success.
            debug_log(format_args!(
                "{} non-success status {status}; marker not written, will retry",
                event::INSTALL
            ));
            return;
        }
        Err(e) => {
            debug_log(format_args!(
                "{} failed: {e}; marker not written, will retry",
                event::INSTALL
            ));
            return;
        }
    }

    // Only reached on HTTP 2xx — persist the marker so subsequent launches
    // skip re-sending. IO errors here are non-fatal (next launch retries).
    if let Err(e) = std::fs::create_dir_all(&home_dir) {
        debug_log(format_args!("failed to create {}: {e}", home_dir.display()));
        return;
    }
    if let Err(e) = std::fs::write(&marker_path, "1") {
        debug_log(format_args!(
            "failed to write install marker {}: {e}",
            marker_path.display()
        ));
    }
}

// ── Internals ────────────────────────────────────────────────────────────

/// Resolve `~/.cua-driver-rs`. Returns `None` only on the platform-impossible
/// case where neither `HOME` (Unix) nor `USERPROFILE` (Windows) is set.
fn telemetry_home_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(HOME_SUBDIRECTORY))
}

/// Read the per-install UUID, creating + persisting a fresh one if absent
/// or unreadable. Idempotent across calls in the same process via
/// `INSTALL_ID_CACHE`.
fn get_or_create_install_id() -> String {
    static INSTALL_ID_CACHE: OnceLock<String> = OnceLock::new();
    INSTALL_ID_CACHE
        .get_or_init(load_or_create_install_id_uncached)
        .clone()
}

/// Disk-bound path. Read existing UUID if valid; otherwise generate and persist.
/// Separated from the `OnceLock` wrapper so tests can exercise the path logic
/// without contaminating process-global state.
fn load_or_create_install_id_uncached() -> String {
    let Some(home_dir) = telemetry_home_dir() else {
        // No HOME — generate an ephemeral UUID. Telemetry will still send,
        // but the distinct_id won't be stable across runs. Acceptable
        // fallback for non-interactive containers.
        return uuid::Uuid::new_v4().to_string();
    };
    let id_path = home_dir.join(TELEMETRY_ID_FILE_NAME);

    // Try existing UUID first.
    if let Ok(existing) = std::fs::read_to_string(&id_path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    // Generate + persist a fresh UUID. Persistence failures are
    // non-fatal — we still return the generated id, just without
    // cross-run stability.
    let new_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = std::fs::create_dir_all(&home_dir) {
        debug_log(format_args!("failed to create {}: {e}", home_dir.display()));
        return new_id;
    }
    if let Err(e) = std::fs::write(&id_path, &new_id) {
        debug_log(format_args!(
            "failed to persist install id {}: {e}",
            id_path.display()
        ));
    }
    new_id
}

/// Build the PostHog event payload. Public-in-crate so unit tests can
/// assert on payload shape without an actual HTTP roundtrip.
pub(crate) fn build_payload(
    event_name: &str,
    properties: Option<&serde_json::Value>,
    distinct_id: &str,
) -> serde_json::Value {
    let version = env!("CARGO_PKG_VERSION");

    let mut event_properties = serde_json::Map::new();
    // Caller-provided properties first, so default-envelope keys win on
    // collision (matches Swift's `eventProperties[key] = value` order).
    if let Some(serde_json::Value::Object(map)) = properties {
        for (k, v) in map {
            event_properties.insert(k.clone(), v.clone());
        }
    }
    event_properties.insert("cua_driver_version".into(), version.into());
    event_properties.insert("os".into(), os_name().into());
    event_properties.insert("os_version".into(), os_version().into());
    event_properties.insert("arch".into(), arch().into());
    event_properties.insert("is_ci".into(), is_ci().into());
    event_properties.insert("$lib".into(), "cua-driver-rs".into());
    event_properties.insert("$lib_version".into(), version.into());

    serde_json::json!({
        "api_key": POSTHOG_API_KEY,
        "event": event_name,
        "distinct_id": distinct_id,
        "properties": event_properties,
        "timestamp": iso8601_now(),
    })
}

/// Spawn the HTTP POST on a background tokio task. Caller returns immediately.
/// Requires a tokio runtime to be active in the current thread; falls back
/// to `std::thread::spawn` otherwise so CLI subcommands that don't build
/// a runtime (e.g. `list-tools`) still get telemetry coverage.
fn spawn_capture(
    event_name: String,
    properties: Option<serde_json::Value>,
    bypass_opt_out: bool,
) {
    if !bypass_opt_out && !is_enabled() {
        return;
    }
    let distinct_id = get_or_create_install_id();
    let payload = build_payload(&event_name, properties.as_ref(), &distinct_id);
    let debug = debug_enabled();

    let task = move || {
        if debug {
            eprintln!("[telemetry] sending event: {event_name}");
        }
        match post_to_posthog(&payload) {
            Ok(status) => {
                if debug {
                    eprintln!("[telemetry] {event_name} status: {status}");
                }
            }
            Err(e) => {
                if debug {
                    eprintln!("[telemetry] {event_name} failed: {e}");
                } else {
                    tracing::debug!(target: "cua_driver::telemetry",
                                    "POST {event_name} failed: {e}");
                }
            }
        }
    };

    // Prefer tokio so the POST shares the runtime's reactor; fall back to
    // a plain OS thread for sync entry-points (CLI subcommands like
    // `list-tools` that don't construct a runtime).
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(task);
    } else {
        // Best-effort: detach a short-lived OS thread. ureq's POST is
        // synchronous and self-contained, so this is safe and simple.
        std::thread::Builder::new()
            .name("cua-telemetry".into())
            .spawn(task)
            .ok();
    }
}

/// Actual ureq POST. Returns the HTTP status code on success. Anything else
/// (network error, 4xx/5xx, timeout) is propagated as `Err`.
fn post_to_posthog(payload: &serde_json::Value) -> Result<u16, String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(POSTHOG_TIMEOUT_SECS)))
        .build()
        .new_agent();

    match agent
        .post(POSTHOG_CAPTURE_URL)
        .header("Content-Type", "application/json")
        .send_json(payload)
    {
        Ok(response) => Ok(response.status().as_u16()),
        Err(e) => Err(e.to_string()),
    }
}

// ── Environment helpers ──────────────────────────────────────────────────

fn parse_env_bool(var: &str) -> Option<bool> {
    let raw = std::env::var(var).ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
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

/// Stringly OS name — `"macos" | "windows" | "linux"` to match Swift's
/// `"macos"` value exactly on darwin.
fn os_name() -> &'static str {
    std::env::consts::OS // already "macos"/"windows"/"linux"
}

/// Reported OS version. Best-effort: returns whatever the OS exposes via
/// the canonical "release" file/registry. Swift uses
/// `ProcessInfo.operatingSystemVersionString` (e.g. "Version 14.5 (Build 23F79)");
/// we approximate with a short string per platform.
fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_owned())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_owned())
    }
    #[cfg(target_os = "linux")]
    {
        // `/etc/os-release` is the freedesktop standard; fall back to "unknown".
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_owned())
            })
            .unwrap_or_else(|| "unknown".to_owned())
    }
    #[cfg(target_os = "windows")]
    {
        // No std API; cmd /c ver is best-effort.
        std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_owned())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unknown".to_owned())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unknown".to_owned()
    }
}

fn arch() -> &'static str {
    // std::env::consts::ARCH returns "aarch64"/"x86_64"/etc.
    // Map "aarch64" → "arm64" so the Rust label matches Swift's
    // (`#if arch(arm64)` → `"arm64"`) and dashboards group correctly.
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

/// True when any well-known CI env var is set. List mirrors Swift exactly.
fn is_ci() -> bool {
    const CI_VARS: &[&str] = &[
        "CI",
        "CONTINUOUS_INTEGRATION",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "JENKINS_URL",
        "CIRCLECI",
    ];
    CI_VARS.iter().any(|v| std::env::var_os(v).is_some())
}

/// ISO-8601 timestamp without bringing in a chrono dependency. PostHog
/// accepts `YYYY-MM-DDTHH:MM:SSZ` (UTC) directly.
fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil-date conversion: 1970-01-01 + secs. Algorithm cribbed from
    // Howard Hinnant's civil_from_days; safe through year 9999.
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_unix(unix_secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = (unix_secs % 86_400) as u32;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    // Howard Hinnant's civil_from_days, shifted to 0000-03-01 era epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (year, m, d, hour, minute, second)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// All env-mutating tests serialise on this lock — `std::env::set_var`
    /// is process-global, parallel tests would race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_env_bool_recognises_canonical_forms() {
        let _g = ENV_LOCK.lock().unwrap();
        for (raw, expected) in [
            ("0", false),
            ("false", false),
            ("FALSE", false),
            ("no", false),
            ("off", false),
            ("1", true),
            ("true", true),
            ("YES", true),
            ("on", true),
        ] {
            unsafe { std::env::set_var("CUA_TELEMETRY_TEST_BOOL", raw); }
            assert_eq!(parse_env_bool("CUA_TELEMETRY_TEST_BOOL"), Some(expected),
                       "raw={raw:?}");
        }
        unsafe { std::env::set_var("CUA_TELEMETRY_TEST_BOOL", "maybe"); }
        assert_eq!(parse_env_bool("CUA_TELEMETRY_TEST_BOOL"), None);
        unsafe { std::env::remove_var("CUA_TELEMETRY_TEST_BOOL"); }
        assert_eq!(parse_env_bool("CUA_TELEMETRY_TEST_BOOL"), None);
    }

    #[test]
    fn is_enabled_defaults_to_true_and_honors_env_opt_out() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var(ENV_TELEMETRY_ENABLED); }
        assert!(is_enabled(), "default must be enabled (Swift parity)");

        unsafe { std::env::set_var(ENV_TELEMETRY_ENABLED, "false"); }
        assert!(!is_enabled(), "explicit false must disable");

        unsafe { std::env::set_var(ENV_TELEMETRY_ENABLED, "0"); }
        assert!(!is_enabled(), "0 must disable");

        unsafe { std::env::set_var(ENV_TELEMETRY_ENABLED, "true"); }
        assert!(is_enabled(), "explicit true must enable");

        unsafe { std::env::remove_var(ENV_TELEMETRY_ENABLED); }
    }

    #[test]
    fn is_ci_detects_known_vars() {
        let _g = ENV_LOCK.lock().unwrap();
        // Snapshot + clear all CI vars; restore at end.
        let saved: Vec<(&str, Option<String>)> = [
            "CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS",
            "GITLAB_CI", "JENKINS_URL", "CIRCLECI",
        ].iter()
            .map(|v| (*v, std::env::var(v).ok()))
            .collect();
        for (v, _) in &saved { unsafe { std::env::remove_var(v); } }

        assert!(!is_ci(), "with no CI vars set, is_ci must be false");

        unsafe { std::env::set_var("GITHUB_ACTIONS", "true"); }
        assert!(is_ci(), "GITHUB_ACTIONS must trigger CI detection");
        unsafe { std::env::remove_var("GITHUB_ACTIONS"); }

        unsafe { std::env::set_var("CIRCLECI", "1"); }
        assert!(is_ci(), "CIRCLECI must trigger CI detection");
        unsafe { std::env::remove_var("CIRCLECI"); }

        // Restore.
        for (v, val) in saved {
            match val {
                Some(s) => unsafe { std::env::set_var(v, s); },
                None => unsafe { std::env::remove_var(v); },
            }
        }
    }

    #[test]
    fn build_payload_contains_required_keys() {
        let payload = build_payload(
            "cua_driver_test",
            Some(&serde_json::json!({"extra_key": "extra_val"})),
            "test-distinct-id",
        );
        // Top-level envelope.
        assert_eq!(payload["api_key"], POSTHOG_API_KEY);
        assert_eq!(payload["event"], "cua_driver_test");
        assert_eq!(payload["distinct_id"], "test-distinct-id");
        assert!(payload["timestamp"].is_string());

        // Properties: default envelope + caller-provided.
        let props = &payload["properties"];
        assert_eq!(props["cua_driver_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(props["$lib"], "cua-driver-rs");
        assert_eq!(props["$lib_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(props["os"], std::env::consts::OS);
        assert_eq!(props["arch"], arch());
        assert!(props["is_ci"].is_boolean());
        assert!(props["os_version"].is_string());
        // Caller-provided property survives the merge.
        assert_eq!(props["extra_key"], "extra_val");

        // Privacy assertions: nothing that looks like a username,
        // file path, or args should be in the payload.
        let serialized = serde_json::to_string(&payload).unwrap();
        for forbidden in &["$user", "username", "home_dir", "cwd", "argv"] {
            assert!(!serialized.contains(forbidden),
                    "payload must not contain {forbidden}: {serialized}");
        }
    }

    #[test]
    fn build_payload_default_envelope_wins_on_key_collision() {
        // If a caller tries to override a default-envelope key, the
        // default-envelope value must win (mirrors Swift's insertion order).
        let payload = build_payload(
            "cua_driver_test",
            Some(&serde_json::json!({"os": "fake-os", "$lib": "fake-lib"})),
            "id",
        );
        assert_eq!(payload["properties"]["os"], std::env::consts::OS);
        assert_eq!(payload["properties"]["$lib"], "cua-driver-rs");
    }

    #[test]
    fn install_id_persists_across_reads() {
        let _g = ENV_LOCK.lock().unwrap();
        // Redirect HOME to a temp dir so we don't touch the real
        // `~/.cua-driver-rs`.
        let tmp = std::env::temp_dir().join(format!(
            "cua-driver-rs-telemetry-test-{}", uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let saved_home = std::env::var_os("HOME");
        let saved_userprofile = std::env::var_os("USERPROFILE");
        unsafe { std::env::set_var("HOME", &tmp); }
        unsafe { std::env::set_var("USERPROFILE", &tmp); }

        let first = load_or_create_install_id_uncached();
        assert!(!first.is_empty());
        // UUID v4 is 36 chars (8-4-4-4-12). Sanity check.
        assert_eq!(first.len(), 36);

        // Second call must read from disk and return the same UUID.
        let second = load_or_create_install_id_uncached();
        assert_eq!(first, second, "install id must be stable across reads");

        // File must exist on disk and contain the UUID.
        let id_file = tmp.join(HOME_SUBDIRECTORY).join(TELEMETRY_ID_FILE_NAME);
        let on_disk = std::fs::read_to_string(&id_file).unwrap();
        assert_eq!(on_disk.trim(), first);

        // Cleanup.
        let _ = std::fs::remove_dir_all(&tmp);
        match saved_home {
            Some(s) => unsafe { std::env::set_var("HOME", s); },
            None => unsafe { std::env::remove_var("HOME"); },
        }
        match saved_userprofile {
            Some(s) => unsafe { std::env::set_var("USERPROFILE", s); },
            None => unsafe { std::env::remove_var("USERPROFILE"); },
        }
    }

    #[test]
    fn iso8601_now_is_well_formed() {
        let s = iso8601_now();
        // YYYY-MM-DDTHH:MM:SSZ = 20 chars.
        assert_eq!(s.len(), 20, "got {s:?}");
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert_eq!(&s[10..11], "T");
        assert_eq!(&s[13..14], ":");
        assert_eq!(&s[16..17], ":");
    }

    #[test]
    fn arch_maps_aarch64_to_arm64() {
        // Smoke: just confirm we don't return "aarch64" on aarch64 hosts.
        let a = arch();
        assert_ne!(a, "aarch64", "arm64 dashboards expect 'arm64' not 'aarch64'");
    }

    /// Redirect HOME/USERPROFILE to a fresh temp dir for the duration of
    /// the closure, then restore. Used by `capture_install_*` tests so
    /// they don't pollute the real `~/.cua-driver-rs`.
    fn with_isolated_home<R>(test: impl FnOnce(&std::path::Path) -> R) -> R {
        let tmp = std::env::temp_dir().join(format!(
            "cua-driver-rs-install-test-{}", uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let saved_home = std::env::var_os("HOME");
        let saved_userprofile = std::env::var_os("USERPROFILE");
        unsafe { std::env::set_var("HOME", &tmp); }
        unsafe { std::env::set_var("USERPROFILE", &tmp); }

        let result = test(&tmp);

        let _ = std::fs::remove_dir_all(&tmp);
        match saved_home {
            Some(s) => unsafe { std::env::set_var("HOME", s); },
            None => unsafe { std::env::remove_var("HOME"); },
        }
        match saved_userprofile {
            Some(s) => unsafe { std::env::set_var("USERPROFILE", s); },
            None => unsafe { std::env::remove_var("USERPROFILE"); },
        }
        result
    }

    #[test]
    fn capture_install_does_not_write_marker_when_post_fails() {
        // The whole point of the sync install path: if the POST fails,
        // the marker must NOT be written, so the next launch retries.
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|home| {
            let marker = home.join(HOME_SUBDIRECTORY).join(INSTALLATION_RECORDED_FILE_NAME);
            assert!(!marker.exists(), "precondition: marker must not exist");

            capture_install_with_poster(|_payload| {
                Err("simulated network failure".to_owned())
            });

            assert!(!marker.exists(),
                "marker must NOT be written when POST returns Err");
        });
    }

    #[test]
    fn capture_install_does_not_write_marker_when_post_returns_non_2xx() {
        // PostHog returning e.g. 500 must also be treated as failure.
        let _g = ENV_LOCK.lock().unwrap();
        with_isolated_home(|home| {
            let marker = home.join(HOME_SUBDIRECTORY).join(INSTALLATION_RECORDED_FILE_NAME);

            capture_install_with_poster(|_payload| Ok(500u16));
            assert!(!marker.exists(),
                "marker must NOT be written on 5xx response");

            capture_install_with_poster(|_payload| Ok(401u16));
            assert!(!marker.exists(),
                "marker must NOT be written on 4xx response");
        });
    }

}
