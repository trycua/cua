//! pip-preview — shared types + trait for the experimental
//! picture-in-picture agent preview window.
//!
//! The PiP window is an opt-in, always-on-top floating window that
//! shows what the cua-driver agent just did: a post-action screenshot
//! of the target window plus a one-line label summarising the tool
//! call. It mirrors the architecture used by `cursor-overlay` (shared
//! config/types here, platform-specific renderer in each `platform-*`
//! crate) and the registration pattern used by `cua_driver_core::video`
//! (a `OnceLock` factory set once at startup by `main.rs`).
//!
//! macOS is the first working implementation (NSWindow + NSImageView).
//! Windows + Linux ship as compile-clean stubs whose `start()` returns
//! a clear "not yet implemented" error so the rest of the daemon
//! continues without a PiP window.

use std::sync::OnceLock;

/// Canonical `~/.cua-driver/config.json` path matching what the per-platform
/// `set_config` tools write to. Returns `None` when `$HOME` is unset
/// (sandboxed CI).
pub fn default_config_path() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".cua-driver").join("config.json"))
}

/// Merge a single `key`/`value` into `~/.cua-driver/config.json`,
/// preserving any other keys that are already there. Used by the
/// per-platform `set_config` tools to persist `experimental_pip` /
/// `experimental_pip_geometry` so the next daemon restart picks them up.
pub fn write_config_key(key: &str, value: serde_json::Value) -> Result<(), String> {
    let path = default_config_path().ok_or_else(|| "$HOME is not set".to_string())?;
    let mut json: serde_json::Value = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    json[key] = value;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read `experimental_pip` + `experimental_pip_geometry` from the
/// config file, falling back to defaults when missing or malformed.
/// Surfaced by the per-platform `get_config` tools alongside the
/// in-memory `DriverConfig` fields.
pub fn read_pip_keys_from_file() -> (bool, Option<String>) {
    let path = match default_config_path() {
        Some(p) => p,
        None => return (false, None),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return (false, None),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };
    let enabled = json.get("experimental_pip").and_then(|v| v.as_bool()).unwrap_or(false);
    let geometry = json.get("experimental_pip_geometry")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    (enabled, geometry)
}

/// Geometry of the PiP window, in screen points (top-left origin).
///
/// Parsed from `--experimental-pip-geometry WxH+X+Y`. `x` / `y` are
/// optional; when `None` the platform backend picks a sensible
/// "top-right corner with a small inset" default so a user enabling
/// the feature without any geometry flags still sees a window.
#[derive(Debug, Clone, Copy)]
pub struct PipGeometry {
    pub width: u32,
    pub height: u32,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for PipGeometry {
    fn default() -> Self {
        Self {
            width: 320,
            height: 200,
            x: None,
            y: None,
        }
    }
}

impl PipGeometry {
    /// Parse `WxH` or `WxH+X+Y` (matching the common X11 geometry form).
    /// Returns `None` on any parse failure so the caller can fall back
    /// to defaults without panicking.
    pub fn parse(s: &str) -> Option<Self> {
        // Split off the optional `+X+Y` tail first so the leading
        // `WxH` parses cleanly even when no position is provided.
        let (size, pos): (&str, Option<(i32, i32)>) = match s.find('+') {
            Some(i) => {
                let tail = &s[i + 1..];
                let mut parts = tail.split('+');
                let x = parts.next()?.parse().ok()?;
                let y = parts.next()?.parse().ok()?;
                (&s[..i], Some((x, y)))
            }
            None => (s, None),
        };
        let mut wh = size.split('x');
        let w: u32 = wh.next()?.parse().ok()?;
        let h: u32 = wh.next()?.parse().ok()?;
        Some(Self {
            width: w,
            height: h,
            x: pos.map(|p| p.0),
            y: pos.map(|p| p.1),
        })
    }
}

/// Configuration for the PiP window. Built by `main.rs` from CLI
/// flags and handed to `PipBackendFactory::start`.
#[derive(Debug, Clone)]
pub struct PipConfig {
    /// `--experimental-pip` is on argv. The factory is only consulted
    /// when this is true; the field is kept here so backends that
    /// share a `start()` path can early-return.
    pub enabled: bool,
    pub geometry: PipGeometry,
    /// Window title — kept here so the "experimental" label stays in
    /// one place. Defaults to "cua-driver — agent view (experimental)".
    pub title: String,
}

impl Default for PipConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            geometry: PipGeometry::default(),
            title: "cua-driver — agent view (experimental)".to_owned(),
        }
    }
}

impl PipConfig {
    /// Parse the PiP-related CLI flags out of `std::env::args()`.
    /// Recognised flags (all opt-in):
    /// ```text
    /// --experimental-pip
    /// --pip                        (short alias)
    /// --experimental-pip-geometry  WxH | WxH+X+Y
    /// ```
    /// Unknown flags are ignored so this never conflicts with the
    /// other arg-parser passes (CursorConfig, the subcommand router).
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::parse(&args[1..])
    }

    pub fn parse(args: &[String]) -> Self {
        let mut cfg = PipConfig::default();
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--experimental-pip" | "--pip" => cfg.enabled = true,
                "--experimental-pip-geometry" => {
                    if let Some(geom) = args.get(i + 1).and_then(|s| PipGeometry::parse(s)) {
                        cfg.geometry = geom;
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        cfg
    }

    /// Resolve the config from (in order of precedence, low → high):
    ///
    ///   defaults  →  `~/.cua-driver/config.json` keys
    ///                  (`experimental_pip` bool, `experimental_pip_geometry` string)
    ///              →  CLI flags
    ///
    /// Lets users persist `--experimental-pip` across daemon restarts by
    /// editing `~/.cua-driver/config.json` once, instead of re-running
    /// `claude mcp add` with the flag baked into the args list.
    /// Malformed or missing file falls back to the next layer silently.
    pub fn from_args_and_file(config_path: &std::path::Path) -> Self {
        let mut cfg = PipConfig::default();
        if let Ok(text) = std::fs::read_to_string(config_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(b) = json.get("experimental_pip").and_then(|v| v.as_bool()) {
                    cfg.enabled = b;
                }
                if let Some(s) = json.get("experimental_pip_geometry").and_then(|v| v.as_str()) {
                    if let Some(g) = PipGeometry::parse(s) {
                        cfg.geometry = g;
                    }
                }
            }
        }
        // CLI args override anything in the file.
        let args: Vec<String> = std::env::args().collect();
        let cli = PipConfig::parse(&args[1..]);
        if cli.enabled {
            cfg.enabled = true;
        }
        // CLI geometry only overrides when explicitly passed — detect by
        // diffing against PipGeometry::default() (the parse() entry point
        // returns the default when no flag is present).
        if cli.geometry.width != PipGeometry::default().width
            || cli.geometry.height != PipGeometry::default().height
            || cli.geometry.x.is_some()
            || cli.geometry.y.is_some()
        {
            cfg.geometry = cli.geometry;
        }
        cfg
    }
}

/// A single frame pushed into the PiP window after a tool call lands.
///
/// `png_bytes` are the raw PNG bytes produced by the platform
/// screenshot callback — the same path that powers `screenshot.png`
/// in the recording pipeline, so PiP shows exactly what the recorder
/// sees.
#[derive(Debug, Clone)]
pub struct PipFrame {
    pub png_bytes: Vec<u8>,
    /// One-line summary shown overlayed on the frame, e.g.
    /// `click element_index=2` or `type_text "hello world"`.
    pub action_label: String,
    /// Wall-clock timestamp (ms since Unix epoch) — used by backends
    /// that want to show "last update Xs ago" in the title bar.
    pub timestamp_ms: u64,
}

/// A live PiP window. Owned by `main.rs` for the lifetime of the
/// process; `shutdown()` consumes it and closes the window.
pub trait PipBackend: Send + Sync {
    /// Push a new frame to the window. Non-blocking; the backend is
    /// responsible for dispatching the actual draw to whatever thread
    /// its UI toolkit requires (the macOS impl dispatches to the main
    /// queue via `dispatch_async`).
    fn push_frame(&self, frame: PipFrame);

    /// Close the window and release native resources. Called from
    /// `main.rs` on shutdown.
    fn shutdown(self: Box<Self>);
}

/// Spawns a fresh PiP window. Registered once at startup via
/// `set_pip_backend_factory`.
pub trait PipBackendFactory: Send + Sync {
    fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>>;
}

static PIP_FACTORY: OnceLock<Box<dyn PipBackendFactory>> = OnceLock::new();

/// Register the platform's PiP backend factory. Idempotent — subsequent
/// calls are silently ignored, matching the other startup-callback
/// setters in `cua_driver_core`.
pub fn set_pip_backend_factory(factory: Box<dyn PipBackendFactory>) {
    let _ = PIP_FACTORY.set(factory);
}

/// Start a PiP window using the registered backend. Returns an error
/// when no backend has been registered for this platform — `main.rs`
/// treats that as "PiP unavailable on this OS" and continues without
/// the window.
pub fn start_pip(cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
    let factory = PIP_FACTORY
        .get()
        .ok_or_else(|| anyhow::anyhow!("no PiP backend registered for this platform"))?;
    factory.start(cfg)
}
