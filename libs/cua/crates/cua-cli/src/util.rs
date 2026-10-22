//! Shared helpers: paths, output, prompts, browser, HTTP.

use cua_sdk::CuaError;
use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
    time::Duration,
};

/// The "Cua Cloud has ended" error every removed Cua Cloud command and option
/// fails with (exit code 4, like the SDK).
pub fn cloud_ended() -> CuaError {
    CuaError::Unsupported(cua_sandbox_core::CLOUD_ENDED.into())
}

/// The arguments of a removed Cua Cloud command, accepted and ignored so
/// that a script still calling it fails with the one ended message instead
/// of a parse error. Such commands are hidden: not in `--help` or the docs.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct EndedArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    _args: Vec<String>,
}

/// `~/.cua` (or `$CUA_HOME`).
pub fn cua_home() -> PathBuf {
    cua_daemon::cua_home()
}

/// An `Internal` error from anything displayable.
pub fn internal(e: impl std::fmt::Display) -> CuaError {
    CuaError::Internal(e.to_string())
}

/// An `Http` error from anything displayable.
pub fn http_err(e: impl std::fmt::Display) -> CuaError {
    CuaError::Http(e.to_string())
}

/// Writes a line, ignoring broken pipes.
pub fn line(out: &mut dyn Write, s: impl AsRef<str>) {
    let _ = writeln!(out, "{}", s.as_ref());
}

/// Pretty JSON line.
pub fn json_line(out: &mut dyn Write, v: &serde_json::Value) {
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_default()
    );
}

/// Whether an HTTP header carries a credential (`authorization`,
/// `x-cua-env-authorization`, cookies, API keys, tokens, secrets).
pub fn is_secret_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "cookie"
        || n == "set-cookie"
        || [
            "authorization",
            "token",
            "secret",
            "api-key",
            "apikey",
            "password",
        ]
        .iter()
        .any(|k| n.contains(k))
}

/// A credential for display: the auth scheme (`Bearer`, `Basic`) stays,
/// the value becomes `****`.
pub fn mask_secret(value: &str) -> String {
    match value.split_once(' ') {
        Some((scheme, rest))
            if !rest.trim().is_empty() && scheme.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            format!("{scheme} ****")
        }
        _ if value.is_empty() => String::new(),
        _ => "****".into(),
    }
}

/// `value`, masked when `name` is a credential header and `show` is off.
pub fn header_for_display(name: &str, value: &str, show: bool) -> String {
    if !show && is_secret_header(name) {
        mask_secret(value)
    } else {
        value.to_string()
    }
}

/// Left-aligned table with a header row. Columns are sized to content.
pub fn table(out: &mut dyn Write, headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(c.chars().count());
            }
        }
    }
    let fmt = |cells: Vec<String>| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i + 1 == cells.len() {
                    c.clone()
                } else {
                    format!("{c:<w$}", w = widths[i])
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    line(out, fmt(headers.iter().map(|h| h.to_string()).collect()));
    for r in rows {
        line(out, fmt(r.clone()));
    }
}

/// True when both stdin and stdout are terminals.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Asks a y/N question on stderr. Non-interactive stdin answers `default`.
pub fn confirm(prompt: &str, default: bool) -> bool {
    if !std::io::stdin().is_terminal() {
        return default;
    }
    eprint!("{prompt} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return false;
    }
    matches!(s.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Reads one line from stdin after a prompt on stderr.
pub fn prompt_line(prompt: &str) -> Option<String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Opens `url` in the default browser, only for an interactive terminal and
/// unless `CUA_NO_BROWSER` is set. Returns whether a browser was launched.
pub fn open_browser(url: &str) -> bool {
    if std::env::var_os("CUA_NO_BROWSER").is_some() || !interactive() {
        return false;
    }
    launch_url(url)
}

/// Hands `url` to the OS's default handler.
///
/// Windows uses `ShellExecuteW`, never `cmd /C start`: cmd.exe reads an
/// unquoted `&` as a command separator, so a sign-in URL arrived as
/// `...?response_type=code` with every other query parameter lost (CUA-1701).
#[cfg(windows)]
fn launch_url(url: &str) -> bool {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, target) = (wide("open"), wide(url));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    r as isize > 32
}

#[cfg(not(windows))]
fn launch_url(url: &str) -> bool {
    let (program, args) = launch_command(url);
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// The command that opens `url` on macOS and Linux. The URL is one argv
/// entry, passed through exactly; no shell sees it.
#[cfg(not(windows))]
fn launch_command(url: &str) -> (&'static str, Vec<String>) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    (program, vec![url.to_string()])
}

/// A reqwest client with sane timeouts.
pub fn http() -> reqwest::Client {
    cua_spacesd_client::transport::ensure_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .user_agent(format!("cua-cli/{}", cua_sdk::VERSION))
        .build()
        .expect("http client")
}

/// Current local time as `YYYYMMDD_HHMMSS`-style strings.
pub fn timestamp(fmt: &str) -> String {
    chrono::Local::now().format(fmt).to_string()
}

/// Creates the parent directory of `p`.
pub fn ensure_parent(p: &std::path::Path) -> Result<(), CuaError> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(internal)?;
    }
    Ok(())
}

/// Writes a file readable only by the owner (0600 on Unix).
pub fn write_private(p: &std::path::Path, data: &[u8]) -> Result<(), CuaError> {
    // A test must never write tokens or credentials into the real ~/.cua.
    cua_home::guard_write(p).map_err(internal)?;
    ensure_parent(p)?;
    let tmp = p.with_extension("tmp");
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(&tmp).map_err(internal)?;
        f.write_all(data).map_err(internal)?;
    }
    std::fs::rename(&tmp, p).map_err(internal)
}

pub use cua_sandbox_core::progress::CreateWords;

/// The step now running.
struct Running {
    phase: cua_sandbox_core::progress::Phase,
    text: String,
    since: std::time::Instant,
}

/// A create's steps on stderr.
///
/// On a terminal that may rewrite a line (not `NO_COLOR`, `TERM=dumb` or
/// `CI`), the running step shows a spinner and the time it has taken
/// (`⠋ Waiting for its services (12 s)`), and becomes `✓ Started (1.2 s)`
/// when the next one begins. Anywhere else each step is one plain line,
/// printed once, with no cursor codes. Never used for `--json` or `-q`.
pub struct StepLines {
    words: CreateWords,
    running: std::sync::Mutex<Option<Running>>,
    stopped: std::sync::atomic::AtomicBool,
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// "1.2 s", "12 s", "2 min 5 s".
pub fn elapsed_text(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.1} s")
    } else if secs < 60.0 {
        format!("{} s", secs.round() as u64)
    } else {
        let s = secs.round() as u64;
        format!("{} min {} s", s / 60, s % 60)
    }
}

impl StepLines {
    /// Lines for a create of this kind. On a live terminal a thread
    /// redraws the spinner until [`StepLines::finish`].
    pub fn new(words: CreateWords) -> std::sync::Arc<Self> {
        let lines = std::sync::Arc::new(Self::bare(words));
        if crate::diag::settings().live {
            let weak = std::sync::Arc::downgrade(&lines);
            std::thread::spawn(move || {
                while let Some(lines) = weak.upgrade() {
                    if lines.stopped.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    if let Some(out) = lines.tick(std::time::Instant::now()) {
                        lines.write(&out);
                    }
                    drop(lines);
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
        }
        lines
    }

    /// Without the redraw thread (tests drive the clock themselves).
    pub fn bare(words: CreateWords) -> Self {
        Self {
            words,
            running: Default::default(),
            stopped: Default::default(),
        }
    }

    fn frame(r: &Running, now: std::time::Instant) -> String {
        let since = now.saturating_duration_since(r.since);
        let spin = SPINNER[(since.as_millis() / 100) as usize % SPINNER.len()];
        format!("{spin} {} ({})", r.text, elapsed_text(since))
    }

    fn done(r: &Running, now: std::time::Instant, ok: bool, color: bool) -> String {
        let mark = match (ok, color) {
            (true, true) => "\x1b[32m✓\x1b[0m",
            (true, false) => "✓",
            (false, true) => "\x1b[31m✗\x1b[0m",
            (false, false) => "✗",
        };
        format!(
            "{mark} {} ({})",
            r.text,
            elapsed_text(now.saturating_duration_since(r.since))
        )
    }

    /// What to write for `p`: `None` when nothing changes. With `live` the
    /// running step is a spinner line and a finished one a `✓` line; without
    /// it each step is one plain line, written when it begins.
    pub fn render(
        &self,
        p: &cua_sandbox_core::progress::Progress,
        live: bool,
        now: std::time::Instant,
    ) -> Option<String> {
        let text = self.words.line(p)?;
        let color = crate::diag::settings().color;
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        match running.as_mut() {
            Some(r) if r.text == text => None,
            // The same step with new numbers.
            Some(r) if r.phase == p.phase => {
                r.text = text;
                live.then(|| format!("\r\x1b[2K{}", Self::frame(r, now)))
            }
            // A new step: the last one is done.
            Some(r) => {
                let done = Self::done(r, now, true, color);
                *running = Some(Running {
                    phase: p.phase,
                    text: text.clone(),
                    since: now,
                });
                let r = running.as_ref().expect("just set");
                Some(if live {
                    format!("\r\x1b[2K{done}\n{}", Self::frame(r, now))
                } else {
                    format!("{text}\n")
                })
            }
            None => {
                *running = Some(Running {
                    phase: p.phase,
                    text: text.clone(),
                    since: now,
                });
                let r = running.as_ref().expect("just set");
                Some(if live {
                    Self::frame(r, now)
                } else {
                    format!("{text}\n")
                })
            }
        }
    }

    /// The running step redrawn at `now` (the spinner turns, the time
    /// grows).
    pub fn tick(&self, now: std::time::Instant) -> Option<String> {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        running
            .as_ref()
            .map(|r| format!("\r\x1b[2K{}", Self::frame(r, now)))
    }

    fn write(&self, out: &str) {
        let mut err = std::io::stderr().lock();
        let _ = write!(err, "{out}");
        let _ = err.flush();
        let line = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|r| Self::frame(r, std::time::Instant::now()));
        crate::diag::set_live(line);
    }

    /// Writes `p`'s line to stderr, and tells the log writer which line is
    /// live so a log line does not tear it.
    pub fn show(&self, p: &cua_sandbox_core::progress::Progress) {
        let live = crate::diag::settings().live;
        if let Some(out) = self.render(p, live, std::time::Instant::now()) {
            self.write(&out);
        }
    }

    /// What ends the output: the last step as done (`ok`) or failed.
    pub fn finish_text(&self, ok: bool, live: bool, now: std::time::Instant) -> Option<String> {
        let color = crate::diag::settings().color;
        let r = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        live.then(|| format!("\r\x1b[2K{}\n", Self::done(&r, now, ok, color)))
    }

    /// Ends the output: every step stays as a `✓` (or `✗`) line.
    pub fn finish(&self, ok: bool) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let out = self.finish_text(ok, crate::diag::settings().live, std::time::Instant::now());
        crate::diag::set_live(None);
        if let Some(out) = out {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "{out}");
            let _ = err.flush();
        }
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;

    #[test]
    fn credential_headers_are_masked_unless_shown() {
        for h in [
            "authorization",
            "Authorization",
            "x-cua-env-authorization",
            "proxy-authorization",
            "x-api-key",
            "x-auth-token",
            "cookie",
            "x-client-secret",
        ] {
            assert!(is_secret_header(h), "{h}");
        }
        for h in ["x-request-id", "accept", "content-type", "mcp-session-id"] {
            assert!(!is_secret_header(h), "{h}");
        }
        assert_eq!(
            header_for_display("authorization", "Bearer abc", false),
            "Bearer ****"
        );
        assert_eq!(
            header_for_display("authorization", "Bearer abc", true),
            "Bearer abc"
        );
        assert_eq!(header_for_display("x-api-key", "sk-123", false), "****");
        assert_eq!(header_for_display("x-api-key", "", false), "");
        assert_eq!(header_for_display("x-request-id", "box", false), "box");
        // A scheme-less value with spaces is masked whole.
        assert_eq!(mask_secret("a/b c"), "****");
    }
}

/// Test helper: a private `CUA_HOME` and no `CUA_DEFAULT_*` in the
/// environment for one test; tests that touch the environment take turns.
#[cfg(test)]
pub mod test_env {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const VARS: [&str; 4] = [
        "CUA_HOME",
        "CUA_DEFAULT_ON",
        "CUA_DEFAULT_KIND",
        "CUA_DEFAULT_RUNTIME",
    ];

    /// Restores the environment on drop.
    pub struct Guard {
        _lock: std::sync::MutexGuard<'static, ()>,
        /// The private home.
        _home: tempfile::TempDir,
        saved: Vec<(&'static str, Option<String>)>,
    }

    /// Isolates the environment until the guard drops.
    pub fn isolated() -> Guard {
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let saved = VARS.iter().map(|v| (*v, std::env::var(v).ok())).collect();
        // SAFETY: tests that change the environment hold LOCK.
        unsafe {
            for v in VARS {
                std::env::remove_var(v);
            }
            std::env::set_var("CUA_HOME", home.path());
        }
        Guard {
            _lock: lock,
            _home: home,
            saved,
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: see `isolated`.
            unsafe {
                for (k, v) in &self.saved {
                    match v {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }
}

/// After a command caught Ctrl-C itself (to cancel a create), the rest of
/// it quits on Ctrl-C again, as a command does by default.
pub fn exit_on_ctrl_c() {
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            std::process::exit(130);
        }
    });
}

#[cfg(test)]
mod create_step_tests {
    #[test]
    fn create_steps_read_in_words_with_a_spinner_and_a_time_on_a_terminal() {
        use cua_sandbox_core::progress::{Phase, Progress, Transfer};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mac = super::CreateWords::for_image("macos", "", true);
        let lines = super::StepLines::bare(mac);
        let gb = 1u64 << 30;
        let pull = |done: u64| {
            Progress::phase(Phase::Pulling).bytes(Transfer {
                done,
                total: 30 * gb,
                per_second: Some((85u64 << 20) as f64),
            })
        };
        let r = |p: &Progress, ms| lines.render(p, true, at(ms));
        assert_eq!(
            r(&Progress::phase(Phase::Preparing), 0).as_deref(),
            Some("⠋ Preparing (0.0 s)")
        );
        assert_eq!(
            r(&pull(gb * 124 / 10), 1500).as_deref(),
            Some(
                "\r\x1b[2K✓ Preparing (1.5 s)\n⠋ Downloading macOS (first time only) 12.4 of 30.0 GB \u{b7} 85 MB/s \u{b7} about 4 min (0.0 s)"
            )
        );
        // The same numbers draw nothing; new numbers rewrite the line.
        assert_eq!(r(&pull(gb * 124 / 10), 1600), None);
        assert_eq!(
            r(&pull(30 * gb), 31_000).as_deref(),
            Some(
                "\r\x1b[2K⠴ Downloading macOS (first time only) 30.0 of 30.0 GB \u{b7} 85 MB/s \u{b7} less than a minute (30 s)"
            )
        );
        assert_eq!(
            r(&Progress::phase(Phase::Booting), 95_000).as_deref(),
            Some(
                "\r\x1b[2K✓ Downloading macOS (first time only) 30.0 of 30.0 GB \u{b7} 85 MB/s \u{b7} less than a minute (1 min 34 s)\n⠋ Booting macOS (0.0 s)"
            )
        );
        assert_eq!(r(&Progress::phase(Phase::Ready), 96_000), None);
        // The spinner turns and the time grows between events.
        assert_eq!(
            lines.tick(at(95_400)).as_deref(),
            Some("\r\x1b[2K⠼ Booting macOS (0.4 s)")
        );
        // The end: done, or failed.
        assert_eq!(
            lines.finish_text(true, true, at(97_000)).as_deref(),
            Some("\r\x1b[2K✓ Booting macOS (2.0 s)\n")
        );
        assert_eq!(lines.tick(at(98_000)), None);
        let failed = super::StepLines::bare(super::CreateWords::default());
        failed.render(&Progress::phase(Phase::Creating), true, at(0));
        assert_eq!(
            failed.finish_text(false, true, at(500)).as_deref(),
            Some("\r\x1b[2K✗ Creating (0.5 s)\n")
        );
    }

    /// `NO_COLOR`, `TERM=dumb`, `CI` and pipes: one plain line per step, no
    /// cursor codes, no spinner, no times.
    #[test]
    fn plain_steps_are_one_line_each_without_escape_codes() {
        use cua_sandbox_core::progress::{Phase, Progress, Transfer};
        let now = std::time::Instant::now();
        let c = super::CreateWords::for_image("ghcr.io/trycua/linux:24.04", "container", false);
        let lines = super::StepLines::bare(c);
        let pull = |done: u64| {
            Progress::phase(Phase::Pulling).bytes(Transfer {
                done,
                total: 1 << 30,
                per_second: Some(1e8),
            })
        };
        let out: Vec<Option<String>> = [
            Progress::phase(Phase::Preparing),
            pull(1 << 20),
            pull(5 << 20),
            Progress::phase(Phase::Creating),
            Progress::phase(Phase::Booting),
            Progress::phase(Phase::WaitingForServices),
            Progress::phase(Phase::Ready),
        ]
        .iter()
        .map(|p| lines.render(p, false, now))
        .collect();
        assert_eq!(
            out,
            [
                Some("Preparing\n".to_string()),
                Some(
                    "Downloading image 0.0 of 1.0 GB \u{b7} 95 MB/s \u{b7} less than a minute\n"
                        .into()
                ),
                None,
                Some("Creating\n".into()),
                Some("Starting\n".into()),
                Some("Waiting for its services\n".into()),
                None,
            ]
        );
        assert_eq!(lines.finish_text(true, false, now), None);
        for line in out.iter().flatten() {
            assert!(!line.contains('\x1b') && !line.contains('\r'), "{line:?}");
        }
        // A Linux container sandbox's words, and a VM's.
        let c = super::CreateWords::for_image("ghcr.io/trycua/linux:24.04", "container", false);
        assert_eq!(c.step(Phase::Creating), Some("Creating"));
        assert_eq!(c.step(Phase::Booting), Some("Starting"));
        let vm = super::CreateWords::for_image("linux", "vm", false);
        assert_eq!(vm.step(Phase::Creating), Some("Preparing the disk"));
        assert_eq!(vm.step(Phase::Booting), Some("Booting"));
    }

    #[test]
    fn elapsed_time_reads_in_seconds_and_minutes() {
        use std::time::Duration;
        assert_eq!(super::elapsed_text(Duration::from_millis(1234)), "1.2 s");
        assert_eq!(super::elapsed_text(Duration::from_secs(12)), "12 s");
        assert_eq!(super::elapsed_text(Duration::from_secs(125)), "2 min 5 s");
    }
}

#[cfg(all(test, not(windows)))]
mod browser_tests {
    /// A real sign-in URL: several `&`-joined parameters (CUA-1701).
    const SIGN_IN: &str = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/auth?response_type=code&client_id=cua-cli&redirect_uri=http%3A%2F%2F127.0.0.1%3A54968%2Fcallback&scope=openid&state=s&code_challenge=c&code_challenge_method=S256";

    #[test]
    fn the_url_reaches_the_opener_whole() {
        let (program, args) = super::launch_command(SIGN_IN);
        assert_ne!(program, "cmd");
        assert_eq!(args, vec![SIGN_IN.to_string()]);
    }
}
