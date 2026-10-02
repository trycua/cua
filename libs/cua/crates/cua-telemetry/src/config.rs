//! The switches, and where telemetry state lives.
//!
//! One decision for every Cua program, highest precedence first:
//!
//! 1. `DO_NOT_TRACK` set to anything but `0` or empty: off.
//! 2. `CUA_TELEMETRY`: `0/false/no/off` off, `1/true/yes/on` on.
//! 3. Legacy switches, off only: `CUA_TELEMETRY_ENABLED=false`,
//!    `CUA_TELEMETRY_DISABLED=1`.
//! 4. `$CUA_HOME/config.toml`, `[telemetry] enabled = "off"` (what
//!    `cua config set telemetry off`, `cua telemetry off` and the Spaces app's
//!    Settings write).
//! 5. A CI environment: off (turn it on with `CUA_TELEMETRY=1`; events are
//!    then marked `is_ci`).
//! 6. Otherwise on.
//!
//! State lives in `$CUA_HOME/telemetry/` (default `~/.cua/telemetry/`).

use std::path::{Path, PathBuf};

/// The environment variable that forces telemetry on or off.
pub const ENV_TELEMETRY: &str = "CUA_TELEMETRY";
/// The cross-tool opt-out (<https://consoledonottrack.com>).
pub const ENV_DO_NOT_TRACK: &str = "DO_NOT_TRACK";
/// Legacy switch (Python and TypeScript SDKs, cua-driver).
pub const ENV_LEGACY_ENABLED: &str = "CUA_TELEMETRY_ENABLED";
/// Legacy switch (Python SDK).
pub const ENV_LEGACY_DISABLED: &str = "CUA_TELEMETRY_DISABLED";
/// Marks events sent by Cua's own tests and dogfooding.
pub const ENV_SYNTHETIC: &str = "CUA_TELEMETRY_SYNTHETIC";
/// Print every payload to stderr as it is queued.
pub const ENV_DEBUG: &str = "CUA_TELEMETRY_DEBUG";
/// Override the ingest endpoint (self-hosting, tests).
pub const ENV_ENDPOINT: &str = "CUA_TELEMETRY_ENDPOINT";
/// Refuse every non-loopback send (set for all test runs).
pub const ENV_FORBID_NETWORK: &str = "CUA_TELEMETRY_FORBID_NETWORK";
/// How Cua was installed (set by installers).
pub const ENV_INSTALL_CHANNEL: &str = "CUA_INSTALL_CHANNEL";
/// File in the telemetry state dir where install.sh / install.ps1 record the
/// install channel (`install_script`) for the first-run event.
pub const INSTALL_CHANNEL_FILE: &str = "install_channel";

/// Environment variables that identify a CI environment.
pub const CI_VARIABLES: &[&str] = &[
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
];

/// Where the effective setting came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `DO_NOT_TRACK`.
    DoNotTrack,
    /// `CUA_TELEMETRY`.
    Env,
    /// `CUA_TELEMETRY_ENABLED` / `CUA_TELEMETRY_DISABLED`.
    LegacyEnv(&'static str),
    /// `$CUA_HOME/config.toml`.
    Config(PathBuf),
    /// A CI environment.
    Ci,
    /// The built-in default.
    Default,
}

impl Source {
    /// Short name (`do_not_track`, `env`, `legacy_env`, `config`, `ci`,
    /// `default`).
    pub fn kind(&self) -> &'static str {
        match self {
            Source::DoNotTrack => "do_not_track",
            Source::Env => "env",
            Source::LegacyEnv(_) => "legacy_env",
            Source::Config(_) => "config",
            Source::Ci => "ci",
            Source::Default => "default",
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::DoNotTrack => f.write_str("env DO_NOT_TRACK"),
            Source::Env => f.write_str("env CUA_TELEMETRY"),
            Source::LegacyEnv(v) => write!(f, "env {v}"),
            Source::Config(p) => write!(f, "config {}", p.display()),
            Source::Ci => f.write_str("CI environment (set CUA_TELEMETRY=1 to send)"),
            Source::Default => f.write_str("default"),
        }
    }
}

/// The effective decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Whether usage events may be sent.
    pub enabled: bool,
    /// Why.
    pub source: Source,
    /// Running in CI.
    pub is_ci: bool,
}

/// An environment lookup (the process environment, or a map in tests).
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// `0/false/no/off` → false, `1/true/yes/on` → true.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" | "disable" | "disabled" => Some(false),
        "1" | "true" | "yes" | "on" | "enable" | "enabled" => Some(true),
        _ => None,
    }
}

/// Whether the environment is a CI system.
pub fn is_ci(env: Env<'_>) -> bool {
    CI_VARIABLES.iter().any(|v| {
        env(v).is_some_and(|s| {
            let s = s.trim();
            !s.is_empty() && !s.eq_ignore_ascii_case("false") && s != "0"
        })
    })
}

/// The cua home: `$CUA_HOME`, else `~/.cua`.
pub fn cua_home(env: Env<'_>) -> Option<PathBuf> {
    if let Some(h) = env("CUA_HOME").filter(|h| !h.trim().is_empty()) {
        return Some(PathBuf::from(h));
    }
    env("HOME")
        .or_else(|| env("USERPROFILE"))
        .filter(|h| !h.trim().is_empty())
        .map(|h| PathBuf::from(h).join(".cua"))
}

/// `$CUA_HOME/config.toml`.
pub fn config_path(home: &Path) -> PathBuf {
    home.join("config.toml")
}

/// `$CUA_HOME/telemetry`.
pub fn state_dir(home: &Path) -> PathBuf {
    home.join("telemetry")
}

/// The `[telemetry] enabled` value in the config file, if set.
pub fn config_value(home: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(config_path(home)).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let v = doc.get("telemetry")?.get("enabled")?.as_value()?;
    match v {
        toml_edit::Value::Boolean(b) => Some(*b.value()),
        toml_edit::Value::String(s) => parse_bool(s.value()),
        toml_edit::Value::Integer(i) => Some(*i.value() != 0),
        _ => None,
    }
}

/// Writes `[telemetry] enabled = "on"|"off"` to the config file, keeping
/// everything else in it.
pub fn write_config_value(home: &Path, enabled: bool) -> std::io::Result<PathBuf> {
    let path = config_path(home);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    if !doc.contains_table("telemetry") {
        doc["telemetry"] = toml_edit::table();
    }
    doc["telemetry"]["enabled"] = toml_edit::value(if enabled { "on" } else { "off" });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::fsutil::write_atomic(&path, doc.to_string().as_bytes())?;
    Ok(path)
}

/// The effective decision for `env` and the config under `home`.
pub fn decide(env: Env<'_>, home: Option<&Path>) -> Decision {
    let ci = is_ci(env);
    let d = |enabled, source| Decision {
        enabled,
        source,
        is_ci: ci,
    };
    if env(ENV_DO_NOT_TRACK).is_some_and(|v| {
        let v = v.trim();
        !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
    }) {
        return d(false, Source::DoNotTrack);
    }
    if let Some(b) = env(ENV_TELEMETRY).as_deref().and_then(parse_bool) {
        return d(b, Source::Env);
    }
    if env(ENV_LEGACY_ENABLED).as_deref().and_then(parse_bool) == Some(false) {
        return d(false, Source::LegacyEnv(ENV_LEGACY_ENABLED));
    }
    if env(ENV_LEGACY_DISABLED).as_deref().and_then(parse_bool) == Some(true) {
        return d(false, Source::LegacyEnv(ENV_LEGACY_DISABLED));
    }
    if let Some(home) = home
        && let Some(b) = config_value(home)
    {
        return d(b, Source::Config(config_path(home)));
    }
    if ci {
        return d(false, Source::Ci);
    }
    d(true, Source::Default)
}

/// Whether sends to non-loopback hosts are forbidden (tests).
pub fn network_forbidden(env: Env<'_>) -> bool {
    cfg!(test)
        || env(ENV_FORBID_NETWORK)
            .as_deref()
            .and_then(parse_bool)
            .unwrap_or(false)
}
