//! The commands that ship with Cua Spaces: `cua teleport`, `cua keyvault`,
//! `cua volume config` and the Keyvault broker behind the MCP
//! `teleport_browser_session` tool.
//!
//! Their command line is defined in this crate (MIT); their implementation
//! (source-available, FSL-1.1-MIT) is in `cua-spaces-cli`, whose `cua`
//! registers a [`CliExtension`] and then runs [`crate::main`]. The Cua Spaces
//! apps ship that build. This build, alone, hands those commands to the Cua
//! Spaces `cua` when it finds one ([`spaces_cli`]): `CUA_SPACES_CLI`, the
//! Cua Spaces app bundle, or `cua-spaces-cli` on `PATH`. Otherwise they fail
//! with an error that says where they ship.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use cua_sdk::{Cua, CuaError, MediaEvent, MediaOpenOptions, MediaSession, SpacesdClient};

use crate::drive_cmd::ConfigCmd;
use crate::keyvault_cmd::KeyvaultCmd;
use crate::teleport::TeleportCmd;
use crate::teleport_session::Broker;

/// Set by the Cua Spaces `cua` on the commands it is handed, so a
/// misconfigured helper can never hand them back.
pub const ENV_FORWARDED: &str = "CUA_SPACES_CLI_FORWARDED";
/// An explicit path to the Cua Spaces `cua`.
pub const ENV_SPACES_CLI: &str = "CUA_SPACES_CLI";

/// What the Cua Spaces build of `cua` adds.
#[async_trait::async_trait(?Send)]
pub trait CliExtension: Send + Sync {
    /// `cua teleport <cmd>`. `cua` is open for `push` only.
    async fn teleport(
        &self,
        cua: Option<&Cua>,
        cmd: TeleportCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// `cua keyvault <cmd>`.
    async fn keyvault(
        &self,
        cmd: KeyvaultCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// `cua volume config <cmd>`.
    fn drive_config(
        &self,
        cmd: ConfigCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// The Keyvault broker the MCP `teleport_browser_session` tool files
    /// requests with.
    fn session_broker(&self) -> Option<Arc<dyn Broker>>;

    /// `cua viewer`: the standalone HTML5 viewer server.
    async fn viewer(
        &self,
        cua: Arc<Cua>,
        listen: &str,
        no_open: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError>;

    /// Opens a media session whose video `frames` receives decoded (packed
    /// BGRA) frames (`cua sb stream-probe`).
    async fn open_media_decoded(
        &self,
        env: Arc<SpacesdClient>,
        options: MediaOpenOptions,
        frames: Arc<dyn DecodedFrames>,
    ) -> Result<Arc<MediaSession>, CuaError>;
}

/// Receives decoded frames (see [`CliExtension::open_media_decoded`]).
pub trait DecodedFrames: Send + Sync {
    /// One frame: `width` x `height`, packed top-down BGRA.
    fn frame(&self, width: u32, height: u32, bgra: Vec<u8>);
    /// A control event.
    fn event(&self, event: MediaEvent);
}

static EXTENSION: OnceLock<Arc<dyn CliExtension>> = OnceLock::new();

/// Registers the Cua Spaces commands (once, before [`crate::main`]).
pub fn register(extension: Arc<dyn CliExtension>) {
    let _ = EXTENSION.set(extension);
}

/// The registered extension, if any.
pub fn get() -> Option<&'static Arc<dyn CliExtension>> {
    EXTENSION.get()
}

pub(crate) async fn teleport(
    cua: Option<&Cua>,
    cmd: TeleportCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.teleport(cua, cmd, json, out).await,
        None => forward("teleport"),
    }
}

pub(crate) async fn keyvault(
    cmd: KeyvaultCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.keyvault(cmd, json, out).await,
        None => forward("the Keyvault"),
    }
}

pub(crate) async fn viewer(
    cua: Arc<Cua>,
    listen: &str,
    no_open: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.viewer(cua, listen, no_open, out).await,
        None => forward("the standalone viewer (`cua viewer`)"),
    }
}

pub(crate) fn drive_config(
    cmd: ConfigCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    match get() {
        Some(e) => e.drive_config(cmd, json, out),
        None => forward("the Cua Volume's configuration"),
    }
}

/// Runs this process's command line in the Cua Spaces `cua` and returns its
/// exit code, or explains where `what` ships.
fn forward(what: &str) -> Result<i32, CuaError> {
    let missing = || {
        CuaError::Unsupported(format!(
            "{what} ships with Cua Spaces (source-available, FSL-1.1-MIT) and this `cua` \
             does not include it; install Cua Spaces (https://cua.ai/download) and run its \
             `cua`, or set {ENV_SPACES_CLI} to it"
        ))
    };
    if std::env::var_os(ENV_FORWARDED).is_some() {
        return Err(missing());
    }
    let Some(helper) = spaces_cli() else {
        return Err(missing());
    };
    let status = std::process::Command::new(&helper)
        .args(std::env::args_os().skip(1))
        .env(ENV_FORWARDED, "1")
        .status()
        .map_err(|e| CuaError::Internal(format!("{}: {e}", helper.display())))?;
    Ok(status.code().unwrap_or(1))
}

/// The Cua Spaces `cua`: `CUA_SPACES_CLI`, else the Cua Spaces app's bundled
/// `cua`, else `cua-spaces-cli` on `PATH`. Never this executable.
pub fn spaces_cli() -> Option<PathBuf> {
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let not_me = |p: PathBuf| -> Option<PathBuf> {
        let real = p.canonicalize().ok()?;
        (Some(&real) != me.as_ref() && real.is_file()).then_some(p)
    };
    if let Some(p) = std::env::var_os(ENV_SPACES_CLI).filter(|v| !v.is_empty()) {
        return not_me(PathBuf::from(p));
    }
    // Test processes only use an explicit `CUA_SPACES_CLI`, never an
    // installed app.
    if std::env::var_os("CUA_ENV_TEST_SANDBOX").is_some() {
        return None;
    }
    app_bundle_candidates()
        .into_iter()
        .find_map(not_me)
        .or_else(|| {
            on_path(if cfg!(windows) {
                "cua-spaces-cli.exe"
            } else {
                "cua-spaces-cli"
            })
            .and_then(not_me)
        })
}

/// What `cua --version` of `helper` prints as a version (`None` when it does
/// not run in five seconds or prints none).
fn helper_version(helper: &Path) -> Option<String> {
    let mut child = std::process::Command::new(helper)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    text.lines().next().map(|l| l.trim().to_string())
}

/// The Cua Spaces `cua` a `cua daemon start` may hand the daemon to: the
/// one [`spaces_cli`] finds, unless it is older than this `cua` (an app
/// not updated yet), whose daemon would be a downgrade of the CLI the user
/// upgraded. `Err` says why, so the caller can tell the user. A helper whose
/// version cannot be read is used, as before.
pub fn spaces_cli_not_older() -> Result<Option<PathBuf>, String> {
    let Some(helper) = spaces_cli() else {
        return Ok(None);
    };
    Ok(match helper_version(&helper) {
        Some(v) if cua_home::version::is_newer(cua_daemon::VERSION, &v) => {
            return Err(format!(
                "the Cua Spaces app's cua at {} is {v}, older than this cua {}; starting this cua's daemon \
                 instead. Update Cua Spaces to use its features",
                helper.display(),
                cua_daemon::VERSION
            ));
        }
        _ => Some(helper),
    })
}

fn app_bundle_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(PathBuf::from(
            "/Applications/Cua Spaces.app/Contents/MacOS/cua",
        ));
        if let Some(home) = std::env::var_os("HOME") {
            out.push(Path::new(&home).join("Applications/Cua Spaces.app/Contents/MacOS/cua"));
        }
    } else if cfg!(windows) {
        for var in ["LOCALAPPDATA", "ProgramFiles"] {
            if let Some(dir) = std::env::var_os(var) {
                out.push(Path::new(&dir).join("Cua Spaces").join("cua.exe"));
            }
        }
    }
    out
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// The extensions this process registered that the running daemon does not
/// report (`GetInfoResponse.features`, `extension:<name>`); empty when none
/// is missing or no daemon answers.
pub(crate) async fn missing_daemon_extensions() -> Vec<String> {
    let want: Vec<String> = cua_daemon::extension::registered()
        .iter()
        .map(|e| e.name().to_string())
        .collect();
    if want.is_empty() {
        return want;
    }
    let Ok(client) = cua_daemon::client::existing_daemon().await else {
        return Vec::new();
    };
    let Ok(info) = client.info().await else {
        return Vec::new();
    };
    want.into_iter()
        .filter(|name| {
            !info
                .features
                .iter()
                .any(|f| f.strip_prefix(cua_daemon::extension::FEATURE_PREFIX) == Some(name))
        })
        .collect()
}

/// The running daemon's version when it is strictly newer than this
/// `cua` (so it is never replaced by it), else `None`.
pub(crate) async fn newer_daemon_version() -> Option<String> {
    let info = cua_daemon::client::existing_daemon()
        .await
        .ok()?
        .info()
        .await
        .ok()?;
    cua_home::version::is_newer(&info.version, cua_daemon::VERSION).then_some(info.version)
}

/// How long [`stop_daemon`] waits for the daemon to go: one with Spaces
/// running unmounts their volumes and lets go of their VMs first, which
/// took 12 s on a Mac with one running macOS Space (10 s was too short:
/// the app updated to 0.7.0 was left without its daemon).
pub(crate) const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long [`stop_daemon`] waits after each signal to a daemon that did
/// not exit when asked.
#[cfg(unix)]
const TERMINATE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Asks the running daemon to stop and waits (bounded by [`STOP_WAIT`])
/// for it to go; a cua daemon still running then is ended (SIGTERM, then
/// SIGKILL).
pub(crate) async fn stop_daemon(discovery: &Path, pid: u32) -> Result<(), CuaError> {
    stop_daemon_within(discovery, pid, STOP_WAIT).await
}

async fn stop_daemon_within(
    discovery: &Path,
    pid: u32,
    wait: std::time::Duration,
) -> Result<(), CuaError> {
    if let Ok(client) = cua_daemon::client::existing_daemon().await {
        let _ = client.shutdown().await;
    }
    let tick = std::time::Duration::from_millis(100);
    for _ in 0..(wait.as_millis() / tick.as_millis()) {
        let gone = match cua_daemon::Discovery::read(discovery) {
            Some(d) => d.pid != pid,
            None => true,
        };
        // Removing discovery and closing listeners does not prove process exit.
        if gone && !cua_daemon::pid_alive(pid) && cua_daemon::client::live_address().is_none() {
            return Ok(());
        }
        tokio::time::sleep(tick).await;
    }
    // Asked and waited for, it still runs: after an app update such a
    // daemon (its listeners closed, its process idle) kept holding the
    // Keyvault socket (#4950). End it, so this build's daemon replaces it.
    #[cfg(unix)]
    if tokio::task::spawn_blocking(move || cua_daemon::terminate_daemon(pid, TERMINATE_WAIT))
        .await
        .unwrap_or(false)
    {
        eprintln!(
            "{}: the running {} daemon (pid {pid}) did not exit when asked; ended it",
            crate::util::cli(),
            crate::util::cli()
        );
        if cua_daemon::client::live_address().is_none() {
            return Ok(());
        }
    }
    Err(CuaError::Timeout(format!(
        "the running cua daemon (pid {pid}) did not stop within {} s",
        wait.as_secs()
    )))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::{Child, Command};
    use std::time::Duration;

    struct Process(Child);

    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[tokio::test]
    async fn missing_discovery_does_not_mean_the_old_process_exited() {
        let _env = crate::util::test_env::isolated();
        let home = tempfile::tempdir().unwrap();
        let discovery = home.path().join("daemon.json");
        let mut process = Process(Command::new("sleep").arg("60").spawn().unwrap());
        let pid = process.0.id();

        let result =
            tokio::time::timeout(Duration::from_millis(250), stop_daemon(&discovery, pid)).await;
        assert!(result.is_err(), "must wait for the old process to exit");
        assert!(process.0.try_wait().unwrap().is_none());

        process.0.kill().unwrap();
        process.0.wait().unwrap();
        stop_daemon(&discovery, pid).await.unwrap();
    }

    /// The daemon of #4950: asked to stop, it closed its listeners but its
    /// process never exited. It ignores SIGTERM here, so SIGKILL ends it.
    #[tokio::test]
    async fn a_daemon_that_never_exits_is_ended() {
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::util::test_env::isolated();
        let home = tempfile::tempdir().unwrap();
        let exe = home.path().join("cua");
        std::fs::write(&exe, "#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = Command::new(&exe)
            .args(["daemon", "start", "--foreground"])
            .spawn()
            .unwrap();
        let pid = child.id();
        // Reaped as soon as it dies, so a killed one is not left a zombie.
        let reaper = std::thread::spawn(move || child.wait().unwrap());
        tokio::time::sleep(Duration::from_millis(300)).await;

        let discovery = home.path().join("daemon.json");
        stop_daemon_within(&discovery, pid, Duration::from_millis(300))
            .await
            .expect("the stuck daemon is ended");
        assert!(!reaper.join().unwrap().success());
        assert!(!cua_daemon::pid_alive(pid));
    }

    /// UL2: a `cua daemon start` does not hand the daemon to an app's cua
    /// that is older than itself.
    #[cfg(unix)]
    #[test]
    fn a_helper_older_than_this_cua_is_not_handed_the_daemon() {
        use std::os::unix::fs::PermissionsExt;
        let _env = crate::util::test_env::isolated();
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helper-cua");
        let write = |version: &str| {
            let _ = std::fs::remove_file(&helper);
            std::fs::write(&helper, format!("#!/bin/sh\necho 'cua {version}'\n")).unwrap();
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // SAFETY: the test holds the environment lock.
        unsafe { std::env::set_var(ENV_SPACES_CLI, &helper) };
        write("0.0.1-old");
        let err = spaces_cli_not_older().unwrap_err();
        assert!(err.contains("older than this cua"), "{err}");
        write("99.0.0");
        assert_eq!(spaces_cli_not_older().unwrap(), Some(helper.clone()));
        write(cua_daemon::VERSION);
        assert_eq!(spaces_cli_not_older().unwrap(), Some(helper.clone()));
        // SAFETY: as above.
        unsafe { std::env::remove_var(ENV_SPACES_CLI) };
    }
}
