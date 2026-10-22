//! What `cua` writes besides its results: log verbosity, colour, the
//! terminal rules for live lines, and how an error is shown.
//!
//! The rules (the output style guide in the CLI reference):
//!
//! - stdout carries results and data; stderr carries progress, logs,
//!   warnings, errors and notices;
//! - `-q` shows errors only, the default shows warnings, `-v` adds what Cua
//!   chose and did, `-vv` is debug, `-vvv` is trace including dependencies;
//!   `--log-level` and `CUA_LOG_LEVEL` say the same in words, and `CUA_LOG`
//!   stays the `tracing` filter for experts;
//! - colour and in-place lines (a step that rewrites itself) only on a
//!   terminal, and never with `NO_COLOR`, `TERM=dumb` or `CI`;
//! - an error is `error: <one sentence>` and `fix: <one sentence>`; the
//!   layers it passed through appear with `-v`;
//! - log lines and in-place lines never tear each other: a log line clears
//!   the live line, prints, and the live line is drawn again.

use cua_sdk::{CuaError, ErrorReport};
use std::io::{IsTerminal, Write};
use std::sync::Mutex;

/// How loud `cua` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// `-q`: errors only.
    Quiet,
    /// The default: errors and warnings.
    Normal,
    /// `-v`: what Cua chose and did.
    Info,
    /// `-vv`
    Debug,
    /// `-vvv`: everything, dependencies included.
    Trace,
}

/// `--log-level` / `CUA_LOG_LEVEL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum LogLevel {
    /// Errors only (`-q`).
    Error,
    /// Errors and warnings (the default).
    Warn,
    /// What Cua chose and did (`-v`).
    Info,
    /// Debug detail (`-vv`).
    Debug,
    /// Everything, dependencies included (`-vvv`).
    Trace,
}

/// `--color`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    /// Colour on a terminal, unless `NO_COLOR`, `TERM=dumb` or `CI` is set.
    #[default]
    Auto,
    /// Always colour.
    Always,
    /// Never colour.
    Never,
}

/// The verbosity flags as parsed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flags {
    /// `-q`
    pub quiet: bool,
    /// Number of `-v`.
    pub verbose: u8,
    /// `--log-level` (or `CUA_LOG_LEVEL`).
    pub log_level: Option<LogLevel>,
}

impl Flags {
    /// `-q` beats `-v`, which beats `--log-level` / `CUA_LOG_LEVEL`.
    pub fn level(&self) -> Level {
        if self.quiet {
            return Level::Quiet;
        }
        match self.verbose {
            0 => {}
            1 => return Level::Info,
            2 => return Level::Debug,
            _ => return Level::Trace,
        }
        match self.log_level {
            Some(LogLevel::Error) => Level::Quiet,
            Some(LogLevel::Warn) | None => Level::Normal,
            Some(LogLevel::Info) => Level::Info,
            Some(LogLevel::Debug) => Level::Debug,
            Some(LogLevel::Trace) => Level::Trace,
        }
    }
}

/// The Cua crates whose log targets `-v` and `-vv` turn up (a `tracing`
/// filter names a target by its module path, so there is no wildcard).
/// A test keeps the list in step with `libs/cua/crates`.
pub(crate) const CUA_TARGETS: &[&str] = &[
    "cua_agent_setup",
    "cua_agents",
    "cua_auth",
    "cua_byoc",
    "cua_cli",
    "cua_contrib",
    "cua_daemon",
    "cua_disk",
    "cua_home",
    "cua_host",
    "cua_hotspot",
    "cua_image",
    "cua_keyvault",
    "cua_machine_seal",
    "cua_mesh",
    "cua_sandbox_core",
    "cua_sdk",
    "cua_spaces",
    "cua_spacesd_client",
    "cua_telemetry",
    "cua_teleport",
    "cua_volume",
    "cua_vmm",
];

/// Crates under `libs/cua/crates` that never log through `tracing` to the
/// CLI's stderr (libraries of types, bindings, tools).
#[cfg(test)]
pub(crate) const NOT_LOGGING: &[&str] = &[
    "cua_bindgen",
    "cua_chromium_storage",
    "cua_cli",
    "cua_create_failure",
    "cua_icon_cache",
    "cua_logging",
    "cua_media_client",
    "cua_media_codec",
    "cua_media_protocol",
    "cua_media_transport",
    "cua_mock_llm",
    "cua_proto",
    "cua_spaces_app_core",
    "cua_spaces_cli",
    "cua_spaces_contract",
    "cua_spaces_ext",
    "cua_spaces_ffi",
    "cua_spaces_transcript",
    "cua_spacesd_html5",
    "cua_teleport_bundle",
    "cua_viewer",
];

/// The `tracing` filter for a level. `cua_log` is `CUA_LOG` (the expert
/// override, used as given when set); `daemon_log` is the background daemon,
/// which logs `info` by default.
///
/// The WireGuard library's own timer messages (`boringtun`) stay off unless
/// the spec names them: they log a warning every few seconds while a mesh
/// peer that went away is retried, which says nothing a user can act on.
/// `CUA_LOG=warn,boringtun=debug` brings them back.
pub fn log_filter_spec(cua_log: Option<&str>, level: Level, daemon_log: bool) -> String {
    let own = |l: &str| {
        let mut spec = String::from("warn");
        for t in CUA_TARGETS {
            spec.push_str(&format!(",{t}={l}"));
        }
        spec
    };
    let base = match cua_log.map(str::trim).filter(|s| !s.is_empty()) {
        Some(spec) => spec.to_string(),
        None => match level {
            Level::Quiet => "error".to_string(),
            Level::Normal if daemon_log => "info".to_string(),
            Level::Normal => "warn".to_string(),
            Level::Info => own("info"),
            Level::Debug => own("debug"),
            Level::Trace => "trace".to_string(),
        },
    };
    if base.contains("boringtun") {
        base
    } else {
        format!("{base},boringtun=off")
    }
}

/// The parts of the environment that decide how to draw, read once so tests
/// can state them.
#[derive(Clone, Copy, Debug, Default)]
pub struct TermEnv {
    /// `NO_COLOR` is set and not empty (https://no-color.org).
    pub no_color: bool,
    /// `TERM=dumb`.
    pub dumb: bool,
    /// `CI` is set (and not `false` or `0`).
    pub ci: bool,
}

impl TermEnv {
    /// The current process environment.
    pub fn current() -> Self {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// From a variable lookup.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            no_color: get("NO_COLOR").is_some_and(|v| !v.is_empty()),
            dumb: get("TERM").is_some_and(|v| v == "dumb"),
            ci: get("CI").is_some_and(|v| {
                let v = v.trim().to_ascii_lowercase();
                !(v.is_empty() || v == "0" || v == "false")
            }),
        }
    }

    /// Whether to colour a stream: `--color`, else a terminal that is not
    /// dumb, and neither `NO_COLOR` nor `CI`.
    pub fn use_color(&self, choice: ColorChoice, is_tty: bool) -> bool {
        match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => is_tty && !self.no_color && !self.dumb && !self.ci,
        }
    }

    /// Whether stderr may rewrite a line in place (steps, spinners): a
    /// terminal that is not dumb, with neither `NO_COLOR` nor `CI`. Anything
    /// else gets each line once, plain.
    pub fn live_lines(&self, is_tty: bool) -> bool {
        is_tty && !self.no_color && !self.dumb && !self.ci
    }
}

/// What this process decided once at start-up.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// How loud.
    pub level: Level,
    /// Colour on stderr.
    pub color: bool,
    /// In-place lines on stderr.
    pub live: bool,
}

static SETTINGS: std::sync::OnceLock<Settings> = std::sync::OnceLock::new();

/// Records the settings for this process (first call wins).
pub fn init(flags: Flags, choice: ColorChoice) -> Settings {
    let env = TermEnv::current();
    let tty = std::io::stderr().is_terminal();
    let s = Settings {
        level: flags.level(),
        color: env.use_color(choice, tty),
        live: env.live_lines(tty),
    };
    *SETTINGS.get_or_init(|| s)
}

/// The process settings (defaults before [`init`], for library callers and
/// tests).
pub fn settings() -> Settings {
    SETTINGS.get().copied().unwrap_or(Settings {
        level: Level::Normal,
        color: false,
        live: false,
    })
}

/// Whether a command shows its steps: on a terminal (stderr), not with
/// `--json` and not with `-q`.
pub fn show_steps(json: bool) -> bool {
    !json && settings().level != Level::Quiet && std::io::stderr().is_terminal()
}

// ---------------------------------------------------------------- errors

/// Exit code for a report: the refined causes keep the family of the
/// variant they came from.
pub fn exit_code_of(e: &CuaError, report: &ErrorReport) -> i32 {
    match report.code.as_str() {
        "image_not_found" => 3,
        "network" => 5,
        "registry_auth" | "cloud_credentials" => 6,
        _ => crate::exit_code(e),
    }
}

/// `error:` and `fix:` lines (with `detail:` at `-v` and above), coloured
/// when `color`.
pub fn render_error(r: &ErrorReport, level: Level, color: bool) -> String {
    let label = |name: &str, code: &str| {
        if color {
            format!("\x1b[{code}m{name}:\x1b[0m")
        } else {
            format!("{name}:")
        }
    };
    let mut out = format!("{} {}\n", label("error", "1;31"), r.message);
    if let Some(h) = &r.hint {
        out.push_str(&format!("{} {h}\n", label("fix", "1;36")));
    }
    if level >= Level::Info
        && let Some(d) = &r.detail
    {
        out.push_str(&format!("{} {d}\n", label("detail", "2")));
    }
    out
}

/// The `--json` error object, one line:
/// `{"error":{"code":..,"message":..,"hint":..}}` (plus `detail` at `-v`).
pub fn json_error(r: &ErrorReport, level: Level) -> serde_json::Value {
    let mut e = serde_json::json!({
        "code": r.code,
        "message": r.message,
        "hint": r.hint,
    });
    if level >= Level::Info
        && let Some(d) = &r.detail
    {
        e["detail"] = serde_json::Value::String(d.clone());
    }
    serde_json::json!({ "error": e })
}

/// Shows `e` the way the process was asked to: JSON on stdout, else
/// `error:` / `fix:` on stderr. Returns the exit code.
pub fn show_error(e: &CuaError, json: bool, out: &mut dyn Write) -> i32 {
    let s = settings();
    let report = e.report();
    let code = exit_code_of(e, &report);
    if json {
        let _ = writeln!(out, "{}", json_error(&report, s.level));
        let _ = out.flush();
    } else {
        let text = render_error(&report, s.level, s.color);
        let _ = out.flush();
        write_stderr(text.as_bytes());
    }
    code
}

// -------------------------------------------------- live lines and logs

/// The text of the line on stderr that is being rewritten in place, if one
/// is: a log line has to clear it first and draw it again after itself.
static LIVE: Mutex<Option<String>> = Mutex::new(None);

/// Marks `text` as the live line now on screen (`None`: no live line).
pub fn set_live(text: Option<String>) {
    *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = text;
}

/// Writes to stderr without tearing the live line: clear it, write, draw it
/// again.
pub fn write_stderr(bytes: &[u8]) {
    let live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut err = std::io::stderr().lock();
    match live.as_deref() {
        Some(line) if settings().live => {
            let _ = err.write_all(b"\r\x1b[2K");
            let _ = err.write_all(bytes);
            if !bytes.ends_with(b"\n") {
                let _ = err.write_all(b"\n");
            }
            let _ = err.write_all(line.as_bytes());
        }
        _ => {
            let _ = err.write_all(bytes);
        }
    }
    let _ = err.flush();
}

/// The `tracing` writer: events go through [`write_stderr`].
#[derive(Clone, Copy, Default)]
pub struct LogWriter;

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        write_stderr(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> LogWriter {
        LogWriter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> TermEnv {
        TermEnv::from_vars(|k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_string())
        })
    }

    #[test]
    fn colour_and_live_lines_need_a_plain_terminal() {
        let plain = env(&[]);
        assert!(plain.use_color(ColorChoice::Auto, true));
        assert!(plain.live_lines(true));
        // Not a terminal: neither.
        assert!(!plain.use_color(ColorChoice::Auto, false));
        assert!(!plain.live_lines(false));
        for vars in [
            &[("NO_COLOR", "1")][..],
            &[("TERM", "dumb")],
            &[("CI", "true")],
            &[("CI", "1")],
        ] {
            let e = env(vars);
            assert!(!e.use_color(ColorChoice::Auto, true), "{vars:?}");
            assert!(!e.live_lines(true), "{vars:?}");
        }
        // Empty NO_COLOR and CI=false do not count (no-color.org).
        assert!(env(&[("NO_COLOR", "")]).use_color(ColorChoice::Auto, true));
        assert!(env(&[("CI", "false")]).use_color(ColorChoice::Auto, true));
        // --color wins over the environment.
        let ci = env(&[("CI", "1"), ("NO_COLOR", "1")]);
        assert!(ci.use_color(ColorChoice::Always, false));
        assert!(!plain.use_color(ColorChoice::Never, true));
    }

    #[test]
    fn the_flags_pick_one_level() {
        let l = |quiet, verbose, log_level| {
            Flags {
                quiet,
                verbose,
                log_level,
            }
            .level()
        };
        assert_eq!(l(false, 0, None), Level::Normal);
        assert_eq!(l(true, 0, None), Level::Quiet);
        assert_eq!(l(false, 1, None), Level::Info);
        assert_eq!(l(false, 2, None), Level::Debug);
        assert_eq!(l(false, 3, None), Level::Trace);
        assert_eq!(l(false, 9, None), Level::Trace);
        assert_eq!(l(false, 0, Some(LogLevel::Error)), Level::Quiet);
        assert_eq!(l(false, 0, Some(LogLevel::Warn)), Level::Normal);
        assert_eq!(l(false, 0, Some(LogLevel::Debug)), Level::Debug);
        // -q beats -v beats --log-level.
        assert_eq!(l(true, 2, Some(LogLevel::Trace)), Level::Quiet);
        assert_eq!(l(false, 1, Some(LogLevel::Trace)), Level::Info);
    }

    #[test]
    fn the_filter_follows_the_level_and_cua_log_wins() {
        let spec = |cua_log, level, daemon| log_filter_spec(cua_log, level, daemon);
        assert_eq!(spec(None, Level::Normal, false), "warn,boringtun=off");
        assert_eq!(spec(None, Level::Normal, true), "info,boringtun=off");
        assert_eq!(spec(None, Level::Quiet, false), "error,boringtun=off");
        assert_eq!(spec(None, Level::Trace, false), "trace,boringtun=off");
        let info = spec(None, Level::Info, false);
        assert!(info.starts_with("warn,cua_agent_setup=info,"), "{info}");
        assert!(info.contains(",cua_vmm=info"), "{info}");
        assert!(spec(None, Level::Debug, false).contains(",cua_cli=debug"));
        // CUA_LOG is used as given, whatever the flags say.
        assert_eq!(
            spec(Some("cua_vmm=trace"), Level::Quiet, false),
            "cua_vmm=trace,boringtun=off"
        );
        assert_eq!(
            spec(Some("warn,boringtun=debug"), Level::Normal, false),
            "warn,boringtun=debug"
        );
        // An empty CUA_LOG is not set.
        assert_eq!(spec(Some("  "), Level::Normal, false), "warn,boringtun=off");
        // Every spec parses.
        for l in [
            Level::Quiet,
            Level::Normal,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ] {
            tracing_subscriber::EnvFilter::try_new(spec(None, l, false)).unwrap();
        }
    }

    /// Every crate under `libs/cua/crates` is either a log target `-v`
    /// turns up or listed as one that never logs to the CLI.
    #[test]
    fn the_target_list_covers_every_crate() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut missing = Vec::new();
        for entry in std::fs::read_dir(crates).unwrap().flatten() {
            if !entry.path().join("Cargo.toml").exists() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().replace('-', "_");
            if !CUA_TARGETS.contains(&name.as_str()) && !NOT_LOGGING.contains(&name.as_str()) {
                missing.push(name);
            }
        }
        assert!(
            missing.is_empty(),
            "add these crates to CUA_TARGETS (they log) or NOT_LOGGING in diag.rs: {missing:?}"
        );
    }

    fn report() -> ErrorReport {
        ErrorReport {
            code: "already_exists".into(),
            message: "a sandbox named 'x' already exists".into(),
            hint: Some("pick another name".into()),
            detail: Some("raw layers".into()),
        }
    }

    #[test]
    fn an_error_is_two_lines_and_the_layers_come_with_v() {
        let r = report();
        assert_eq!(
            render_error(&r, Level::Normal, false),
            "error: a sandbox named 'x' already exists\nfix: pick another name\n"
        );
        assert_eq!(
            render_error(&r, Level::Info, false),
            "error: a sandbox named 'x' already exists\nfix: pick another name\ndetail: raw layers\n"
        );
        let coloured = render_error(&r, Level::Normal, true);
        assert!(coloured.starts_with("\x1b[1;31merror:\x1b[0m "));
        let no_hint = ErrorReport { hint: None, ..r };
        assert_eq!(
            render_error(&no_hint, Level::Quiet, false),
            "error: a sandbox named 'x' already exists\n"
        );
    }

    #[test]
    fn a_json_error_has_code_message_and_hint() {
        let r = report();
        assert_eq!(
            json_error(&r, Level::Normal),
            serde_json::json!({"error": {
                "code": "already_exists",
                "message": "a sandbox named 'x' already exists",
                "hint": "pick another name",
            }})
        );
        assert_eq!(json_error(&r, Level::Info)["error"]["detail"], "raw layers");
        let no_hint = ErrorReport { hint: None, ..r };
        assert_eq!(
            json_error(&no_hint, Level::Normal)["error"]["hint"],
            serde_json::Value::Null
        );
    }
}
