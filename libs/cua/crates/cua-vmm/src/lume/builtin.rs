//! Built-in Lume: a pinned, signed Lume that cua downloads into its own
//! directory the first time a macOS Space needs one, so creating a macOS
//! Space never asks for Homebrew, a terminal or an installer.
//!
//! * **Which Lume.** [`LumeSource`] (`cua config set runtime.lume`, the
//!   apps' Settings): `auto` (the default) uses a Lume already on this Mac
//!   when it is at least [`MIN_VERSION`] and the built-in one otherwise;
//!   `builtin` always the built-in one; `system` only the one on this Mac
//!   (whatever its version), and never downloads anything.
//! * **Old servers.** A `lume serve` already answering that is older than
//!   [`MIN_VERSION`] is never stopped (it may be a user's LaunchAgent):
//!   `auto` and `builtin` run a current Lume on the next free port instead
//!   ([`super::LumeRuntime::ensure_serving`]).
//! * **Pinned and verified.** [`VERSION`] from the trycua/cua release, its
//!   SHA-256 checked before anything is unpacked, then `codesign --verify`
//!   and the Developer ID team ([`TEAM_ID`]) checked before it is used. A
//!   new app (and cua) release moves the pin.
//! * **Cua's own directory.** `$CUA_HOME/runtimes/lume/<version>`
//!   ([`dir`]); [`remove`] deletes it. A user's own Lume is never touched.
//! * **Lazy.** Nothing is downloaded until a create needs it
//!   ([`super::LumeRuntime::ensure_serving`]), with the download reported as
//!   the create's progress.

use std::path::{Path, PathBuf};

use cua_home::channel::CuaChannel;

use crate::error::{Result, VmmError};
use crate::host;

/// The pinned Lume release.
pub const VERSION: &str = "0.6.0";
/// Its Apple silicon tarball (`lume` launcher + signed `lume.app`).
pub const URL: &str =
    "https://github.com/trycua/cua/releases/download/lume-v0.6.0/lume-0.6.0-darwin-arm64.tar.gz";
/// The tarball's SHA-256 (the release manifest's).
pub const SHA256: &str = "4d25c7c36ebd3fdf0e2f97f9e7a2c4ff2d0538ba9eb2d536f6dbb542d9d504a7";
/// The tarball's size, for progress and the disk check.
pub const SIZE: u64 = 6_114_070;
/// Cua AI, Inc.'s Developer ID team: the only signer accepted.
pub const TEAM_ID: &str = "YCK386LBJ7";

/// The setting that picks the Lume: `runtime.lume` in
/// `$CUA_HOME/config.toml`, overridden by `CUA_RUNTIME_LUME`.
pub const SETTING_ENV: &str = "CUA_RUNTIME_LUME";

/// Which Lume macOS Spaces run on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LumeSource {
    /// This Mac's Lume when there is one at least [`MIN_VERSION`], else
    /// the built-in one.
    #[default]
    Auto,
    /// Always the built-in one.
    Builtin,
    /// Only this Mac's own Lume (nothing is downloaded).
    System,
}

impl LumeSource {
    /// `auto`, `builtin` or `system`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "builtin" | "built-in" => Some(Self::Builtin),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// The setting's word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Builtin => "builtin",
            Self::System => "system",
        }
    }

    /// The setting now: the environment, else the config file, else auto.
    /// Read on every use, so a change in Settings applies at once.
    pub fn current() -> Self {
        Self::of(crate::kinds::source(crate::kinds::RuntimeKind::Vm))
    }

    /// `[runtime] vm` (or the older `lume`) in `path` (auto when
    /// absent or unreadable).
    pub fn from_config(path: &Path) -> Self {
        Self::of(crate::kinds::source_in(crate::kinds::RuntimeKind::Vm, path))
    }

    fn of(s: crate::kinds::KindSource) -> Self {
        match s {
            crate::kinds::KindSource::Auto => Self::Auto,
            crate::kinds::KindSource::Builtin => Self::Builtin,
            crate::kinds::KindSource::System => Self::System,
        }
    }

    /// Whether the built-in Lume may be used (and downloaded).
    pub fn allows_builtin(self) -> bool {
        self != Self::System
    }
}

/// Where the built-in Lume lives.
pub fn dir() -> PathBuf {
    root().join(VERSION)
}

/// `$CUA_HOME/runtimes/lume`: every built-in Lume version.
pub fn root() -> PathBuf {
    root_in(&host::cua_home())
}

/// [`root`] under the cua home `home`.
pub fn root_in(home: &Path) -> PathBuf {
    home.join("runtimes").join("lume")
}

/// The built-in Lume's executable, when it is installed and was verified.
pub fn installed() -> Option<PathBuf> {
    let d = dir();
    let bin = d.join("lume.app/Contents/MacOS/lume");
    (bin.is_file() && d.join(VERIFIED).is_file()).then_some(bin)
}

/// Written last, once the unpacked Lume passed every check.
const VERIFIED: &str = ".verified";

/// The Lume to run for `source` on this process's channel
/// ([`resolve_for`]).
pub fn resolve(source: LumeSource) -> Option<PathBuf> {
    resolve_for(CuaChannel::current(), source)
}

/// The Lume to run for `source` on `channel`: this Mac's own (`system`, or
/// `auto` when it has one at least [`MIN_VERSION`]), else the built-in one
/// when installed.
///
/// This Mac's own Lume is the one the channel names
/// ([`CuaChannel::lume_bin_name`]): Stable's `lume`, Local's `lume-local`.
/// Local never picks up Stable's `lume`, which keeps its VMs in `~/.lume`
/// and its server on port 7777.
pub fn resolve_for(channel: CuaChannel, source: LumeSource) -> Option<PathBuf> {
    let system = || system_lume(channel, host::which, &root());
    match source {
        // `runtime.vm_path` overrides the one found on PATH.
        LumeSource::System => crate::kinds::custom_path(crate::kinds::RuntimeKind::Vm)
            .filter(|p| p.is_file())
            .or_else(system),
        LumeSource::Builtin => installed(),
        LumeSource::Auto => system()
            .filter(|p| system_binary_usable(source, binary_version_for(channel, p).as_deref()))
            .or_else(installed),
    }
}

/// This Mac's own Lume for `channel`, looked up with `which` by the name
/// the channel gives it; never the built-in one under `builtin_root`.
fn system_lume(
    channel: CuaChannel,
    which: impl Fn(&str) -> Option<PathBuf>,
    builtin_root: &Path,
) -> Option<PathBuf> {
    which(channel.lume_bin_name()).filter(|p| !p.starts_with(builtin_root))
}

/// The oldest Lume cua runs macOS Spaces on. Lume 0.6.0 is the first
/// release with what cua's client relies on (forced and bounded stop for
/// detached VMs, running without a VNC listener, GPU passthrough), and a
/// real 0.5.3 `lume serve` hung booting the macOS 26 image ("stopped while
/// booting") where 0.6.0 boots it. It is the pin today; it only moves when
/// cua starts needing something newer, not with every pin.
pub const MIN_VERSION: &str = "0.6.0";

/// The first `x.y.z` in `text` (`lume --version` prints `0.6.0`; a
/// `v0.6.0` or `lume 0.6.0-beta` is read the same way).
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    text.split(|c: char| c.is_whitespace() || c == 'v' || c == '-' || c == '+')
        .find_map(|word| {
            let mut parts = word.split('.');
            let v = (
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            );
            parts.next().is_none().then_some(v)
        })
}

/// Whether `version` is at least [`MIN_VERSION`] (an unreadable one is not).
pub fn new_enough(version: &str) -> bool {
    match (parse_version(version), parse_version(MIN_VERSION)) {
        (Some(v), Some(min)) => v >= min,
        _ => false,
    }
}

/// The version `GET /lume/host/status` reports, when it means something:
/// every Lume up to 0.6.0 answered a fixed placeholder `1.0.0` there.
pub fn reported_version(status_version: Option<&str>) -> Option<String> {
    status_version
        .filter(|v| v.trim() != "1.0.0" && parse_version(v).is_some())
        .map(|v| v.trim().to_string())
}

/// What to do with a `lume serve` that already answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerVerdict {
    /// New enough, or its version cannot be told (a remote `LUME_API`,
    /// another user's process): use it, as before.
    Use,
    /// Too old, but `runtime.lume` is `system`: use it and say so.
    UseOld(String),
    /// Too old: run a current Lume on another port instead. The old server
    /// is never stopped (it may be a user's own LaunchAgent).
    Avoid(String),
}

/// The verdict for a server of `version` under `source`.
pub fn judge_server(source: LumeSource, version: Option<&str>) -> ServerVerdict {
    match version {
        None => ServerVerdict::Use,
        Some(v) if new_enough(v) => ServerVerdict::Use,
        Some(v) if source == LumeSource::System => ServerVerdict::UseOld(v.to_string()),
        Some(v) => ServerVerdict::Avoid(v.to_string()),
    }
}

/// Whether this Mac's `lume` (of `version`, `None` when `--version` said
/// nothing) may run Spaces: always for `system`, else only when new enough.
pub fn system_binary_usable(source: LumeSource, version: Option<&str>) -> bool {
    source == LumeSource::System || version.is_some_and(new_enough)
}

/// The ports tried, in order, for a current Lume when the configured one
/// is taken by an old server: the next nine. Fixed, so every cua process
/// on this Mac finds the same one. Stable's walk (`fallback_ports_for`).
pub fn fallback_ports(port: u16) -> impl Iterator<Item = u16> {
    fallback_ports_for(CuaChannel::Stable, port)
}

/// How many ports after its own one a channel's Lume may move to.
pub const FALLBACK_SPAN: u16 = 9;

/// The ports a channel's Lume may use: its own and the nine after it
/// (Stable's `7777..=7786`, Local's `7787..=7796`).
pub fn port_range(channel: CuaChannel) -> std::ops::RangeInclusive<u16> {
    let p = channel.lume_port();
    p..=p.saturating_add(FALLBACK_SPAN)
}

/// [`fallback_ports`] for `channel`: the next nine ports after `port`,
/// except any in another channel's range, so a Local Lume never walks into
/// Stable's ports (and Stable's never into Local's).
pub fn fallback_ports_for(channel: CuaChannel, port: u16) -> impl Iterator<Item = u16> {
    let others: Vec<_> = CuaChannel::ALL
        .into_iter()
        .filter(move |c| *c != channel)
        .map(port_range)
        .collect();
    (1..=FALLBACK_SPAN)
        .filter_map(move |d| port.checked_add(d))
        .filter(move |p| !others.iter().any(|r| r.contains(p)))
}

/// `url` with its port replaced by `port`.
pub fn with_port(url: &str, port: u16) -> String {
    let url = url.trim_end_matches('/');
    match url.rsplit_once(':') {
        Some((head, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{head}:{port}")
        }
        _ => format!("{url}:{port}"),
    }
}

/// Whether `url` is a server on this Mac.
pub fn is_loopback(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split('/').next().unwrap_or("");
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// `lume --version`, run as `lume` runs (its channel's environment: the
/// pinned Lume initialises its settings and telemetry before it parses
/// `--version`, so a bare run would touch Stable's `~/.config/lume` and
/// `~/.lume`). Cached per binary and modification time; bounded to five
/// seconds.
pub fn binary_version(lume: &LumeCmd) -> Option<String> {
    cached_version(lume.bin(), || run_version(lume))
}

/// [`binary_version`] of `bin` for `channel`, preparing the channel's
/// environment under `config_root` only when the version is not cached.
pub fn binary_version_in(channel: CuaChannel, config_root: &Path, bin: &Path) -> Option<String> {
    cached_version(bin, || {
        let lume = LumeCmd::new(channel, config_root, bin)
            .inspect_err(|e| tracing::warn!(error = %e, "could not prepare the Lume environment"))
            .ok()?;
        run_version(&lume)
    })
}

/// [`binary_version_in`] under the default Lume state directory.
pub fn binary_version_for(channel: CuaChannel, bin: &Path) -> Option<String> {
    binary_version_in(channel, &super::default_root(), bin)
}

fn cached_version(bin: &Path, run: impl FnOnce() -> Option<String>) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    /// Binary -> (its modification time, its version).
    type Seen = HashMap<PathBuf, (Option<std::time::SystemTime>, Option<String>)>;
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
    let bin = std::fs::canonicalize(bin).unwrap_or_else(|_| bin.to_path_buf());
    let mtime = std::fs::metadata(&bin).and_then(|m| m.modified()).ok();
    if let Some((t, v)) = SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .get(&bin)
        && *t == mtime
    {
        return v.clone();
    }
    let v = run();
    SEEN.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(bin, (mtime, v.clone()));
    v
}

fn run_version(lume: &LumeCmd) -> Option<String> {
    use std::io::Read as _;
    use std::process::Stdio;
    let mut child = lume
        .command()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    parse_version(&out).map(|(a, b, c)| format!("{a}.{b}.{c}"))
}

/// The executable of this user's process listening on TCP `port` on this
/// Mac (`lsof`, then `proc_pidpath`); `None` when there is none or it
/// belongs to someone else.
pub fn serving_binary(port: u16) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/lsof")
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let pid: i32 = String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.trim().parse().ok())?;
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: `buf` is writable for its full length, which is passed.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        Some(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = port;
        None
    }
}

/// Deletes every built-in Lume (`$CUA_HOME/runtimes/lume`). A user's own
/// Lume is elsewhere and stays.
pub fn remove() -> Result<()> {
    match std::fs::remove_dir_all(root()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Lume's switch for its product telemetry (`false` turns it off for the
/// process; it wins over the config file).
const TELEMETRY_VAR: &str = "LUME_TELEMETRY_ENABLED";

/// A Lume to run for a channel: its binary and the environment every
/// process started from it needs. **Every** Lume subprocess goes through
/// one of these (`serve`, `pull`, `get`, `run`, `ssh`, `attach`), so none of
/// them can fall back to Stable's VM directory.
///
/// Stable's has no environment: its Lume runs as it always did.
///
/// Local's Lume keeps its VMs in `~/.lume-local`
/// ([`crate::storage::lume_dir_for`]) whichever Lume runs it, so it gets:
///
/// * `XDG_CONFIG_HOME`: a config directory of its own under `config_root`,
///   with a `lume/config.yaml` that makes the VM directory the default
///   location. **This is the isolation for any Lume named `lume`**, the
///   bundled one included, so it never reads or writes Stable's `~/.lume` or
///   `~/.config/lume`;
/// * `LUME_HOME`: the VM directory. **Contract with Lume (#5804):** a Lume
///   named `lume-local` relocates its VMs, cache and settings through
///   `LUME_HOME`; one named `lume` (Stable's, and the bundled one, on 0.6.0
///   and on newer releases alike) uses `LUME_HOME` only for its
///   release-channel file. So `LUME_HOME` alone does not isolate the
///   bundled `lume`: never drop the config above. It is set for
///   `lume-local`, and so the release-channel file is Local's own;
/// * `TMPDIR`: a private temp directory under `config_root`, where Lume
///   pulls images (`lume_vm_*`), so Local's start-up sweep and Stable's
///   never see each other's half-made pulls;
/// * `LUME_TELEMETRY_ENABLED=false` (and `telemetryEnabled: false` in the
///   config): the pinned Lume writes its telemetry identity, locks and
///   markers under `~/.lume` whatever `LUME_HOME` says, which is Stable's.
///   Local's Lume has none until the pin supports an isolated home.
#[derive(Clone, Debug)]
pub struct LumeCmd {
    bin: PathBuf,
    env: Vec<(&'static str, std::ffi::OsString)>,
    /// The private temp directory its `TMPDIR` names (Local's).
    temp: Option<PathBuf>,
}

impl LumeCmd {
    /// `bin` with no extra environment (Stable's, and tests').
    pub fn plain(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            env: Vec::new(),
            temp: None,
        }
    }

    /// `bin` for `channel`; Local's writes its config under `config_root`.
    pub fn new(
        channel: CuaChannel,
        config_root: &Path,
        bin: impl Into<PathBuf>,
    ) -> std::io::Result<Self> {
        Self::with_vm_dir(
            channel,
            config_root,
            bin,
            &crate::storage::lume_dir_for(channel),
        )
    }

    /// [`LumeCmd::new`] with the VM directory given.
    pub fn with_vm_dir(
        channel: CuaChannel,
        config_root: &Path,
        bin: impl Into<PathBuf>,
        vm_dir: &Path,
    ) -> std::io::Result<Self> {
        let bin = bin.into();
        if !channel.is_local() {
            return Ok(Self::plain(bin));
        }
        let xdg = config_root.join("xdg");
        seed_config(&xdg.join("lume").join("config.yaml"), vm_dir)?;
        let temp = config_root.join("tmp");
        std::fs::create_dir_all(&temp)?;
        Ok(Self {
            bin,
            env: vec![
                (crate::storage::LUME_HOME_VAR, vm_dir.as_os_str().to_owned()),
                ("XDG_CONFIG_HOME", xdg.into_os_string()),
                ("TMPDIR", temp.as_os_str().to_owned()),
                (TELEMETRY_VAR, "false".into()),
            ],
            temp: Some(temp),
        })
    }

    /// The temp directory this Lume pulls into, when it has a private one
    /// (Local's). Stable's is the user's shared temp directory.
    pub fn temp_dir(&self) -> Option<&Path> {
        self.temp.as_deref()
    }

    /// The Lume binary.
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// The environment it runs with (empty for Stable).
    pub fn env(&self) -> &[(&'static str, std::ffi::OsString)] {
        &self.env
    }

    /// A command for it, with the environment set.
    pub fn command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.bin);
        cmd.envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }

    /// [`LumeCmd::command`] for `tokio`.
    pub fn tokio_command(&self) -> tokio::process::Command {
        tokio::process::Command::from(self.command())
    }

    /// The program and leading arguments that run it with its environment
    /// when only a program and arguments can be given (`PrefixExec`,
    /// [`super::LumeSshExec`], a command for another process to run): the
    /// binary itself for Stable, `env K=V... <bin>` for Local.
    pub fn wrapped(&self) -> (String, Vec<String>) {
        if self.env.is_empty() {
            return (self.bin.display().to_string(), Vec::new());
        }
        let mut lead: Vec<String> = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", v.to_string_lossy()))
            .collect();
        lead.push(self.bin.display().to_string());
        ("/usr/bin/env".into(), lead)
    }

    /// [`LumeCmd::wrapped`] followed by `args`, as one argv.
    pub fn argv<I, S>(&self, args: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let (program, lead) = self.wrapped();
        let mut argv = vec![program];
        argv.extend(lead);
        argv.extend(args.into_iter().map(Into::into));
        argv
    }
}

/// `s` as a YAML scalar. A JSON string is a double-quoted YAML scalar
/// (YAML 1.2 is a superset of JSON, and Lume's parser reads the escapes
/// JSON writes), so `serde_json` escapes `\`, `"` and control characters
/// instead of them being pasted into the quotes.
fn yaml_string(s: &str) -> String {
    serde_json::to_string(s).expect("a string always serializes")
}

/// Lume's `config.yaml` that makes `vm_dir` the one VM location. The paths
/// are escaped ([`yaml_string`]): a `LUME_HOME` with a backslash or a quote
/// in it still reads back as the same directory.
pub fn config_yaml(vm_dir: &str) -> String {
    let dir = yaml_string(vm_dir);
    let cache = yaml_string(&format!("{vm_dir}/cache"));
    format!(
        "# Written by Cua Spaces Local: its Lume keeps VMs apart from the released Cua Spaces.\n\
         defaultLocationName: \"home\"\n\
         cacheDirectory: {cache}\n\
         cachingEnabled: false\n\
         telemetryEnabled: false\n\
         \n\
         vmLocations:\n\
         \x20 - name: \"home\"\n\
         \x20   path: {dir}\n"
    )
}

/// Writes the Local Lume's `config.yaml` at `file` unless it already makes
/// `vm_dir` the default location with telemetry off.
fn seed_config(file: &Path, vm_dir: &Path) -> std::io::Result<()> {
    let dir = vm_dir.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} cannot be a Lume VM directory", vm_dir.display()),
        )
    })?;
    let current = std::fs::read_to_string(file).ok();
    if current.as_deref().is_some_and(|y| {
        crate::storage::lume_default_location(y).is_some_and(|p| p == dir)
            && y.lines().any(|l| l.trim() == "telemetryEnabled: false")
    }) {
        return Ok(());
    }
    let parent = file.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    // Written whole to a temp file beside it and renamed over it: a Lume
    // started meanwhile reads the old config or the new one, never an
    // empty or partial file (which the pinned Lume answers with `~/.lume`).
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut tmp, config_yaml(dir).as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(file).map_err(|e| e.error)?;
    Ok(())
}

/// One install at a time in this process (a second create waits for it).
static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Installs the built-in Lume when it is not there yet; its executable.
/// The download is reported as the running create's progress.
#[cfg(feature = "lume")]
pub async fn ensure() -> Result<PathBuf> {
    if let Some(bin) = installed() {
        return Ok(bin);
    }
    let _one = INSTALL.lock().await;
    if let Some(bin) = installed() {
        return Ok(bin);
    }
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return Err(VmmError::missing(
            "Lume",
            "macOS Spaces need a Mac with Apple silicon",
        ));
    }
    std::fs::create_dir_all(root())?;
    crate::disk::ensure_space(&root(), SIZE * 5, "set up the built-in Lume")?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root())?;
    let tarball = staging.path().join("lume.tar.gz");
    download(URL, &tarball).await?;
    let got = sha256_file(&tarball)?;
    if got != SHA256 {
        return Err(VmmError::other(format!(
            "the built-in Lume download did not match its checksum (got {got}); try again"
        )));
    }
    let unpacked = staging.path().join("lume");
    std::fs::create_dir_all(&unpacked)?;
    host::run(
        "/usr/bin/tar",
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &unpacked.to_string_lossy(),
        ],
    )
    .await?;
    verify_signature(&unpacked.join("lume.app")).await?;
    std::fs::write(unpacked.join(VERIFIED), VERSION)?;
    let target = dir();
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&unpacked, &target)?;
    tracing::info!(version = VERSION, dir = %target.display(), "installed the built-in Lume");
    installed().ok_or_else(|| VmmError::other("the built-in Lume did not install"))
}

/// Streams `url` to `to`, reporting bytes as create progress.
#[cfg(feature = "lume")]
async fn download(url: &str, to: &Path) -> Result<()> {
    use crate::progress::{Meter, Phase, Progress, report};
    use tokio::io::AsyncWriteExt as _;
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| VmmError::other(format!("download the built-in Lume: {e}")))?;
    let mut res = http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| VmmError::other(format!("download the built-in Lume: {e}")))?;
    let total = res.content_length().unwrap_or(SIZE);
    let mut file = tokio::fs::File::create(to).await?;
    let mut meter = Meter::new();
    let mut done = 0u64;
    let detail = format!("Setting up Lume {VERSION} for macOS Spaces");
    report(Progress::phase(Phase::Preparing).detail(detail.clone()));
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| VmmError::other(format!("download the built-in Lume: {e}")))?
    {
        if done + chunk.len() as u64 > SIZE * 4 {
            return Err(VmmError::other("the built-in Lume download is too large"));
        }
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if let Some(t) = meter.sample_at(std::time::Instant::now(), done, total) {
            report(
                Progress::phase(Phase::Preparing)
                    .detail(detail.clone())
                    .bytes(t)
                    .fraction(done as f64 / total.max(1) as f64),
            );
        }
    }
    file.flush().await?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `codesign --verify --strict` passes and the signer is [`TEAM_ID`].
async fn verify_signature(app: &Path) -> Result<()> {
    let app = app.to_string_lossy();
    host::run(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", &app],
    )
    .await
    .map_err(|e| VmmError::other(format!("the built-in Lume is not validly signed: {e}")))?;
    let out = tokio::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2", &app])
        .output()
        .await?;
    // codesign -d prints to stderr.
    let text = String::from_utf8_lossy(&out.stderr);
    if !team_matches(&text) {
        return Err(VmmError::other(
            "the built-in Lume is not signed by Cua AI, Inc.",
        ));
    }
    Ok(())
}

fn team_matches(codesign_display: &str) -> bool {
    codesign_display
        .lines()
        .any(|l| l.trim() == format!("TeamIdentifier={TEAM_ID}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_parses_and_defaults_to_auto() {
        assert_eq!(LumeSource::parse("Built-in"), Some(LumeSource::Builtin));
        assert_eq!(LumeSource::parse("system"), Some(LumeSource::System));
        assert_eq!(LumeSource::parse("nope"), None);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        assert_eq!(LumeSource::from_config(&p), LumeSource::Auto);
        std::fs::write(
            &p,
            "[default]\non = \"local\"\n\n[runtime]\nlume = \"system\"\n",
        )
        .unwrap();
        assert_eq!(LumeSource::from_config(&p), LumeSource::System);
        assert!(!LumeSource::System.allows_builtin());
        assert!(LumeSource::Auto.allows_builtin());
    }

    #[test]
    fn versions_parse_and_compare_against_the_minimum() {
        assert_eq!(parse_version("0.6.0\n"), Some((0, 6, 0)));
        assert_eq!(parse_version("lume v0.5.3"), Some((0, 5, 3)));
        assert_eq!(parse_version("0.7.1-beta.2"), Some((0, 7, 1)));
        assert_eq!(parse_version("Error: unknown option"), None);
        assert_eq!(parse_version("1.2"), None);
        assert!(!new_enough("0.5.3"));
        assert!(new_enough("0.6.0"));
        assert!(new_enough("0.10.0"), "numeric, not lexical");
        assert!(new_enough("1.0.0"));
        assert!(!new_enough("garbage"));
        assert!(new_enough(VERSION), "the pinned Lume is always new enough");
    }

    #[test]
    fn host_status_s_placeholder_version_says_nothing() {
        assert_eq!(reported_version(Some("1.0.0")), None);
        assert_eq!(reported_version(None), None);
        assert_eq!(reported_version(Some("nope")), None);
        assert_eq!(reported_version(Some("0.6.1")), Some("0.6.1".into()));
    }

    #[test]
    fn an_old_server_is_avoided_unless_system_is_chosen() {
        use ServerVerdict::*;
        for s in [LumeSource::Auto, LumeSource::Builtin] {
            assert_eq!(judge_server(s, Some("0.5.3")), Avoid("0.5.3".into()));
            assert_eq!(judge_server(s, Some("0.6.0")), Use);
            assert_eq!(judge_server(s, None), Use, "unknown: as before");
        }
        assert_eq!(
            judge_server(LumeSource::System, Some("0.5.3")),
            UseOld("0.5.3".into())
        );
        assert_eq!(judge_server(LumeSource::System, Some("0.6.0")), Use);
    }

    #[test]
    fn auto_uses_this_mac_s_lume_only_when_new_enough() {
        assert!(!system_binary_usable(LumeSource::Auto, Some("0.5.3")));
        assert!(!system_binary_usable(LumeSource::Auto, None));
        assert!(system_binary_usable(LumeSource::Auto, Some("0.6.0")));
        assert!(system_binary_usable(LumeSource::System, Some("0.5.3")));
        assert!(system_binary_usable(LumeSource::System, None));
    }

    #[test]
    fn fallback_ports_and_urls() {
        assert_eq!(
            fallback_ports(7777).collect::<Vec<_>>(),
            (7778..=7786).collect::<Vec<_>>()
        );
        assert_eq!(fallback_ports(u16::MAX).count(), 0);
        assert_eq!(
            with_port("http://127.0.0.1:7777", 7778),
            "http://127.0.0.1:7778"
        );
        assert_eq!(
            with_port("http://127.0.0.1:7777/", 7779),
            "http://127.0.0.1:7779"
        );
        assert_eq!(with_port("http://localhost", 7778), "http://localhost:7778");
        assert!(is_loopback("http://127.0.0.1:7777"));
        assert!(is_loopback("http://localhost:7777/"));
        assert!(is_loopback("http://[::1]:7777"));
        assert!(!is_loopback("http://10.0.0.5:7777"));
    }

    #[test]
    fn stable_keeps_its_ports_and_local_has_a_range_of_its_own() {
        use CuaChannel::{Local, Stable};
        assert_eq!(Stable.lume_port(), 7777);
        assert_eq!(Local.lume_port(), 7787);
        assert_eq!(port_range(Stable), 7777..=7786);
        assert_eq!(port_range(Local), 7787..=7796);
        // Stable is unchanged.
        let stable: Vec<u16> = fallback_ports_for(Stable, 7777).collect();
        assert_eq!(stable, (7778..=7786).collect::<Vec<_>>());
        assert_eq!(fallback_ports(7777).collect::<Vec<_>>(), stable);
        // Local walks 7788-7796 from its own port.
        assert_eq!(
            fallback_ports_for(Local, 7787).collect::<Vec<_>>(),
            (7788..=7796).collect::<Vec<_>>()
        );
        // Neither walks into the other's range, even from an overridden port.
        for from in 7770..7800 {
            assert!(
                fallback_ports_for(Local, from).all(|p| !port_range(Stable).contains(&p)),
                "Local from {from}"
            );
            assert!(
                fallback_ports_for(Stable, from).all(|p| !port_range(Local).contains(&p)),
                "Stable from {from}"
            );
        }
        assert_eq!(fallback_ports_for(Local, u16::MAX).count(), 0);
    }

    #[test]
    fn each_channel_looks_for_its_own_lume_binary() {
        use CuaChannel::{Local, Stable};
        let root = Path::new("/h/.cua/runtimes/lume");
        let on_path = |names: &'static [&'static str]| {
            move |n: &str| {
                names
                    .contains(&n)
                    .then(|| PathBuf::from("/Users/a/.local/bin").join(n))
            }
        };
        // Local prefers an installed `lume-local`, and never takes `lume`.
        assert_eq!(
            system_lume(Local, on_path(&["lume", "lume-local"]), root),
            Some(PathBuf::from("/Users/a/.local/bin/lume-local"))
        );
        assert_eq!(system_lume(Local, on_path(&["lume"]), root), None);
        // Stable is unchanged: `lume`.
        assert_eq!(
            system_lume(Stable, on_path(&["lume", "lume-local"]), root),
            Some(PathBuf::from("/Users/a/.local/bin/lume"))
        );
        assert_eq!(system_lume(Stable, on_path(&["lume-local"]), root), None);
        // The built-in one is not "this Mac's own".
        assert_eq!(
            system_lume(Local, |_| Some(root.join("0.6.0/lume-local")), root),
            None
        );
    }

    fn lume_for(channel: CuaChannel, root: &Path, vms: &Path) -> std::io::Result<LumeCmd> {
        LumeCmd::with_vm_dir(channel, root, "/opt/lume", vms)
    }

    #[test]
    fn stable_s_lume_is_started_as_it_always_was() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = lume_for(CuaChannel::Stable, dir.path(), Path::new("/h/.lume")).unwrap();
        assert!(cmd.env().is_empty(), "no telemetry or temp dir overrides");
        assert_eq!(cmd.temp_dir(), None);
        assert_eq!(cmd.wrapped(), ("/opt/lume".to_string(), Vec::new()));
        assert_eq!(cmd.argv(["get", "vm"]), ["/opt/lume", "get", "vm"]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn local_s_lume_is_started_on_its_own_vm_directory_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let vms = Path::new("/Users/a/.lume-local");
        let cmd = lume_for(CuaChannel::Local, dir.path(), vms).unwrap();
        let get = |k: &str| {
            cmd.env()
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("LUME_HOME"), Some(vms.into()));
        let xdg = PathBuf::from(get("XDG_CONFIG_HOME").unwrap());
        assert!(xdg.starts_with(dir.path()));
        // A Lume that does not read LUME_HOME yet reads this config instead.
        let yaml = std::fs::read_to_string(xdg.join("lume/config.yaml")).unwrap();
        assert_eq!(
            crate::storage::lume_default_location(&yaml).as_deref(),
            Some("/Users/a/.lume-local")
        );
        assert!(yaml.contains("cacheDirectory: \"/Users/a/.lume-local/cache\""));
        assert!(
            !yaml.contains("\"~/.lume\""),
            "never Stable's default location"
        );
        // The pinned Lume writes its telemetry identity under ~/.lume
        // whatever LUME_HOME says: Local's has telemetry off, by the
        // environment (which wins) and in its config.
        assert!(yaml.contains("telemetryEnabled: false"));
        assert_eq!(get("LUME_TELEMETRY_ENABLED"), Some("false".into()));
        assert_eq!(get("TMPDIR"), Some(dir.path().join("tmp").into()));
        // Where only a program and arguments can be given, the same
        // environment goes through `env`.
        let argv = cmd.argv(["attach", "vm"]);
        assert_eq!(argv[0], "/usr/bin/env");
        assert!(argv.contains(&"LUME_HOME=/Users/a/.lume-local".to_string()));
        assert!(argv.iter().any(|a| a.starts_with("XDG_CONFIG_HOME=")));
        assert_eq!(&argv[argv.len() - 3..], ["/opt/lume", "attach", "vm"]);
    }

    #[test]
    fn the_local_config_is_kept_while_it_is_right_and_fixed_when_it_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xdg/lume/config.yaml");
        let vms = Path::new("/Users/a/.lume-local");
        lume_for(CuaChannel::Local, dir.path(), vms).unwrap();
        // Right: left alone (Lume may have added its own settings).
        let kept = format!(
            "{}# added by Lume\n",
            std::fs::read_to_string(&file).unwrap()
        );
        std::fs::write(&file, &kept).unwrap();
        lume_for(CuaChannel::Local, dir.path(), vms).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), kept);
        // Wrong (say it still names Stable's directory): rewritten.
        std::fs::write(
            &file,
            "defaultLocationName: \"home\"\nvmLocations:\n  - name: \"home\"\n    path: \"~/.lume\"\n",
        )
        .unwrap();
        lume_for(CuaChannel::Local, dir.path(), vms).unwrap();
        assert_eq!(
            crate::storage::lume_default_location(&std::fs::read_to_string(&file).unwrap())
                .as_deref(),
            Some("/Users/a/.lume-local")
        );
        // An older config of Local's that left telemetry on: rewritten.
        std::fs::write(
            &file,
            format!(
                "defaultLocationName: \"home\"\ntelemetryEnabled: true\nvmLocations:\n  - name: \"home\"\n    path: \"{}\"\n",
                vms.display()
            ),
        )
        .unwrap();
        lume_for(CuaChannel::Local, dir.path(), vms).unwrap();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("telemetryEnabled: false")
        );
        // A directory with a quote or a backslash in it is escaped, not
        // refused: it reads back as the same directory.
        for odd in ["/a\"b", r"/a\b"] {
            lume_for(CuaChannel::Local, dir.path(), Path::new(odd)).unwrap();
            let text = std::fs::read_to_string(&file).unwrap();
            assert_eq!(
                crate::storage::lume_default_location(&text).as_deref(),
                Some(odd)
            );
        }
    }

    /// A stand-in `lume` that prints a version and records the environment
    /// it was run with (the pinned Lume initialises settings and telemetry
    /// before it parses `--version`).
    #[cfg(unix)]
    fn recording_lume(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let bin = dir.join("lume");
        let seen = dir.join("seen");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n[ \"$1\" = --version ] || exit 64\n\
                 printf '%s\\n' \"xdg=$XDG_CONFIG_HOME\" \"telemetry=$LUME_TELEMETRY_ENABLED\" \"tmp=$TMPDIR\" \"home=$LUME_HOME\" > '{}'\n\
                 echo 0.6.0\n",
                seen.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, seen)
    }

    /// The version probes (the doctor's and the one that picks a Lume) run
    /// Lume with its channel's environment, so Local's never touches
    /// Stable's `~/.config/lume` or `~/.lume`; Stable's run as they did.
    #[cfg(unix)]
    #[tokio::test]
    async fn version_probes_run_with_the_channel_s_environment() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cua-local/vmm/lume");
        let (local_bin, local_seen) = recording_lume(&t.path().join("local-bin"));
        let (stable_bin, stable_seen) = recording_lume(&t.path().join("stable-bin"));
        let (text_bin, text_seen) = recording_lume(&t.path().join("text-bin"));

        // `builtin::binary_version_in` (server and system-Lume probes).
        assert_eq!(
            binary_version_in(CuaChannel::Local, &root, &local_bin).as_deref(),
            Some("0.6.0")
        );
        let seen = std::fs::read_to_string(&local_seen).unwrap();
        assert!(
            seen.contains(&format!("xdg={}", root.join("xdg").display())),
            "{seen}"
        );
        assert!(seen.contains("telemetry=false"), "{seen}");
        assert!(
            seen.contains(&format!("tmp={}", root.join("tmp").display())),
            "{seen}"
        );
        assert!(seen.contains("home="), "{seen}");
        assert!(!seen.contains("home=\n"), "LUME_HOME unset: {seen}");

        // Stable's probe has no overrides.
        assert_eq!(
            binary_version_in(CuaChannel::Stable, &root, &stable_bin).as_deref(),
            Some("0.6.0")
        );
        let seen = std::fs::read_to_string(&stable_seen).unwrap();
        assert!(
            !seen.contains(&root.display().to_string()) && !seen.contains("telemetry=false"),
            "Stable's probe got Local's environment: {seen}"
        );

        // The doctor's probe (`crate::lume::version_text`).
        let cmd = LumeCmd::with_vm_dir(
            CuaChannel::Local,
            &root,
            &text_bin,
            Path::new("/Users/a/.lume-local"),
        )
        .unwrap();
        assert_eq!(
            crate::lume::version_text(&cmd).await.as_deref(),
            Some("0.6.0")
        );
        let seen = std::fs::read_to_string(&text_seen).unwrap();
        assert!(
            seen.contains("telemetry=false") && seen.contains("xdg="),
            "{seen}"
        );
        assert!(!seen.contains("xdg=\n"), "{seen}");
    }

    /// Local's `config.yaml` is replaced atomically: a Lume started while it
    /// is rewritten reads the old config or the new one, never an empty or
    /// cut-off file (the pinned Lume answers one with `~/.lume`).
    #[test]
    fn a_reader_never_sees_a_partial_local_config() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xdg/lume/config.yaml");
        let a = Path::new("/Users/a/.lume-local");
        let b = Path::new("/Users/b/.lume-local");
        seed_config(&file, a).unwrap();
        let done = AtomicBool::new(false);
        let reads = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            let readers: Vec<_> = (0..3)
                .map(|_| {
                    s.spawn(|| {
                        while !done.load(Ordering::Relaxed) {
                            let yaml = std::fs::read_to_string(&file).expect("always there");
                            let at = crate::storage::lume_default_location(&yaml);
                            assert!(
                                matches!(
                                    at.as_deref(),
                                    Some("/Users/a/.lume-local" | "/Users/b/.lume-local")
                                ),
                                "a partial config: {yaml:?}"
                            );
                            reads.fetch_add(1, Ordering::Relaxed);
                        }
                    })
                })
                .collect();
            // Each writer finds the other's directory in the file, so every
            // call rewrites it.
            let writers: Vec<_> = [a, b]
                .into_iter()
                .map(|vm| {
                    let file = &file;
                    s.spawn(move || {
                        for _ in 0..1500 {
                            seed_config(file, vm).unwrap();
                        }
                    })
                })
                .collect();
            for w in writers {
                w.join().unwrap();
            }
            done.store(true, Ordering::Relaxed);
            for r in readers {
                r.join().unwrap();
            }
        });
        assert!(reads.load(Ordering::Relaxed) > 0);
        // No temp files left beside it.
        let left: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("config.yaml")]);
    }

    /// A VM directory with a backslash, a quote or a newline in it is
    /// escaped, so a real YAML parser (Lume's) reads the same directory
    /// back: it is not decoded as `\t` or `\U`, and it ends where it ends.
    #[test]
    fn the_config_reads_back_the_directory_it_was_written_for() {
        #[derive(serde::Deserialize)]
        struct Location {
            name: String,
            path: String,
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Config {
            default_location_name: String,
            cache_directory: String,
            telemetry_enabled: bool,
            vm_locations: Vec<Location>,
        }
        for dir in [
            "/Users/a/.lume-local",
            r"/Volumes/Fast\tmp\Users\new",
            r"C:\Users\a\.lume-local",
            r#"/Users/a/it's "quoted""#,
            "/Users/a/two\nlines",
            "/Users/a/ünïcode #not a comment: x",
        ] {
            let parsed: Config = serde_yaml_ng::from_str(&config_yaml(dir))
                .unwrap_or_else(|e| panic!("{dir:?}: {e}"));
            assert_eq!(parsed.default_location_name, "home", "{dir:?}");
            assert_eq!(parsed.vm_locations.len(), 1, "{dir:?}");
            assert_eq!(parsed.vm_locations[0].name, "home", "{dir:?}");
            assert_eq!(parsed.vm_locations[0].path, dir);
            assert_eq!(parsed.cache_directory, format!("{dir}/cache"));
            assert!(!parsed.telemetry_enabled);
            // Cua's own reader agrees with the parser.
            assert_eq!(
                crate::storage::lume_default_location(&config_yaml(dir)).as_deref(),
                Some(dir),
                "{dir:?}"
            );
        }
    }

    /// Seeding writes that config and is then a no-op for the same directory
    /// (the file is read back through the escapes), and replaces it atomically
    /// for another.
    #[test]
    fn seeding_a_backslash_directory_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("xdg/lume/config.yaml");
        let vm = Path::new(r"/Volumes/Fast\tmp\Users\new");
        seed_config(&file, vm).unwrap();
        let first = std::fs::read_to_string(&file).unwrap();
        let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&first).unwrap();
        assert_eq!(
            parsed["vmLocations"][0]["path"].as_str(),
            vm.to_str(),
            "{first}"
        );
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        seed_config(&file, vm).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
        let other = Path::new(r"C:\other");
        seed_config(&file, other).unwrap();
        let parsed: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(parsed["vmLocations"][0]["path"].as_str(), other.to_str());
        let left: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("config.yaml")]);
    }

    #[test]
    fn only_cua_s_team_is_accepted() {
        assert!(team_matches(
            "Identifier=com.trycua.lume\nTeamIdentifier=YCK386LBJ7\n"
        ));
        assert!(!team_matches("TeamIdentifier=ABCDE12345\n"));
        assert!(!team_matches("TeamIdentifier=not set\n"));
    }

    #[test]
    fn the_pin_is_a_sha256_and_a_release_url() {
        assert_eq!(SHA256.len(), 64);
        assert!(URL.contains(&format!(
            "lume-v{VERSION}/lume-{VERSION}-darwin-arm64.tar.gz"
        )));
    }

    /// Opt-in (network): downloads the pinned Lume into a temporary
    /// `CUA_HOME`, verifies it, and finds it as `builtin` and `auto`.
    /// `CUA_HOME=<tmp> cargo test -p cua-vmm --lib builtin -- --ignored`.
    #[cfg(all(feature = "lume", target_os = "macos", target_arch = "aarch64"))]
    #[tokio::test]
    #[ignore = "downloads the pinned Lume release (network)"]
    async fn downloads_and_verifies_the_pinned_lume() {
        let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
        assert!(
            !home.is_empty() && !home.ends_with("/.cua"),
            "never the real cua home"
        );
        let bin = ensure().await.unwrap();
        assert_eq!(installed(), Some(bin.clone()));
        assert_eq!(resolve(LumeSource::Builtin), Some(bin.clone()));
        let out = std::process::Command::new(&bin)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains(VERSION));
        // Again: already there, nothing downloaded.
        assert_eq!(ensure().await.unwrap(), bin);
        remove().unwrap();
        assert!(installed().is_none());
    }
}
