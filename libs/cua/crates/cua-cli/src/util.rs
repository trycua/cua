//! Shared helpers: paths, output, prompts, browser, HTTP.

use cua_home::channel::CuaChannel;
use cua_sdk::CuaError;
use std::{
    collections::HashSet,
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

/// Raises this process's soft open-file limit toward 65,536 (the daemon
/// holds a socket or pipe per Space, client, stream and engine
/// connection); the SDK does the same when its runtime starts. Returns the
/// soft limit before and after, or `None` when nothing changed.
pub fn raise_open_file_limit() -> Option<(u64, u64)> {
    cua_sdk::support::raise_open_file_limit()
}

#[cfg(all(test, unix))]
mod fd_limit_tests {
    #[test]
    fn the_daemon_gets_more_than_the_default_open_file_limit() {
        let _ = super::raise_open_file_limit();
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit fills the struct it is given.
        assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) }, 0);
        assert!(lim.rlim_cur >= lim.rlim_max.min(1024), "{}", lim.rlim_cur);
        // Raising again is a no-op.
        assert_eq!(super::raise_open_file_limit(), None);
    }
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

/// The name of this build's CLI, for a message that holds one command's name
/// apart from its arguments (a `cua-local sb create ...` command in a JSON
/// list): `cua`, or `cua-local` on Local. Prose goes through [`hint`].
pub fn cli() -> &'static str {
    CuaChannel::current().cli_name()
}

/// Writes a hint line: [`line`] with each `cua <verb>` command it tells the
/// reader to run named for this build's CLI (`cua-local` on Local, see
/// [`hint`]). Stable's output is unchanged. For messages we wrote, never for
/// data (a Space's output, names, JSON).
pub fn hint_line(out: &mut dyn Write, s: impl AsRef<str>) {
    line(out, hint(s.as_ref()));
}

/// `s` with each command it tells the reader to run named for this build's
/// CLI, so "run `cua host setup`" says `cua-local host setup` on Local and a
/// pasted hint never runs the Stable CLI. Stable's text is returned as is.
/// Two spellings are rewritten: a backticked `` `cua <anything>` `` and a bare
/// `cua <verb>` ("Run it: cua spaces create ...") where `<verb>` is one of this
/// CLI's commands, so prose such as "cua will not ask again" is left alone.
/// Text inside double quotes is left alone too: that is where a message puts
/// a name the user chose (`{name:?}`), and a host called "cua host" is not a
/// command. For messages we wrote; where a message names a user's value next
/// to a command, spell the command with [`cli`] instead.
pub fn hint(s: &str) -> String {
    hint_for(CuaChannel::current(), s)
}

fn hint_for(channel: CuaChannel, s: &str) -> String {
    if !channel.is_local() {
        return s.to_string();
    }
    rename_commands(s, channel.cli_name(), cli_verbs(), true)
}

/// Help text we wrote (about, examples, argument help) for `channel`, given
/// the names of the CLI's commands: [`hint`] without the quote rule, since
/// there is no user's value in it (an example can quote an argument).
pub(crate) fn localize_for(channel: CuaChannel, s: &str, verbs: &HashSet<String>) -> String {
    if !channel.is_local() {
        return s.to_string();
    }
    rename_commands(s, channel.cli_name(), verbs, false)
}

/// The names (and aliases) of the CLI's commands, such as `host` and `sb`.
fn cli_verbs() -> &'static HashSet<String> {
    static VERBS: std::sync::OnceLock<HashSet<String>> = std::sync::OnceLock::new();
    VERBS.get_or_init(|| verbs_of(&crate::cli_command(true)))
}

/// The names (and aliases) of `cli`'s top-level commands.
pub(crate) fn verbs_of(cli: &clap::Command) -> HashSet<String> {
    cli.get_subcommands()
        .flat_map(|c| std::iter::once(c.get_name()).chain(c.get_all_aliases()))
        .map(str::to_string)
        .collect()
}

/// `text` with each command it names (a backticked `` `cua ...` ``, or a bare
/// `cua <verb>` that is not part of a longer word, path or name) starting with
/// `name` instead of `cua`. With `skip_quoted`, not inside double quotes.
fn rename_commands(text: &str, name: &str, verbs: &HashSet<String>, skip_quoted: bool) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut copied = 0;
    for (at, _) in text.match_indices("cua ") {
        let before = &text[..at];
        let prev = before.chars().next_back();
        let starts_word = prev.is_none_or(|c| !(c.is_alphanumeric() || "-_./~".contains(c)));
        let rest = &text[at + 4..];
        let verb_len = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(rest.len());
        let is_command = prev == Some('`') || verbs.contains(&rest[..verb_len]);
        if starts_word && is_command && !(skip_quoted && inside_double_quotes(before)) {
            out.push_str(&text[copied..at]);
            out.push_str(name);
            copied = at + 3;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Whether the end of `before` is inside a double-quoted span (an unescaped
/// `"` opens one and the next closes it).
fn inside_double_quotes(before: &str) -> bool {
    let (mut open, mut escaped) = (false, false);
    for c in before.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => open = !open,
            _ => {}
        }
    }
    open
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
    const VARS: [&str; 5] = [
        "CUA_HOME",
        "CUA_SPACESD_BIN",
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

#[cfg(test)]
mod hint_tests {
    use super::*;

    const HINT: &str = "run `cua host setup` (or `cua auth login`); then cua spaces ls, \
                        cua sb view x, the cua daemon's log, cua will not ask, ~/.cua config, \
                        `cua` and Cua spaces ls";

    #[test]
    fn stable_hints_are_unchanged() {
        assert_eq!(hint_for(CuaChannel::Stable, HINT), HINT);
    }

    #[test]
    fn local_hints_name_the_local_cli() {
        assert_eq!(
            hint_for(CuaChannel::Local, HINT),
            "run `cua-local host setup` (or `cua-local auth login`); then cua-local spaces ls, \
             cua-local sb view x, the cua-local daemon's log, cua will not ask, ~/.cua config, \
             `cua` and Cua spaces ls"
        );
        // A command named in prose is a command of this CLI too, and the word
        // after `cua` is the whole verb, not a prefix of one.
        assert_eq!(
            hint_for(CuaChannel::Local, "Not logged in. Run 'cua auth login'."),
            "Not logged in. Run 'cua-local auth login'."
        );
        assert_eq!(
            hint_for(CuaChannel::Local, "cua hostile, cua sbx, cua"),
            "cua hostile, cua sbx, cua"
        );
        // A value the user chose sits in double quotes (`{name:?}`): never a
        // command, whatever it says. The command next to it is renamed.
        assert_eq!(
            hint_for(
                CuaChannel::Local,
                r#"no team of yours is called "cua host" (`cua team ls`)"#
            ),
            r#"no team of yours is called "cua host" (`cua-local team ls`)"#
        );
        // An escaped quote inside the value does not end it.
        assert_eq!(
            hint_for(
                CuaChannel::Local,
                r#"host "say \"cua host\" now" added; run cua host status"#
            ),
            r#"host "say \"cua host\" now" added; run cua-local host status"#
        );
        // Help text we wrote has no user's value: an example's quotes do not
        // hide its command.
        let verbs = cli_verbs();
        assert_eq!(
            localize_for(
                CuaChannel::Local,
                r#"say "cua host setup" then cua sb ls"#,
                verbs
            ),
            r#"say "cua-local host setup" then cua-local sb ls"#
        );
        assert_eq!(
            localize_for(CuaChannel::Stable, r#"say "cua host setup""#, verbs),
            r#"say "cua host setup""#
        );
        // Not a command of this CLI, part of a longer name or path: untouched.
        for t in [
            "cua foo bar",
            "ai.cua spaces",
            "my-cua spaces",
            "/opt/cua spaces",
            "cua",
            "",
        ] {
            assert_eq!(hint_for(CuaChannel::Local, t), t, "{t}");
        }
    }

    #[test]
    fn the_verbs_are_the_cli_commands() {
        let verbs = cli_verbs();
        for v in [
            "host", "spaces", "sb", "auth", "jobs", "run", "do", "daemon",
        ] {
            assert!(verbs.contains(v), "{v}");
        }
        assert!(!verbs.contains("will"));
    }
}
