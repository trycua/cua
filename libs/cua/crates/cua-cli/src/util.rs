//! Shared helpers: paths, output, prompts, browser, HTTP.

use cua_sdk::CuaError;
use std::{
    io::{IsTerminal, Write},
    path::PathBuf,
    time::Duration,
};

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

/// What is being created, for the words of its steps.
#[derive(Debug, Clone, Copy, Default)]
pub struct CreateWords {
    /// A macOS VM (Lume downloads its base image once).
    pub macos: bool,
    /// A VM (it has a disk to prepare and boots).
    pub vm: bool,
    /// A Space (Cua starts inside it), not a bare sandbox.
    pub space: bool,
}

impl CreateWords {
    /// The words for an image reference or alias (`macos`, `ghcr.io/trycua/macos:26`).
    pub fn for_image(image: &str, kind: &str, space: bool) -> Self {
        let image = image.to_ascii_lowercase();
        let macos = image.contains("macos");
        Self {
            macos,
            vm: macos || image.contains("windows") || kind == "vm",
            space,
        }
    }

    /// The step `phase` in words; `None` for `ready` and phases this build
    /// does not know.
    pub fn step(&self, phase: cua_sandbox_core::progress::Phase) -> Option<&'static str> {
        use cua_sandbox_core::progress::Phase;
        Some(match phase {
            Phase::Preparing => "Preparing",
            Phase::Pulling if self.macos => "Downloading macOS (first time only)",
            Phase::Pulling => "Downloading image",
            Phase::Creating if self.vm => "Preparing the disk",
            Phase::Creating => "Creating",
            Phase::Booting if self.macos => "Booting macOS",
            Phase::Booting if self.vm => "Booting",
            Phase::Booting => "Starting",
            Phase::WaitingForServices if self.space => "Starting Cua inside the Space",
            Phase::WaitingForServices => "Waiting for its services",
            Phase::Connecting => "Connecting",
            Phase::Ready => return None,
        })
    }

    /// One report as a line: the step, then the download's bytes, rate and
    /// time left ("4.2 of 22.1 GB · 85 MB/s · about 4 min"), else its
    /// percent.
    pub fn line(&self, p: &cua_sandbox_core::progress::Progress) -> Option<String> {
        let step = self.step(p.phase)?;
        let measure = match (p.bytes, p.fraction) {
            (Some(b), _) => b.describe(),
            (None, Some(f)) => format!("{:.0}%", f * 100.0),
            (None, None) => String::new(),
        };
        Some(format!("{step} {measure}").trim_end().to_string())
    }
}

/// A create's steps on a terminal (stderr): each step keeps a line of its
/// own, and the step running now updates in place with its bytes. Never
/// used for `--json` or when stderr is not a terminal.
#[derive(Default)]
pub struct StepLines {
    words: CreateWords,
    /// The step on the current line, and that line's text.
    shown: std::sync::Mutex<Option<(cua_sandbox_core::progress::Phase, String)>>,
}

impl StepLines {
    /// Lines for a create of this kind.
    pub fn new(words: CreateWords) -> Self {
        Self {
            words,
            shown: Default::default(),
        }
    }

    /// What to write for `p`: `None` when the line would not change.
    pub fn render(&self, p: &cua_sandbox_core::progress::Progress) -> Option<String> {
        let text = self.words.line(p)?;
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        let out = match shown.as_ref() {
            Some((_, last)) if *last == text => return None,
            // Still the same step: rewrite its line.
            Some((phase, _)) if *phase == p.phase => format!("\r\x1b[2K{text}"),
            // A new step: the last one stays on its line.
            Some(_) => format!("\n{text}"),
            None => text.clone(),
        };
        *shown = Some((p.phase, text));
        Some(out)
    }

    /// Writes `p`'s line to stderr.
    pub fn show(&self, p: &cua_sandbox_core::progress::Progress) {
        if let Some(out) = self.render(p) {
            let mut err = std::io::stderr();
            let _ = write!(err, "{out}");
            let _ = err.flush();
        }
    }

    /// Ends the output: the steps done stay, the running step's line goes.
    pub fn finish(&self) {
        if self
            .shown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .is_some()
        {
            eprint!("\r\x1b[2K");
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
        for h in [
            "x-cua-fleet-claim",
            "accept",
            "content-type",
            "mcp-session-id",
        ] {
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
        assert_eq!(header_for_display("x-cua-fleet-claim", "box", false), "box");
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
    fn create_steps_read_in_words_and_keep_a_line_each() {
        use cua_sandbox_core::progress::{Phase, Progress, Transfer};
        let mac = super::CreateWords::for_image("macos", "", true);
        let lines = super::StepLines::new(mac);
        let gb = 1u64 << 30;
        let pull = |done: u64| {
            Progress::phase(Phase::Pulling).bytes(Transfer {
                done,
                total: 30 * gb,
                per_second: Some((85u64 << 20) as f64),
            })
        };
        let out: Vec<Option<String>> = vec![
            lines.render(&Progress::phase(Phase::Preparing)),
            lines.render(&pull(gb * 124 / 10)),
            lines.render(&pull(gb * 124 / 10)),
            lines.render(&pull(30 * gb)),
            lines.render(&Progress::phase(Phase::Creating)),
            lines.render(&Progress::phase(Phase::Booting)),
            lines.render(&Progress::phase(Phase::WaitingForServices)),
            lines.render(&Progress::phase(Phase::Ready)),
        ];
        assert_eq!(
            out,
            [
                Some("Preparing".to_string()),
                Some(
                    "\nDownloading macOS (first time only) 12.4 of 30.0 GB \u{b7} 85 MB/s \u{b7} about 4 min"
                        .into()
                ),
                None,
                Some(
                    "\r\x1b[2KDownloading macOS (first time only) 30.0 of 30.0 GB \u{b7} 85 MB/s \u{b7} less than a minute"
                        .into()
                ),
                Some("\nPreparing the disk".into()),
                Some("\nBooting macOS".into()),
                Some("\nStarting Cua inside the Space".into()),
                None,
            ]
        );
        // A Linux container sandbox.
        let c = super::CreateWords::for_image("ghcr.io/trycua/linux:24.04", "container", false);
        assert_eq!(c.step(Phase::Creating), Some("Creating"));
        assert_eq!(c.step(Phase::Booting), Some("Starting"));
        assert_eq!(
            c.step(Phase::WaitingForServices),
            Some("Waiting for its services")
        );
        let vm = super::CreateWords::for_image("linux", "vm", false);
        assert_eq!(vm.step(Phase::Creating), Some("Preparing the disk"));
        assert_eq!(vm.step(Phase::Booting), Some("Booting"));
    }
}

#[cfg(test)]
mod browser_tests {
    /// A real sign-in URL: several `&`-joined parameters (CUA-1701).
    const SIGN_IN: &str = "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/auth?response_type=code&client_id=cua-cli&redirect_uri=http%3A%2F%2F127.0.0.1%3A54968%2Fcallback&scope=openid&state=s&code_challenge=c&code_challenge_method=S256";

    #[cfg(not(windows))]
    #[test]
    fn the_url_reaches_the_opener_whole() {
        let (program, args) = super::launch_command(SIGN_IN);
        assert_ne!(program, "cmd");
        assert_eq!(args, vec![SIGN_IN.to_string()]);
    }
}
