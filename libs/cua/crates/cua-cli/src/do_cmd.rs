//! `cua do`: one-shot computer actions against a selected target, for
//! agents. Output is one line starting with ✅ or ❌, then a context line
//! (`💻 target  🔍 zoom`). The target and zoom state persist in
//! `~/.cua/do_target.json`; coordinates are in screenshot-image space and
//! are mapped to screen points with the last screenshot's scale and origin.
//!
//! Targets are SDK sandboxes (Fleet, local or direct). `host` is a direct
//! sandbox pointing at a cua-spacesd on this machine (default
//! `http://127.0.0.1:3211`), gated behind `cua do-host-consent`.

use crate::{
    computer::{self, Computer, MAX_LENGTH, split_keys},
    trajectory,
    util::{self, internal},
};
use clap::{Args, Subcommand};
use cua_sdk::{Cua, CuaError, SandboxCreateOptions};
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf, sync::Arc};

/// Legacy provider words accepted by `cua do switch <provider> <name>`.
const LEGACY_PROVIDERS: &[&str] = &[
    "cloud",
    "cloudv2",
    "local",
    "lume",
    "lumier",
    "docker",
    "winsandbox",
    "fleet",
    "direct",
    "sandbox",
];
/// Default host spacesd (`CUA_HOST_ENV_URL`).
pub const DEFAULT_HOST_ENV_URL: &str = "http://127.0.0.1:3211";

#[derive(Args, Debug)]
pub struct DoArgs {
    /// Disable trajectory recording for this command.
    #[arg(long)]
    no_record: bool,
    #[command(subcommand)]
    action: DoAction,
}

#[derive(Subcommand, Debug)]
pub enum DoAction {
    /// Select the target: a sandbox name, `host`, `url <URL>`, or a legacy
    /// `<provider> <name>` pair.
    #[command(after_help = "Examples:
  cua do switch dev
  cua do switch cloud:dev
  # This machine (after `cua do-host-consent`)
  cua do switch host")]
    Switch {
        /// Sandbox name or ref, `host`, `url`, or a legacy provider word.
        target: String,
        /// The URL after `url`, or the sandbox name after a provider word.
        name: Option<String>,
        /// spacesd token (`url` and `host`).
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Sandbox name to register a `url` target under.
        #[arg(long = "as")]
        alias: Option<String>,
    },
    /// Show the current target and zoom state.
    #[command(after_help = "Examples:
  cua do status")]
    Status,
    /// List targets (optionally of one provider).
    #[command(after_help = "Examples:
  cua do ls")]
    Ls {
        /// Only targets of this provider.
        provider: Option<String>,
    },
    /// Crop screenshots to a window and map coordinates into it.
    #[command(after_help = "Examples:
  cua do zoom Firefox")]
    Zoom {
        /// Window title or app name.
        window_name: String,
    },
    /// Return to full-screen screenshots.
    #[command(after_help = "Examples:
  cua do unzoom")]
    Unzoom,
    /// Take a screenshot (saved to ~/.cua/screenshots, newest 20 kept, unless --save).
    #[command(after_help = "Examples:
  cua do screenshot
  cua do screenshot --save desktop.png")]
    Screenshot {
        /// Save the PNG here.
        #[arg(short, long)]
        save: Option<PathBuf>,
    },
    /// Screenshot plus an AI summary of the screen and its interactive
    /// elements (needs ANTHROPIC_API_KEY).
    #[command(after_help = "Examples:
  cua do snapshot
  cua do snapshot find the search box")]
    Snapshot {
        /// What to focus the summary on.
        instructions: Vec<String>,
        /// Model (default `claude-haiku-4-5`, or CUA_SNAPSHOT_MODEL).
        #[arg(long)]
        model: Option<String>,
    },
    /// Click at image coordinates.
    #[command(after_help = "Examples:
  cua do click 640 360
  cua do click 640 360 right")]
    Click {
        /// X in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        x: i64,
        /// Y in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        y: i64,
        /// Mouse button.
        #[arg(default_value = "left", value_parser = ["left", "right", "middle"])]
        button: String,
    },
    /// Double-click at image coordinates.
    #[command(after_help = "Examples:
  cua do dclick 640 360")]
    Dclick {
        /// X in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        x: i64,
        /// Y in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        y: i64,
    },
    /// Move the cursor (image coordinates).
    #[command(after_help = "Examples:
  cua do move 640 360")]
    Move {
        /// X in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        x: i64,
        /// Y in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        y: i64,
    },
    /// Type text.
    #[command(after_help = "Examples:
  cua do type 'hello world'")]
    Type {
        /// Text to type.
        text: String,
    },
    /// Press a key (enter, escape, tab, ...).
    #[command(after_help = "Examples:
  cua do key enter")]
    Key {
        /// Key name.
        key: String,
    },
    /// Keyboard shortcut (cmd+c, ctrl+shift+s).
    #[command(after_help = "Examples:
  cua do hotkey ctrl+shift+s")]
    Hotkey {
        /// Keys joined with `+`.
        keys: String,
    },
    /// Scroll in a direction.
    #[command(after_help = "Examples:
  cua do scroll down
  cua do scroll up 10")]
    Scroll {
        /// Direction.
        #[arg(value_parser = ["up", "down", "left", "right"])]
        direction: String,
        /// Scroll steps.
        #[arg(default_value_t = 3)]
        amount: i64,
    },
    /// Drag between two points (image coordinates).
    #[command(after_help = "Examples:
  cua do drag 100 200 400 200")]
    Drag {
        /// Start X in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        x1: i64,
        /// Start Y in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        y1: i64,
        /// End X in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        x2: i64,
        /// End Y in screenshot pixels.
        #[arg(allow_negative_numbers = true)]
        y2: i64,
    },
    /// Run a shell command, or open an interactive terminal (PTY) when no
    /// command is given on a terminal.
    #[command(after_help = "Examples:
  cua do shell ls -la /tmp
  # An interactive terminal
  cua do shell")]
    Shell {
        /// Terminal width (default: this terminal's).
        #[arg(long)]
        cols: Option<u32>,
        /// Terminal height (default: this terminal's).
        #[arg(long)]
        rows: Option<u32>,
        /// Command and arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Open a file or URL.
    #[command(after_help = "Examples:
  cua do open https://example.com")]
    Open {
        /// File path or URL in the target.
        path: String,
    },
    /// Launch an application.
    #[command(after_help = "Examples:
  cua do launch firefox
  cua do launch firefox --private-window")]
    Launch {
        /// Application name or path.
        app: String,
        /// Arguments passed to the application.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Window management.
    #[command(subcommand)]
    Window(WindowAction),
    /// Accessibility tree and actions.
    #[command(subcommand, name = "a11y")]
    A11y(A11yAction),
    /// Print the cursor position (screen points).
    #[command(after_help = "Examples:
  cua do cursor")]
    Cursor,
    /// Clipboard text.
    #[command(subcommand)]
    Clipboard(ClipboardAction),
}

#[derive(Subcommand, Debug)]
pub enum WindowAction {
    /// List windows (optionally filtered by app or title).
    #[command(after_help = "Examples:
  cua do window ls
  cua do window ls firefox")]
    Ls {
        /// Only windows whose app or title contains this.
        #[arg(default_value = "")]
        app: String,
    },
    /// Remove focus from the current window (presses Escape).
    #[command(after_help = "Examples:
  cua do window unfocus")]
    Unfocus,
    /// Focus a window.
    #[command(
        alias = "activate",
        after_help = "Examples:
  cua do window focus <window-id>"
    )]
    Focus {
        /// Window id (from `cua do window ls`).
        window_id: String,
    },
    /// Minimize a window.
    #[command(after_help = "Examples:
  cua do window minimize <window-id>")]
    Minimize {
        /// Window id (from `cua do window ls`).
        window_id: String,
    },
    /// Maximize a window.
    #[command(after_help = "Examples:
  cua do window maximize <window-id>")]
    Maximize {
        /// Window id (from `cua do window ls`).
        window_id: String,
    },
    /// Restore a minimized or maximized window.
    #[command(after_help = "Examples:
  cua do window restore <window-id>")]
    Restore {
        /// Window id (from `cua do window ls`).
        window_id: String,
    },
    /// Close a window.
    #[command(after_help = "Examples:
  cua do window close <window-id>")]
    Close {
        /// Window id (from `cua do window ls`).
        window_id: String,
    },
    /// Resize a window.
    #[command(after_help = "Examples:
  cua do window resize <window-id> 1280 800")]
    Resize {
        /// Window id (from `cua do window ls`).
        window_id: String,
        /// Width in screen points.
        width: u32,
        /// Height in screen points.
        height: u32,
    },
    /// Move a window.
    #[command(after_help = "Examples:
  cua do window move <window-id> 0 0")]
    Move {
        /// Window id (from `cua do window ls`).
        window_id: String,
        /// Left edge in screen points.
        #[arg(allow_negative_numbers = true)]
        x: i64,
        /// Top edge in screen points.
        #[arg(allow_negative_numbers = true)]
        y: i64,
    },
    /// Show a window as JSON.
    #[command(after_help = "Examples:
  cua do window info <window-id>")]
    Info {
        /// Window id (from `cua do window ls`).
        window_id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum A11yAction {
    /// Print the accessibility tree (JSON).
    #[command(after_help = "Examples:
  cua do a11y tree
  cua do a11y tree --window <window-id> --depth 4")]
    Tree {
        /// Only this window (default: the focused one).
        #[arg(long)]
        window: Option<String>,
        /// Maximum depth (0: unlimited).
        #[arg(long, default_value_t = 0)]
        depth: u32,
    },
    /// Find elements by name (and role).
    #[command(after_help = "Examples:
  cua do a11y find Save --role button")]
    Find {
        /// Element name (substring).
        name: String,
        /// Only elements with this role.
        #[arg(long, default_value = "")]
        role: String,
        /// Only this window (default: the focused one).
        #[arg(long)]
        window: Option<String>,
    },
    /// Act on an element: press, focus, set-value, increment, ...
    #[command(after_help = "Examples:
  cua do a11y act <element-id>
  cua do a11y act <element-id> set-value 'hello'")]
    Act {
        /// Element id from `tree` or `find`.
        element: String,
        /// Action.
        #[arg(default_value = "press")]
        action: String,
        /// Value for `set-value`.
        #[arg(default_value = "")]
        value: String,
        /// Snapshot id from `tree`/`find` (default: the latest).
        #[arg(long, default_value = "")]
        snapshot: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ClipboardAction {
    /// Print clipboard text.
    #[command(after_help = "Examples:
  cua do clipboard get")]
    Get,
    /// Set clipboard text.
    #[command(after_help = "Examples:
  cua do clipboard set 'hello'")]
    Set {
        /// Text to put on the clipboard.
        text: String,
    },
}

// ------------------------------------------------------------------ state

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    zoom_window: Option<String>,
    #[serde(default)]
    zoom_window_id: Option<String>,
    #[serde(default = "one")]
    zoom_scale: f64,
    #[serde(default)]
    zoom_origin: Option<[f64; 2]>,
    #[serde(default)]
    trajectory_session: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

/// How long an input action waits for the desktop session (`cua do`).
const DESKTOP_WAIT: std::time::Duration = std::time::Duration::from_secs(90);

/// How long `cua do launch` waits for the new window to focus it.
const LAUNCH_FOCUS_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

impl DoAction {
    /// Actions that deliver input or open windows: they wait for the
    /// desktop session first.
    fn needs_desktop(&self) -> bool {
        matches!(
            self,
            DoAction::Click { .. }
                | DoAction::Dclick { .. }
                | DoAction::Move { .. }
                | DoAction::Type { .. }
                | DoAction::Key { .. }
                | DoAction::Hotkey { .. }
                | DoAction::Scroll { .. }
                | DoAction::Drag { .. }
                | DoAction::Launch { .. }
                | DoAction::Open { .. }
        )
    }
}

fn one() -> f64 {
    1.0
}

fn state_path() -> PathBuf {
    util::cua_home().join("do_target.json")
}

fn consent_path() -> PathBuf {
    util::cua_home().join("host_consented")
}

impl State {
    fn load() -> Self {
        std::fs::read_to_string(state_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) -> Result<(), CuaError> {
        let p = state_path();
        util::ensure_parent(&p)?;
        std::fs::write(&p, serde_json::to_vec_pretty(self)?).map_err(internal)
    }

    fn reset_zoom(&mut self) {
        self.zoom_window = None;
        self.zoom_window_id = None;
        self.zoom_scale = 1.0;
        self.zoom_origin = None;
    }

    fn to_screen(&self, x: i64, y: i64) -> (f64, f64) {
        let o = self.zoom_origin.unwrap_or([0.0, 0.0]);
        let (sx, sy) = computer::map_point((x as f64, y as f64), self.zoom_scale, (o[0], o[1]));
        (sx.round(), sy.round())
    }

    fn context(&self) -> String {
        let label = if self.name.is_empty() {
            self.provider.clone().unwrap_or_default()
        } else {
            self.name.clone()
        };
        let zoom = match (&self.zoom_window, &self.zoom_window_id) {
            (Some(w), Some(id)) => format!("zoom: {w} ({id})"),
            (Some(w), None) => format!("zoom: {w}"),
            _ => "zoom: off".into(),
        };
        format!("💻 {label}\t🔍 {zoom}")
    }
}

// ------------------------------------------------------------------- run

struct Run<'a> {
    cua: &'a Arc<Cua>,
    state: State,
    record: bool,
}

fn ok(out: &mut dyn Write, msg: impl AsRef<str>) {
    util::line(out, format!("✅ {}", msg.as_ref()));
}

/// `cua do-host-consent`: records consent and switches to the host.
pub async fn host_consent(cua: &Arc<Cua>, out: &mut dyn Write) -> Result<i32, CuaError> {
    let p = consent_path();
    util::ensure_parent(&p)?;
    std::fs::write(&p, "consented").map_err(internal)?;
    let mut r = Run {
        cua,
        state: State::load(),
        record: true,
    };
    r.switch("host".into(), None, None, None, out).await
}

/// Entry point.
pub async fn run(cua: &Arc<Cua>, args: DoArgs, out: &mut dyn Write) -> Result<i32, CuaError> {
    let mut r = Run {
        cua,
        state: State::load(),
        record: !args.no_record,
    };
    match args.action {
        DoAction::Switch {
            target,
            name,
            token,
            alias,
        } => r.switch(target, name, token, alias, out).await,
        DoAction::Status => {
            let Some(p) = r.state.provider.clone() else {
                return fail("No target selected. Run: cua do switch <sandbox>");
            };
            let label = if r.state.name.is_empty() {
                p
            } else {
                format!("{p}/{}", r.state.name)
            };
            let zoom = r
                .state
                .zoom_window
                .as_ref()
                .map(|z| format!(" [zoomed to '{z}']"))
                .unwrap_or_default();
            ok(out, format!("Current target: {label}{zoom}"));
            Ok(0)
        }
        DoAction::Ls { provider } => r.ls(provider, out).await,
        DoAction::Unzoom => {
            r.state.reset_zoom();
            r.state.save()?;
            ok(out, "Unzoomed — full screen");
            Ok(0)
        }
        action => {
            if r.state.provider.is_none() {
                return fail("No target selected. Run: cua do switch <sandbox>");
            }
            let res = r.act(action, out).await;
            match res {
                Ok(Some(msg)) => {
                    ok(out, msg);
                    util::line(out, r.state.context());
                    Ok(0)
                }
                Ok(None) => Ok(0),
                Err(Exit(code, msg)) => {
                    if !msg.is_empty() {
                        eprintln!("❌ {msg}");
                        util::line(out, r.state.context());
                    }
                    Ok(code)
                }
            }
        }
    }
}

fn fail(msg: &str) -> Result<i32, CuaError> {
    eprintln!("❌ {msg}");
    Ok(1)
}

/// A failed action: exit code and message.
struct Exit(i32, String);

impl From<CuaError> for Exit {
    fn from(e: CuaError) -> Self {
        Exit(1, e.to_string())
    }
}

impl Run<'_> {
    async fn switch(
        &mut self,
        target: String,
        name: Option<String>,
        token: Option<String>,
        alias: Option<String>,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError> {
        let had_zoom = self.state.zoom_window.is_some();
        let (provider, sandbox) = match target.to_ascii_lowercase().as_str() {
            "host" => {
                if !consent_path().exists() {
                    eprintln!(
                        "❌ Warning: you are about to allow an AI to control your host PC directly.\n   This grants full keyboard, mouse, and screen access to your local desktop.\n   To continue, please run: cua do-host-consent"
                    );
                    return Ok(1);
                }
                let url = name
                    .or_else(|| std::env::var("CUA_HOST_ENV_URL").ok())
                    .unwrap_or_else(|| DEFAULT_HOST_ENV_URL.into());
                let token = token.or_else(|| std::env::var("CUA_HOST_ENV_TOKEN").ok());
                self.register_direct("host", &url, token).await?;
                ("host".to_string(), "host".to_string())
            }
            "url" => {
                let Some(url) = name else {
                    return fail("Usage: cua do switch url <URL> [--token T] [--as NAME]");
                };
                let n = alias.unwrap_or_else(|| direct_name(&url));
                self.register_direct(&n, &url, token).await?;
                ("direct".to_string(), n)
            }
            p if LEGACY_PROVIDERS.contains(&p) => {
                let Some(n) = name.filter(|n| !n.is_empty()) else {
                    return fail(&format!("Usage: cua do switch {p} <sandbox-name>"));
                };
                self.cua.sandboxes().get(n.clone()).await?;
                (p.to_string(), n)
            }
            _ => {
                self.cua.sandboxes().get(target.clone()).await?;
                ("sandbox".to_string(), target)
            }
        };
        self.state.provider = Some(provider.clone());
        self.state.name = sandbox.clone();
        self.state.reset_zoom();
        self.state.trajectory_session = None;
        self.state.save()?;
        let mut msg = if provider == "host" {
            "Switched to host (local PC)".to_string()
        } else {
            format!("Switched to {sandbox}")
        };
        if had_zoom {
            msg.push_str(" — zoom reset");
        }
        ok(out, msg);
        Ok(0)
    }

    async fn register_direct(
        &self,
        name: &str,
        url: &str,
        token: Option<String>,
    ) -> Result<(), CuaError> {
        let sbx = self.cua.sandboxes();
        match sbx.get(name.to_string()).await {
            Ok(i) if i.location == "direct" => {
                sbx.delete(name.to_string()).await?;
            }
            Ok(_) => {
                return Err(CuaError::InvalidArgument(format!(
                    "a non-direct sandbox named {name} already exists"
                )));
            }
            Err(CuaError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let saved = token.clone();
        sbx.create(SandboxCreateOptions {
            name: Some(name.to_string()),
            token,
            ..SandboxCreateOptions::new(format!("direct:{url}"), "")
        })
        .await?;
        crate::sandbox::save_env_token(name, saved.as_deref())
    }

    async fn ls(&self, provider: Option<String>, out: &mut dyn Write) -> Result<i32, CuaError> {
        let filter = match provider.as_deref().map(str::to_ascii_lowercase).as_deref() {
            None => None,
            Some("host") => {
                util::line(out, "  host  [local]");
                return Ok(0);
            }
            Some("cloud" | "cloudv2" | "fleet") => Some("cloud"),
            Some("local" | "lume" | "lumier" | "docker" | "winsandbox") => Some("local"),
            Some("direct" | "url") => Some("direct"),
            Some(o) => return fail(&format!("Unknown provider '{o}'")),
        };
        if filter.is_none() && consent_path().exists() {
            util::line(out, "  host  [local]");
        }
        // Every known target (all providers) unless one is asked for.
        let list = match filter {
            Some(p) => self.cua.sandboxes().list_known(Some(p)).await?,
            None => crate::sandbox::list_known(self.cua).await?,
        };
        if list.is_empty() && filter.is_some() {
            util::line(out, "No sandboxes found for that provider.");
        }
        for s in list.iter().filter(|s| s.name != "host") {
            util::line(
                out,
                format!(
                    "  {}  [{}]  {}",
                    s.name,
                    format!("{:?}", s.status).to_lowercase(),
                    // `cua do ls` keeps its provider words.
                    match s.location.as_str() {
                        "cloud" => "fleet",
                        other => other,
                    }
                ),
            );
        }
        Ok(0)
    }

    async fn computer(&self) -> Result<Computer, CuaError> {
        Ok(Computer::new(
            crate::sandbox::env_of(self.cua, &self.state.name).await?,
        ))
    }

    async fn zoom_ref(&self, c: &Computer) -> Option<cua_spacesd_client::pb::WindowRef> {
        let name = self.state.zoom_window.as_ref()?;
        if let Some(id) = &self.state.zoom_window_id
            && let Ok(w) = c.window(id).await
        {
            return w.r#ref;
        }
        c.windows(name).await.ok()?.into_iter().next()?.r#ref
    }

    /// Screenshot honoring the zoom; updates the coordinate mapping.
    async fn shot(&mut self, c: &Computer, update: bool) -> Result<computer::Shot, CuaError> {
        let w = self.zoom_ref(c).await;
        let s = match c.screenshot(w.clone(), MAX_LENGTH).await {
            Ok(s) => s,
            Err(_) if w.is_some() => c.screenshot(None, MAX_LENGTH).await?,
            Err(e) => return Err(e),
        };
        if update {
            self.state.zoom_scale = s.scale;
            self.state.zoom_origin = Some([s.origin.0, s.origin.1]);
            self.state.save()?;
        }
        Ok(s)
    }

    async fn focus_zoom(&self, c: &Computer) {
        if let Some(id) = &self.state.zoom_window_id {
            let _ = c.window_op(id, "activate").await;
        }
    }

    fn session(&mut self) -> Option<PathBuf> {
        if let Some(s) = &self.state.trajectory_session {
            let p = PathBuf::from(s);
            if p.is_dir() {
                return Some(p);
            }
        }
        let machine = if self.state.name.is_empty() {
            self.state
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".into())
        } else {
            self.state.name.clone()
        };
        let p = trajectory::new_session(&machine).ok()?;
        self.state.trajectory_session = Some(p.display().to_string());
        let _ = self.state.save();
        Some(p)
    }

    /// Records a turn (never fails the command).
    async fn record(
        &mut self,
        c: Option<&Computer>,
        kind: &str,
        params: serde_json::Value,
        png: Option<Vec<u8>>,
    ) {
        if !self.record {
            return;
        }
        let png = match (png, c) {
            (Some(p), _) => Some(p),
            (None, Some(c)) => self.shot(c, false).await.ok().map(|s| s.png),
            (None, None) => None,
        };
        if let Some(s) = self.session() {
            let _ = trajectory::record_turn(&s, kind, params, png.as_deref());
        }
    }

    async fn act(&mut self, action: DoAction, out: &mut dyn Write) -> Result<Option<String>, Exit> {
        let c = self.computer().await?;
        if action.needs_desktop() {
            // Input sent before the desktop session is up reports success
            // while reaching nothing: wait for it (one `Health` call when
            // it is already up).
            c.ensure_desktop(DESKTOP_WAIT).await?;
        }
        use serde_json::json;
        Ok(Some(match action {
            DoAction::Zoom { window_name } => {
                let found = c.windows(&window_name).await;
                let (id, title) = match found {
                    Ok(ws) if ws.is_empty() => {
                        return Err(Exit(
                            1,
                            format!("No windows found matching '{window_name}'"),
                        ));
                    }
                    Ok(ws) if ws.len() > 1 => {
                        let labels: Vec<String> = ws
                            .iter()
                            .map(|w| format!("{} ({})", computer::window_id(w), w.title))
                            .collect();
                        return Err(Exit(
                            1,
                            format!(
                                "Multiple windows matched '{window_name}' — be more specific. Found: {}",
                                labels.join(", ")
                            ),
                        ));
                    }
                    Ok(ws) => (Some(computer::window_id(&ws[0])), ws[0].title.clone()),
                    Err(_) => (None, window_name.clone()),
                };
                self.state.zoom_window = Some(window_name);
                self.state.zoom_window_id = id.clone();
                self.state.zoom_scale = 1.0;
                self.state.zoom_origin = None;
                self.state.save()?;
                let suffix = id.map(|i| format!(" ({i})")).unwrap_or_default();
                format!("Zoomed to '{title}'{suffix}")
            }
            DoAction::Screenshot { save } => {
                let s = self.shot(&c, true).await?;
                // Without --save: a bounded ~/.cua/screenshots (the newest
                // 20 are kept), not the temp directory.
                let path = match save {
                    Some(p) => p,
                    None => cua_disk::scratch::new_file(
                        &cua_disk::Layout::default(),
                        "cua_screenshot_",
                        &util::timestamp("%Y%m%d_%H%M%S"),
                        "png",
                    )
                    .map_err(internal)?,
                };
                std::fs::write(&path, &s.png).map_err(internal)?;
                self.record(None, "screenshot", json!({}), Some(s.png))
                    .await;
                format!("screenshot saved to {}", path.display())
            }
            DoAction::Snapshot {
                instructions,
                model,
            } => {
                let s = self.shot(&c, true).await?;
                let path = cua_disk::scratch::new_file(
                    &cua_disk::Layout::default(),
                    "cua_snapshot_",
                    &util::timestamp("%Y%m%d_%H%M%S"),
                    "png",
                )
                .map_err(internal)?;
                std::fs::write(&path, &s.png).map_err(internal)?;
                let text = snapshot_summary(&s.png, &instructions.join(" "), model)
                    .await
                    .map_err(|e| Exit(1, e.to_string()))?;
                util::line(out, format!("✅ snapshot — {}", path.display()));
                util::line(out, "");
                util::line(out, text);
                util::line(out, self.state.context());
                return Ok(None);
            }
            DoAction::Click { x, y, button } => {
                self.focus_zoom(&c).await;
                let (sx, sy) = self.state.to_screen(x, y);
                c.click(sx, sy, &button, 1).await?;
                self.record(
                    Some(&c),
                    "click",
                    json!({"x": x, "y": y, "button": button}),
                    None,
                )
                .await;
                format!("clicked ({x}, {y}) [{button}]")
            }
            DoAction::Dclick { x, y } => {
                self.focus_zoom(&c).await;
                let (sx, sy) = self.state.to_screen(x, y);
                c.click(sx, sy, "left", 2).await?;
                self.record(Some(&c), "double_click", json!({"x": x, "y": y}), None)
                    .await;
                format!("double-clicked ({x}, {y})")
            }
            DoAction::Move { x, y } => {
                self.focus_zoom(&c).await;
                let (sx, sy) = self.state.to_screen(x, y);
                c.move_to(sx, sy).await?;
                self.record(None, "move", json!({"x": x, "y": y}), None)
                    .await;
                format!("cursor moved to ({x}, {y})")
            }
            DoAction::Type { text } => {
                self.focus_zoom(&c).await;
                c.type_text(&text).await?;
                self.record(Some(&c), "type", json!({"text": text}), None)
                    .await;
                let preview: String = text.chars().take(40).collect();
                let more = if text.chars().count() > 40 { "…" } else { "" };
                format!("typed: {:?}", format!("{preview}{more}"))
            }
            DoAction::Key { key } => {
                self.focus_zoom(&c).await;
                c.press(&key).await?;
                self.record(Some(&c), "keypress", json!({"keys": [key]}), None)
                    .await;
                format!("pressed key: {key}")
            }
            DoAction::Hotkey { keys } => {
                self.focus_zoom(&c).await;
                let keys = split_keys(&keys);
                c.hotkey(&keys).await?;
                self.record(Some(&c), "hotkey", json!({"keys": keys}), None)
                    .await;
                format!("hotkey: {}", keys.join("+"))
            }
            DoAction::Scroll { direction, amount } => {
                self.focus_zoom(&c).await;
                c.scroll(&direction, amount, None).await?;
                self.record(
                    Some(&c),
                    "scroll",
                    json!({"scroll_direction": direction, "scroll_amount": amount}),
                    None,
                )
                .await;
                format!("scrolled {direction} {amount}x")
            }
            DoAction::Drag { x1, y1, x2, y2 } => {
                self.focus_zoom(&c).await;
                let from = self.state.to_screen(x1, y1);
                let to = self.state.to_screen(x2, y2);
                c.drag(from, to).await?;
                self.record(
                    Some(&c),
                    "drag",
                    json!({"start_x": x1, "start_y": y1, "end_x": x2, "end_y": y2}),
                    None,
                )
                .await;
                format!("dragged ({x1},{y1}) → ({x2},{y2})")
            }
            DoAction::Shell {
                cols,
                rows,
                command,
            } => {
                let line = command.join(" ");
                if line.trim().is_empty() {
                    if !util::interactive() {
                        return Err(Exit(1, "No command provided".into()));
                    }
                    let env = crate::sandbox::env_of(self.cua, &self.state.name).await?;
                    let code = crate::shell::interactive(&env, None, cols, rows).await?;
                    return Err(Exit(code, String::new()));
                }
                let o = c.shell(&line, Some(120_000)).await?;
                let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let code = o.exit.code.unwrap_or(if o.exit.success { 0 } else { 1 });
                if code != 0 {
                    let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                    let detail = if stderr.is_empty() { stdout } else { stderr };
                    return Err(Exit(1, format!("exit {code}: {detail}")));
                }
                self.record(None, "shell", json!({"command": line}), None)
                    .await;
                if stdout.is_empty() {
                    "done".into()
                } else {
                    stdout
                }
            }
            DoAction::Open { path } => {
                c.open(&path).await?;
                self.record(None, "open", json!({"path": path}), None).await;
                format!("opened: {path}")
            }
            DoAction::Launch { app, args } => {
                let (pid, window) = c.launch_and_focus(&app, args, LAUNCH_FOCUS_WAIT).await?;
                self.record(None, "launch", json!({"app": app}), None).await;
                match window {
                    Some(w) => format!(
                        "launched {app} (pid {pid}); focused window {} {:?}",
                        computer::window_id(&w),
                        w.title
                    ),
                    None => format!(
                        "launched {app} (pid {pid}); no new window within {}s",
                        LAUNCH_FOCUS_WAIT.as_secs()
                    ),
                }
            }
            DoAction::Cursor => {
                let (x, y) = c.cursor().await?;
                format!("cursor at ({x}, {y})")
            }
            DoAction::Clipboard(ClipboardAction::Get) => {
                let t = c.clipboard().await?.unwrap_or_default();
                util::line(out, t);
                return Ok(None);
            }
            DoAction::Clipboard(ClipboardAction::Set { text }) => {
                c.set_clipboard(&text).await?;
                "clipboard set".into()
            }
            DoAction::Window(w) => return self.window(&c, w, out).await,
            DoAction::A11y(a) => {
                let v = match a {
                    A11yAction::Tree { window, depth } => {
                        let t = c.a11y_tree(window.as_deref(), depth).await?;
                        json!({
                            "snapshot_id": t.snapshot_id,
                            "truncated": t.truncated,
                            "nodes": t.nodes.iter().map(computer::node_json).collect::<Vec<_>>(),
                        })
                    }
                    A11yAction::Find { name, role, window } => {
                        let f = c.a11y_find(window.as_deref(), &name, &role).await?;
                        json!({
                            "snapshot_id": f.snapshot_id,
                            "nodes": f.nodes.iter().map(computer::node_json).collect::<Vec<_>>(),
                        })
                    }
                    A11yAction::Act {
                        element,
                        action,
                        value,
                        snapshot,
                    } => {
                        c.a11y_act(&snapshot, &element, &action, &value).await?;
                        return Ok(Some(format!("{action} {element}")));
                    }
                };
                util::json_line(out, &v);
                return Ok(None);
            }
            DoAction::Switch { .. } | DoAction::Status | DoAction::Ls { .. } | DoAction::Unzoom => {
                unreachable!("handled by run")
            }
        }))
    }

    async fn window(
        &mut self,
        c: &Computer,
        w: WindowAction,
        out: &mut dyn Write,
    ) -> Result<Option<String>, Exit> {
        Ok(Some(match w {
            WindowAction::Ls { app } => {
                for w in c.windows(&app).await? {
                    util::line(out, computer::window_line(&w));
                }
                return Ok(None);
            }
            WindowAction::Unfocus => {
                c.press("escape").await?;
                "unfocused window".into()
            }
            WindowAction::Focus { window_id } => {
                c.window_op(&window_id, "activate").await?;
                format!("focused window {window_id}")
            }
            WindowAction::Minimize { window_id } => {
                c.window_op(&window_id, "minimize").await?;
                format!("minimized window {window_id}")
            }
            WindowAction::Maximize { window_id } => {
                c.window_op(&window_id, "maximize").await?;
                format!("maximized window {window_id}")
            }
            WindowAction::Restore { window_id } => {
                c.window_op(&window_id, "restore").await?;
                format!("restored window {window_id}")
            }
            WindowAction::Close { window_id } => {
                c.window_op(&window_id, "close").await?;
                format!("closed window {window_id}")
            }
            WindowAction::Resize {
                window_id,
                width,
                height,
            } => {
                c.set_bounds(&window_id, None, Some((width as f64, height as f64)))
                    .await?;
                format!("resized window {window_id} to {width}x{height}")
            }
            WindowAction::Move { window_id, x, y } => {
                c.set_bounds(&window_id, Some((x as f64, y as f64)), None)
                    .await?;
                format!("moved window {window_id} to ({x},{y})")
            }
            WindowAction::Info { window_id } => {
                util::json_line(out, &computer::window_json(&c.window(&window_id).await?));
                return Ok(None);
            }
        }))
    }
}

fn direct_name(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("direct");
    let s: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("direct-{}", s.trim_matches('-'))
}

/// Summarizes a screenshot with the Anthropic Messages API.
async fn snapshot_summary(
    png: &[u8],
    extra: &str,
    model: Option<String>,
) -> Result<String, CuaError> {
    use base64::Engine;
    let key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| CuaError::ProviderNotConfigured("ANTHROPIC_API_KEY not set".into()))?;
    let base = std::env::var("ANTHROPIC_BASE_URL")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "https://api.anthropic.com".into());
    let model = model
        .or_else(|| std::env::var("CUA_SNAPSHOT_MODEL").ok())
        .unwrap_or_else(|| "claude-haiku-4-5".into());
    let mut prompt = String::from(
        "You are analyzing a screenshot for an AI agent.\n\
         1. Write a 1-2 sentence summary of what is currently on screen.\n\
         2. List every interactive element visible (buttons, links, inputs, menus, checkboxes, \
         dropdowns, etc.) with its center coordinates in image pixels (origin = top-left). Be precise.\n\n\
         Respond in this exact JSON format:\n\
         {\"summary\": \"...\", \"elements\": [{\"name\": \"...\", \"type\": \"...\", \"x\": N, \"y\": N}, ...]}\n",
    );
    if !extra.is_empty() {
        prompt.push_str(&format!("\nAdditional instructions: {extra}"));
    }
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 4096,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                    "data": base64::engine::general_purpose::STANDARD.encode(png)}},
                {"type": "text", "text": prompt},
            ],
        }],
    });
    let r = util::http()
        .post(format!("{}/v1/messages", base.trim_end_matches('/')))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(util::http_err)?;
    let status = r.status().as_u16();
    let v: serde_json::Value = r.json().await.map_err(util::http_err)?;
    if status != 200 {
        return Err(CuaError::Http(format!(
            "AI analysis failed (HTTP {status}): {}",
            v["error"]["message"].as_str().unwrap_or_default()
        )));
    }
    if v["stop_reason"] == "refusal" {
        return Err(CuaError::Http("AI analysis was declined".into()));
    }
    let raw = v["content"]
        .as_array()
        .and_then(|c| c.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let json_part = raw
        .find('{')
        .and_then(|a| raw.rfind('}').map(|b| &raw[a..=b]))
        .unwrap_or(&raw);
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_part) else {
        return Ok(raw);
    };
    let mut s = parsed["summary"].as_str().unwrap_or_default().to_string();
    if let Some(els) = parsed["elements"].as_array().filter(|e| !e.is_empty()) {
        s.push_str("\n\nInteractive elements:");
        for e in els {
            s.push_str(&format!(
                "\n  • {} [{}]  ({}, {})",
                e["name"].as_str().unwrap_or("?"),
                e["type"].as_str().unwrap_or("?"),
                e["x"],
                e["y"]
            ));
        }
    }
    Ok(s)
}
