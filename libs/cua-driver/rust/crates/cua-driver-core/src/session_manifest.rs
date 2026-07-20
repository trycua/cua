//! Immutable bounded-autonomy manifest loaded at trusted daemon startup.
//!
//! The agent may propose this file, but selecting and approving it is a
//! launcher responsibility. The manifest only narrows the built-in,
//! managed, and user policy layers; it cannot introduce unreviewed tools.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "yaml")]
use serde::Deserialize;
#[cfg(feature = "yaml")]
use sha2::{Digest, Sha256};

pub const SESSION_POLICY_FILE_ENV: &str = "CUA_DRIVER_SESSION_POLICY_FILE";
pub const SESSION_POLICY_APPROVED_ENV: &str = "CUA_DRIVER_SESSION_POLICY_APPROVED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestDecision {
    Allow,
    Deny,
    Ask,
    Undeclared,
}

#[derive(Debug, Clone)]
pub struct SessionManifest {
    sha256: String,
    expires_unix_ms: u128,
    idle_timeout: Duration,
    allow: HashSet<String>,
    deny: HashSet<String>,
    ask: HashSet<String>,
    existing_profiles: HashSet<(i64, u64)>,
    browser_origins: HashSet<String>,
    last_authorized_dispatch: Arc<Mutex<Instant>>,
    idle_expired: Arc<AtomicBool>,
}

impl SessionManifest {
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn expires_unix_ms(&self) -> u128 {
        self.expires_unix_ms
    }

    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.allow.len(), self.deny.len(), self.ask.len())
    }

    pub fn decision(&self, tool: &str) -> ManifestDecision {
        let tool = canonical_tool_name(tool);
        if self.deny.contains(tool) {
            ManifestDecision::Deny
        } else if self.ask.contains(tool) {
            ManifestDecision::Ask
        } else if self.allow.contains(tool) {
            ManifestDecision::Allow
        } else {
            ManifestDecision::Undeclared
        }
    }

    /// Enforce resource fields for adapters that have completed migration.
    /// An allow-list entry for a tool never implies access to an undeclared
    /// browser process, native window, or origin.
    pub fn authorize_call(&self, tool: &str, args: &serde_json::Value) -> Result<(), String> {
        match tool {
            "browser_prepare"
                if args
                    .pointer("/strategy/kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("existing_profile") =>
            {
                let pid = args
                    .get("pid")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| "autonomous existing-profile access requires pid".to_owned())?;
                let window_id = args
                    .get("window_id")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        "autonomous existing-profile access requires window_id".to_owned()
                    })?;
                if !self.existing_profiles.contains(&(pid, window_id)) {
                    return Err(format!(
                        "browser pid {pid} window {window_id} is outside the autonomous session policy"
                    ));
                }
            }
            "browser_navigate" => {
                let url = args
                    .get("url")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "autonomous browser navigation requires url".to_owned())?;
                self.authorize_browser_url(url)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Validate a live top-level document after redirects and before input.
    pub fn authorize_browser_url(&self, raw: &str) -> Result<(), String> {
        let origin = canonical_origin(raw)?;
        if self.browser_origins.contains(&origin) {
            Ok(())
        } else {
            Err(format!(
                "browser origin '{origin}' is outside the autonomous session policy"
            ))
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expires_unix_ms < now_unix_ms()
    }

    pub fn is_idle_expired(&self) -> bool {
        self.idle_expired.load(Ordering::Acquire)
    }

    /// Refresh the autonomous idle lease only after a call passed every
    /// manifest decision and resource check. Once the idle lease expires it is
    /// terminal for this daemon instance; a tool call cannot revive it.
    pub fn authorize_dispatch(&self) -> Result<(), String> {
        if self.is_idle_expired() {
            return Err("autonomous session policy idle timeout exceeded".to_owned());
        }
        let now = Instant::now();
        let mut last = self.last_authorized_dispatch.lock().unwrap();
        if now.duration_since(*last) > self.idle_timeout {
            self.idle_expired.store(true, Ordering::Release);
            return Err("autonomous session policy idle timeout exceeded".to_owned());
        }
        *last = now;
        Ok(())
    }
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    version: u32,
    mode: String,
    expires_after: String,
    idle_timeout: String,
    #[serde(default)]
    resources: RawResources,
    #[serde(default)]
    allow: RawToolSet,
    #[serde(default)]
    deny: RawToolSet,
    #[serde(default)]
    ask: RawToolSet,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResources {
    #[serde(default)]
    browser: RawBrowserResources,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrowserResources {
    #[serde(default)]
    existing_profiles: Vec<RawExistingProfile>,
    #[serde(default)]
    origins: Vec<String>,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExistingProfile {
    pid: i64,
    window_id: u64,
}

#[cfg(feature = "yaml")]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolSet {
    #[serde(default)]
    tools: Vec<String>,
}

static CONFIGURED_MANIFEST: OnceLock<Result<Option<SessionManifest>, String>> = OnceLock::new();

pub fn configured_session_manifest() -> Result<Option<&'static SessionManifest>, String> {
    match CONFIGURED_MANIFEST.get_or_init(load_configured_manifest) {
        Ok(manifest) => Ok(manifest.as_ref()),
        Err(error) => Err(error.clone()),
    }
}

fn load_configured_manifest() -> Result<Option<SessionManifest>, String> {
    let Some(path) = std::env::var_os(SESSION_POLICY_FILE_ENV) else {
        return Ok(None);
    };
    load_manifest(Path::new(&path)).map(Some)
}

pub(crate) fn load_manifest(path: &Path) -> Result<SessionManifest, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read session policy {}: {error}", path.display()))?;
    if bytes.is_empty() {
        return Err(format!("session policy {} is empty", path.display()));
    }
    #[cfg(feature = "yaml")]
    let raw: RawManifest = serde_yaml_ng::from_slice(&bytes)
        .map_err(|error| format!("failed to parse session policy {}: {error}", path.display()))?;
    #[cfg(not(feature = "yaml"))]
    return Err("session policy support requires the yaml feature".to_owned());

    #[cfg(feature = "yaml")]
    {
        if raw.version != 1 {
            return Err(format!(
                "unsupported session policy version {}; expected 1",
                raw.version
            ));
        }
        if raw.mode != "autonomous" {
            return Err("session policy mode must be autonomous".to_owned());
        }
        let expires_after = parse_duration(&raw.expires_after)?;
        let idle_timeout = parse_duration(&raw.idle_timeout)?;
        if expires_after.is_zero() || expires_after > Duration::from_secs(24 * 60 * 60) {
            return Err("expires_after must be greater than zero and no more than 24h".to_owned());
        }
        if idle_timeout.is_zero() || idle_timeout > expires_after {
            return Err(
                "idle_timeout must be greater than zero and no more than expires_after".to_owned(),
            );
        }
        let allow = validate_tools("allow", raw.allow.tools)?;
        let deny = validate_tools("deny", raw.deny.tools)?;
        let ask = validate_tools("ask", raw.ask.tools)?;
        if allow.is_empty() {
            return Err("session policy allow.tools must not be empty".to_owned());
        }
        for tool in &allow {
            if deny.contains(tool) || ask.contains(tool) {
                return Err(format!(
                    "session policy tool '{tool}' appears in more than one decision set"
                ));
            }
        }
        for tool in &deny {
            if ask.contains(tool) {
                return Err(format!(
                    "session policy tool '{tool}' appears in more than one decision set"
                ));
            }
        }
        let mut existing_profiles = HashSet::new();
        for profile in raw.resources.browser.existing_profiles {
            if profile.pid <= 0 || profile.window_id == 0 {
                return Err(
                    "browser existing_profiles entries require positive pid and window_id"
                        .to_owned(),
                );
            }
            if !existing_profiles.insert((profile.pid, profile.window_id)) {
                return Err(format!(
                    "browser existing_profiles repeats pid {} window {}",
                    profile.pid, profile.window_id
                ));
            }
        }
        let mut browser_origins = HashSet::new();
        for raw_origin in raw.resources.browser.origins {
            let origin = canonical_origin(&raw_origin)?;
            if origin != raw_origin.trim().trim_end_matches('/') {
                return Err(format!(
                    "browser origin '{raw_origin}' must be an exact canonical origin without a path"
                ));
            }
            if !browser_origins.insert(origin.clone()) {
                return Err(format!("browser origins repeats '{origin}'"));
            }
        }
        if !browser_origins.is_empty() {
            const ORIGIN_BYPASS_TOOLS: &[&str] = &[
                "page",
                "click",
                "double_click",
                "right_click",
                "drag",
                "scroll",
                "type_text",
                "press_key",
                "hotkey",
                "set_value",
                "mouse_button_down",
                "mouse_button_up",
                "mouse_drag",
                "parallel_mouse_drag",
                "get_accessibility_tree",
                "get_window_state",
                "get_desktop_state",
            ];
            if let Some(tool) = ORIGIN_BYPASS_TOOLS
                .iter()
                .find(|tool| allow.contains(**tool))
            {
                return Err(format!(
                    "origin-scoped autonomous manifests cannot allow '{tool}' because it bypasses the typed browser origin adapter"
                ));
            }
        }

        let mut digest = Sha256::new();
        digest.update(b"cua-driver-session-policy-v1\0");
        digest.update(&bytes);
        Ok(SessionManifest {
            sha256: format!("{:x}", digest.finalize()),
            expires_unix_ms: now_unix_ms() + expires_after.as_millis(),
            idle_timeout,
            allow,
            deny,
            ask,
            existing_profiles,
            browser_origins,
            last_authorized_dispatch: Arc::new(Mutex::new(Instant::now())),
            idle_expired: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[cfg(feature = "yaml")]
fn validate_tools(section: &str, tools: Vec<String>) -> Result<HashSet<String>, String> {
    let mut validated = HashSet::new();
    for tool in tools {
        let canonical = canonical_tool_name(tool.trim()).to_owned();
        if canonical.is_empty()
            || crate::authorization::advertised_risk_for(&canonical).class
                == crate::authorization::RiskClass::Unclassified
        {
            return Err(format!(
                "session policy {section}.tools contains unknown or unreviewed tool '{tool}'"
            ));
        }
        if !validated.insert(canonical.clone()) {
            return Err(format!(
                "session policy {section}.tools repeats '{canonical}'"
            ));
        }
    }
    Ok(validated)
}

fn canonical_tool_name(tool: &str) -> &str {
    match tool {
        "type_text_chars" => "type_text",
        other => other,
    }
}

fn canonical_origin(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value == "about:blank" {
        return Ok(value.to_owned());
    }
    let parsed =
        url::Url::parse(value).map_err(|_| format!("invalid browser URL or origin '{raw}'"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "browser URL or origin '{raw}' must use http or https"
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("browser URL or origin '{raw}' has no host"))?;
    let mut origin = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Ok(origin)
}

#[cfg(any(feature = "yaml", test))]
fn parse_duration(raw: &str) -> Result<Duration, String> {
    let value = raw.trim();
    if value.len() < 2 {
        return Err(format!(
            "invalid duration '{raw}'; use forms such as 30m or 8h"
        ));
    }
    let (number, unit) = value.split_at(value.len() - 1);
    let number = number
        .parse::<u64>()
        .map_err(|_| format!("invalid duration '{raw}'; use forms such as 30m or 8h"))?;
    let seconds = match unit {
        "s" => number,
        "m" => number.saturating_mul(60),
        "h" => number.saturating_mul(60 * 60),
        _ => {
            return Err(format!(
                "invalid duration unit in '{raw}'; expected s, m, or h"
            ))
        }
    };
    Ok(Duration::from_secs(seconds))
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "yaml")]
    fn manifest(source: &str) -> Result<SessionManifest, String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.yaml");
        std::fs::write(&path, source).unwrap();
        load_manifest(&path)
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn manifest_is_deny_by_default_and_preserves_ask() {
        let loaded = manifest(
            r#"
version: 1
mode: autonomous
expires_after: 8h
idle_timeout: 30m
resources: {}
allow:
  tools: [start_session, get_browser_state]
deny:
  tools: [page]
ask:
  tools: [browser_download]
"#,
        )
        .unwrap();
        assert_eq!(loaded.decision("start_session"), ManifestDecision::Allow);
        assert_eq!(loaded.decision("page"), ManifestDecision::Deny);
        assert_eq!(loaded.decision("browser_download"), ManifestDecision::Ask);
        assert_eq!(loaded.decision("click"), ManifestDecision::Undeclared);
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn browser_resources_bind_existing_profile_and_origins() {
        let manifest = manifest(
            r#"
version: 1
mode: autonomous
expires_after: 8h
idle_timeout: 30m
resources:
  browser:
    existing_profiles:
      - pid: 42
        window_id: 7
    origins:
      - https://app.example.com
allow:
  tools: [browser_prepare, browser_navigate]
"#,
        )
        .unwrap();

        manifest
            .authorize_call(
                "browser_prepare",
                &serde_json::json!({
                    "pid": 42,
                    "window_id": 7,
                    "strategy": {"kind": "existing_profile"}
                }),
            )
            .unwrap();
        assert!(manifest
            .authorize_call(
                "browser_prepare",
                &serde_json::json!({
                    "pid": 42,
                    "window_id": 8,
                    "strategy": {"kind": "existing_profile"}
                }),
            )
            .is_err());
        manifest
            .authorize_browser_url("https://app.example.com/work?q=1")
            .unwrap();
        assert!(manifest
            .authorize_browser_url("https://evil.example/redirect")
            .is_err());
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn idle_timeout_is_terminal_and_origin_bypass_tools_are_rejected() {
        let loaded = manifest(
            r#"
version: 1
mode: autonomous
expires_after: 1h
idle_timeout: 1s
resources: {}
allow:
  tools: [get_config]
"#,
        )
        .unwrap();
        *loaded.last_authorized_dispatch.lock().unwrap() =
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
        assert!(loaded.authorize_dispatch().is_err());
        assert!(loaded.is_idle_expired());
        assert!(loaded.authorize_dispatch().is_err());

        assert!(manifest(
            r#"
version: 1
mode: autonomous
expires_after: 1h
idle_timeout: 10m
resources:
  browser:
    origins: [https://app.example.com]
allow:
  tools: [browser_navigate, page]
"#,
        )
        .unwrap_err()
        .contains("bypasses the typed browser origin adapter"));
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn unknown_and_overlapping_tools_are_rejected() {
        let unknown = manifest(
            "version: 1\nmode: autonomous\nexpires_after: 1h\nidle_timeout: 5m\nallow:\n  tools: [future_tool]\n",
        )
        .unwrap_err();
        assert!(unknown.contains("unknown or unreviewed"));

        let overlap = manifest(
            "version: 1\nmode: autonomous\nexpires_after: 1h\nidle_timeout: 5m\nallow:\n  tools: [click]\ndeny:\n  tools: [click]\n",
        )
        .unwrap_err();
        assert!(overlap.contains("more than one decision set"));
    }

    #[test]
    fn duration_parser_is_bounded_and_explicit() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("1d").is_err());
    }
}
