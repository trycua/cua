//! Running cua-spacesd as a per-OS service.
//!
//! The file generators ([`systemd_unit`], [`launchd_plist`],
//! [`windows_task_xml`]) are pure functions; the [`ServiceManager`]s shell
//! out to `systemctl`, `launchctl` and `schtasks`, or (the fallback for
//! containers without an init system) spawn a detached process with a
//! pidfile. Tests use [`FakeServiceManager`].
//!
//! - Linux: a systemd unit. System-wide (`/etc/systemd/system`) when run as
//!   root, otherwise a user unit (`~/.config/systemd/user`) so the driver
//!   lives in the user's graphical session.
//! - macOS: a LaunchAgent in the GUI (Aqua) session, so screen capture and
//!   accessibility run with the user's TCC grants. The user grants Screen
//!   Recording and Accessibility in System Settings; nothing here grants or
//!   opens anything. With the Cua Spaces app installed, the agent is the
//!   app's own login item ([`AppHelper`]): the app's executable in its
//!   service mode, registered with `SMAppService`, so macOS shows and keeps
//!   every permission under one name, "Cua Spaces", and the Team ID;
//!   without the app, a hand-written LaunchAgent ([`Launchd`]) runs the
//!   downloaded binary.
//! - Windows: a scheduled task that starts at logon with the user's
//!   interactive token, which is the interactive-session helper: a Windows
//!   service lives in session 0 and cannot capture the desktop or inject
//!   input, so the task (not a service) runs the driver in the user session.

use crate::{Error, Result};
use cua_home::channel::CuaChannel;
use cua_home::env::CuaEnv;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Base systemd unit name of the host service (the production home's).
pub const SERVICE_NAME: &str = "cua-spacesd-host";
/// Base Windows task name (the production home's).
pub const WINDOWS_TASK_NAME: &str = "Cua\\SpacesdHost";
/// Base HKCU `Run` value of the Windows logon entry (the production home's).
pub const WINDOWS_RUN_VALUE: &str = "CuaSpacesdHost";

/// The names one cua home's host service is registered under, on every OS
/// (CUA-3512).
///
/// Every cua home runs its own host service, so no two homes may share a
/// unit, task, logon entry or launchd label: a staging setup on a computer
/// that also runs the production host must never end, replace or delete
/// production's. The names come from the resolved cua home and the
/// channel, never from environment variables read later, so the CLI, the
/// app's daemon and the uninstallers agree.
///
/// | Home                                   | Suffix       | systemd unit                    | Windows task              |
/// |----------------------------------------|--------------|---------------------------------|---------------------------|
/// | `~/.cua` (Stable, production)          | none         | `cua-spacesd-host`              | `Cua\SpacesdHost`         |
/// | `~/.cua/envs/staging` (Stable, staging)| `-staging`   | `cua-spacesd-host-staging`      | `Cua\SpacesdHost-staging` |
/// | `~/.cua-local` (Local, staging)        | none         | `cua-spacesd-host-local`        | `Cua\SpacesdHost-local`   |
/// | `~/.cua-local/envs/prod` (Local, prod) | `-prod`      | `cua-spacesd-host-local-prod`   | `Cua\SpacesdHost-local-prod` |
/// | any other `CUA_HOME`                   | `-<8 hex>`   | `cua-spacesd-host[-local]-<hex>`| `Cua\SpacesdHost[-local]-<hex>` |
///
/// The Windows logon entry is the task name without `Cua\` and the dash
/// (`CuaSpacesdHost-staging`); the launchd label is the channel's
/// [`CuaChannel::host_label`] plus the suffix. The production home keeps
/// the names it has always had.
///
/// Builds before CUA-3512 named a staging home's service after `CUA_HOME`
/// alone: without it, the production names; with it, a hash of the path.
/// [`ServiceNames::legacy`] lists those, and the runners retire one only
/// when it runs this home's host ([`ServiceNames::owns`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceNames {
    /// The channel the names are for.
    pub channel: CuaChannel,
    /// `""`, `-staging`, `-prod` or `-<8 hex>` (see the table).
    pub suffix: String,
    /// This home's host folder (`<home>/host`): what a runner of this home
    /// mentions ([`ServiceNames::owns`]).
    pub host_dir: PathBuf,
    /// The user's home the cua homes are below.
    pub user_home: PathBuf,
}

/// Names of one runner kind: this home's, then the names older builds may
/// have used for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyNames {
    /// systemd units (without `.service`).
    pub systemd: Vec<String>,
    /// launchd labels.
    pub launchd: Vec<String>,
    /// Windows task names.
    pub windows_task: Vec<String>,
    /// Windows `Run` values.
    pub windows_run: Vec<String>,
}

impl ServiceNames {
    /// The names of the host service of `cua_home` (the resolved home, as
    /// [`cua_home::cua_home`] gives it), for `channel`, below `user_home`.
    pub fn for_home(channel: CuaChannel, cua_home: &Path, user_home: &Path) -> Self {
        Self {
            channel,
            suffix: home_suffix(channel, cua_home, user_home),
            host_dir: cua_home.join("host"),
            user_home: user_home.to_path_buf(),
        }
    }

    /// The current process's: [`cua_home::cua_home`] for
    /// [`CuaChannel::current`].
    pub fn current() -> Self {
        let user = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        Self::for_home(CuaChannel::current(), &cua_home::cua_home(), &user)
    }

    fn channel_tag(&self) -> &'static str {
        match self.channel {
            CuaChannel::Stable => "",
            CuaChannel::Local => "-local",
        }
    }

    /// The systemd unit name (without `.service`).
    pub fn systemd_unit(&self) -> String {
        format!("{SERVICE_NAME}{}{}", self.channel_tag(), self.suffix)
    }

    /// The launchd label.
    pub fn launchd_label(&self) -> String {
        format!("{}{}", self.channel.host_label(), self.suffix)
    }

    /// The Windows scheduled task name (`Cua\SpacesdHost…`).
    pub fn windows_task(&self) -> String {
        format!("{WINDOWS_TASK_NAME}{}{}", self.channel_tag(), self.suffix)
    }

    /// The HKCU `Run` value of the Windows logon entry.
    pub fn windows_run_value(&self) -> String {
        format!("{WINDOWS_RUN_VALUE}{}{}", self.channel_tag(), self.suffix)
    }

    /// Whether these are the production names of the channel's own home
    /// (no suffix): the names every build before CUA-3512 also used for
    /// other homes.
    pub fn is_default(&self) -> bool {
        self.suffix.is_empty()
    }

    /// Whether this is the channel's home for the environment that is not
    /// its own (Stable's staging home, Local's production home), which
    /// runs beside the channel's own host on the same computer.
    pub fn other_env(&self) -> bool {
        [CuaEnv::Prod, CuaEnv::Staging]
            .iter()
            .any(|e| self.suffix == format!("-{}", e.name()))
    }

    /// The names builds before CUA-3512 may have registered this home's
    /// service under, other than its own: the unsuffixed names (a staging
    /// home set up without `CUA_HOME`) and the `CUA_HOME` hash (one set up
    /// with it). Retire one only when [`ServiceNames::owns`] its content.
    pub fn legacy(&self) -> LegacyNames {
        let home = self.host_dir_home();
        let hash = format!("-{}", home_hash(&home));
        // The old rule hashed every home but `~/.cua` for systemd, and every
        // home but the channel's own for launchd.
        let systemd_hash = (home != self.user_home.join(CuaChannel::Stable.home_dir_name()))
            .then(|| format!("{SERVICE_NAME}{hash}"));
        let launchd_hash = (home != self.user_home.join(self.channel.home_dir_name()))
            .then(|| format!("{}{hash}", self.channel.host_label()));
        let others = |own: String, candidates: Vec<String>| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for c in candidates {
                if c != own && !out.contains(&c) {
                    out.push(c);
                }
            }
            out
        };
        LegacyNames {
            systemd: others(
                self.systemd_unit(),
                std::iter::once(SERVICE_NAME.to_string())
                    .chain(systemd_hash)
                    .collect(),
            ),
            launchd: others(
                self.launchd_label(),
                std::iter::once(self.channel.host_label().to_string())
                    .chain(launchd_hash)
                    .collect(),
            ),
            windows_task: others(self.windows_task(), vec![WINDOWS_TASK_NAME.into()]),
            windows_run: others(self.windows_run_value(), vec![WINDOWS_RUN_VALUE.into()]),
        }
    }

    fn host_dir_home(&self) -> PathBuf {
        self.host_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// Whether a unit, plist, task XML or logon command line (`content`)
    /// runs this home's host: it names a file in this home's host folder
    /// (the driver, its log). Another home's host folder never matches,
    /// even one inside this home (`~/.cua/host/` is not part of
    /// `~/.cua/envs/staging/host/`).
    pub fn owns(&self, content: &str) -> bool {
        // On Windows `/` and `\` are one separator (`envs/staging` joins
        // into a path with both), and case does not matter.
        let norm = |s: &str| {
            if cfg!(windows) {
                s.replace('/', "\\").to_lowercase()
            } else {
                s.to_string()
            }
        };
        let content = norm(content);
        let dir = self.host_dir.to_string_lossy();
        let sep = std::path::MAIN_SEPARATOR;
        [
            format!("{dir}{sep}"),
            format!("{dir}/"),
            xml_escape(&format!("{dir}{sep}")),
        ]
        .iter()
        .any(|needle| content.contains(norm(needle).as_str()))
    }
}

/// `channel`'s cua home for `env` below `user_home` (`~/.cua`,
/// `~/.cua/envs/staging`, `~/.cua-local`, `~/.cua-local/envs/prod`).
fn env_home_in(channel: CuaChannel, env: CuaEnv, user_home: &Path) -> PathBuf {
    let base = user_home.join(channel.home_dir_name());
    match channel.home_subdir(env) {
        Some(sub) => base.join(sub),
        None => base,
    }
}

/// The suffix of `cua_home`'s names: none for the channel's own home in its
/// own environment, the environment's name for the channel's home of the
/// other one, else a hash of the path.
fn home_suffix(channel: CuaChannel, cua_home: &Path, user_home: &Path) -> String {
    let own_env = channel.default_env();
    if cua_home == env_home_in(channel, own_env, user_home) {
        return String::new();
    }
    for env in [CuaEnv::Prod, CuaEnv::Staging] {
        if env != own_env && cua_home == env_home_in(channel, env, user_home) {
            return format!("-{}", env.name());
        }
    }
    format!("-{}", home_hash(cua_home))
}

/// 8 hex digits of SHA-256 of the path: what builds before CUA-3512 added
/// for a `CUA_HOME` other than the channel's own home.
fn home_hash(home: &Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(home.to_string_lossy().as_bytes());
    hex::encode(digest)[..8].to_string()
}

/// What macOS shows for the app's login item (System Settings: Privacy &
/// Security, Login Items): the app's own name, since the app's executable
/// runs it. "Cua Spaces", or "Cua Spaces Local" for the Local build.
pub fn app_helper_name() -> &'static str {
    CuaChannel::current().app_name()
}
/// The cua-spacesd the app ships, inside the app (the service runs only
/// this one). In `Contents/Helpers`, not `Contents/MacOS`: from there, AppKit
/// (its cursor overlay) registered it with LaunchServices as the app itself,
/// and opening the app activated the windowless daemon instead of launching
/// it.
pub const APP_HELPER_DAEMON: &str = "Contents/Helpers/cua-spacesd";
/// Where apps before 0.15 kept it (they run it from there).
pub const APP_HELPER_DAEMON_BEFORE_0_15: &str = "Contents/MacOS/cua-spacesd";
/// The `SMAppService.agent` plist of `channel`'s app, inside the app:
/// `Contents/Library/LaunchAgents/<helper label>.plist` (the label is
/// [`CuaChannel::helper_label`]; `build-app.sh` stamps it per channel).
pub fn app_helper_plist(channel: CuaChannel) -> String {
    format!(
        "Contents/Library/LaunchAgents/{}.plist",
        channel.helper_label()
    )
}
/// The app's own executable (which registers the helper), inside the app.
pub const APP_EXECUTABLE: &str = "Contents/MacOS/CuaSpacesMac";

/// The names installs made before the cua-spacesd rename used. `install` and
/// `uninstall` remove a service left under them (best effort), so an upgrade
/// never runs two copies.
pub const LEGACY_SERVICE_NAME: &str = "cua-env-driver-host";
/// Pre-rename launchd label.
pub const LEGACY_LAUNCHD_LABEL: &str = "com.trycua.env-driver.host";
/// Pre-rename Windows task name.
pub const LEGACY_WINDOWS_TASK_NAME: &str = "Cua\\EnvDriverHost";

/// What to run.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceSpec {
    /// Absolute path of the cua-spacesd binary.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// Environment variables.
    pub env: Vec<(String, String)>,
    /// Log file (stdout + stderr).
    pub log_path: PathBuf,
    /// Working directory.
    pub working_dir: PathBuf,
}

/// Which runner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerKind {
    /// Pick for this OS ([`RunnerKind::detect`]).
    #[default]
    Auto,
    /// systemd (system unit as root, user unit otherwise).
    Systemd,
    /// launchd LaunchAgent in the GUI session.
    Launchd,
    /// macOS: the Cua Spaces app's login item (the app's own executable)
    /// ([`AppHelper`]).
    AppHelper,
    /// Windows scheduled task at logon (interactive token).
    WindowsTask,
    /// A detached process with a pidfile (no init system, e.g. containers).
    Process,
}

impl RunnerKind {
    /// The runner for this machine: on macOS the Cua Spaces app's helper
    /// when the app (with one) is installed, else launchd; the scheduled
    /// task on Windows, systemd on Linux when it is PID 1, else a plain
    /// process.
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            if find_app_helper().is_some() {
                RunnerKind::AppHelper
            } else {
                RunnerKind::Launchd
            }
        } else if cfg!(windows) {
            RunnerKind::WindowsTask
        } else {
            linux_runner(Path::new("/run/systemd/system"))
        }
    }

    /// Resolves `Auto`.
    pub fn resolve(self) -> Self {
        if self == RunnerKind::Auto {
            Self::detect()
        } else {
            self
        }
    }

    /// Wire name (`systemd`, `launchd`, `windows-task`, `process`).
    pub fn as_str(self) -> &'static str {
        match self {
            RunnerKind::Auto => "auto",
            RunnerKind::Systemd => "systemd",
            RunnerKind::Launchd => "launchd",
            RunnerKind::AppHelper => "app-helper",
            RunnerKind::WindowsTask => "windows-task",
            RunnerKind::Process => "process",
        }
    }

    /// Parses a wire name.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "auto" | "" => RunnerKind::Auto,
            "systemd" => RunnerKind::Systemd,
            "launchd" => RunnerKind::Launchd,
            "app-helper" => RunnerKind::AppHelper,
            "windows-task" | "schtasks" => RunnerKind::WindowsTask,
            "process" => RunnerKind::Process,
            other => return Err(Error::InvalidArgument(format!("unknown runner {other:?}"))),
        })
    }
}

/// Service state as reported by the runner.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceState {
    /// The unit / agent / task / pidfile exists.
    pub installed: bool,
    /// The driver is running.
    pub running: bool,
    /// `systemd`, `launchd`, `app-helper`, `windows-task` or `process`.
    pub kind: String,
    /// Human detail (unit path, pid, runner output).
    pub detail: String,
}

/// Installs and controls the host service.
pub trait ServiceManager: Send + Sync {
    /// The runner kind.
    fn kind(&self) -> RunnerKind;
    /// Writes the unit / agent / task and enables it (does not start).
    fn install(&self, spec: &ServiceSpec) -> Result<()>;
    /// Starts (or restarts) it.
    fn start(&self) -> Result<()>;
    /// Stops it.
    fn stop(&self) -> Result<()>;
    /// Stops, disables and deletes it. Idempotent.
    fn uninstall(&self) -> Result<()>;
    /// Current state.
    fn state(&self) -> ServiceState;
    /// Whether the service this runs could bind `addr` for `transport`
    /// now (setup asks before it chooses the service's ports). The real
    /// runners ask the operating system ([`crate::ports::os_port_free`]).
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool;
    /// Ends and deletes this home's service where a build before CUA-3512
    /// registered it under another home's names (see [`ServiceNames`]),
    /// never touching a runner of another home. Returns the names it
    /// retired. Setup calls it before it installs; `uninstall` calls it too.
    fn retire_legacy_names(&self) -> Vec<String> {
        Vec::new()
    }
}

// ------------------------------------------------------------- generators

fn systemd_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c))
    {
        arg.to_string()
    } else {
        format!(
            "\"{}\"",
            arg.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
        )
    }
}

/// The systemd unit. `system` = a system unit (root), else a user unit.
pub fn systemd_unit(spec: &ServiceSpec, system: bool) -> String {
    let exec = std::iter::once(spec.program.to_string_lossy().into_owned())
        .chain(spec.args.iter().cloned())
        .map(|a| systemd_quote(&a))
        .collect::<Vec<_>>()
        .join(" ");
    let mut env = String::new();
    for (k, v) in &spec.env {
        env.push_str(&format!(
            "Environment={}\n",
            systemd_quote(&format!("{k}={v}"))
        ));
    }
    let (after, wanted) = if system {
        ("network-online.target", "multi-user.target")
    } else {
        (
            "network-online.target graphical-session.target",
            "default.target",
        )
    };
    let oom = if system {
        // Only a system unit may lower its OOM score.
        "OOMScoreAdjust=-1000\nMemoryMin=64M\n"
    } else {
        ""
    };
    format!(
        "# Generated by cua-host (`cua host setup` / Cua Spaces \"Set up for access\").\n\
         # Unattended access to this computer through cua-spacesd.\n\
         [Unit]\n\
         Description=Cua unattended access (cua-spacesd)\n\
         Documentation=https://github.com/trycua/cua/tree/main/libs/cua-spacesd\n\
         After={after}\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec}\n\
         WorkingDirectory={wd}\n\
         {env}\
         Restart=always\n\
         RestartSec=2\n\
         OOMPolicy=continue\n\
         {oom}\
         KillMode=mixed\n\
         TimeoutStopSec=10\n\
         StandardOutput=append:{log}\n\
         StandardError=append:{log}\n\
         \n\
         [Install]\n\
         WantedBy={wanted}\n",
        wd = systemd_quote(&spec.working_dir.to_string_lossy()),
        log = spec.log_path.to_string_lossy(),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The LaunchAgent plist (GUI session, kept alive).
pub fn launchd_plist(spec: &ServiceSpec, label: &str) -> String {
    let mut args = format!(
        "    <string>{}</string>\n",
        xml_escape(&spec.program.to_string_lossy())
    );
    for a in &spec.args {
        args.push_str(&format!("    <string>{}</string>\n", xml_escape(a)));
    }
    let mut env = String::new();
    for (k, v) in &spec.env {
        env.push_str(&format!(
            "    <key>{}</key>\n    <string>{}</string>\n",
            xml_escape(k),
            xml_escape(v)
        ));
    }
    let log = xml_escape(&spec.log_path.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Generated by cua-host: unattended access through cua-spacesd, in the
     logged-in GUI session so capture and input run with the user's own
     Screen Recording / Accessibility grants. -->
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>EnvironmentVariables</key>
  <dict>
{env}  </dict>
  <key>WorkingDirectory</key>
  <string>{wd}</string>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Interactive</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#,
        label = xml_escape(label),
        wd = xml_escape(&spec.working_dir.to_string_lossy()),
    )
}

fn windows_quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// The `cmd.exe` arguments that run the driver and append its output to
/// the log: `/d /c [set "K=V"&& ...] driver args >> log 2>&1`.
pub fn windows_cmd_line(spec: &ServiceSpec) -> String {
    let driver = std::iter::once(windows_quote(&spec.program.to_string_lossy()))
        .chain(spec.args.iter().map(|a| windows_quote(a)))
        .collect::<Vec<_>>()
        .join(" ");
    let log = windows_quote(&spec.log_path.to_string_lossy());
    let mut chain = String::new();
    for (k, v) in &spec.env {
        chain.push_str(&format!("set \"{k}={v}\"&& "));
    }
    format!("/d /c {chain}{driver} >> {log} 2>&1")
}

/// The program and arguments that run the driver in the user's session at
/// logon: `conhost.exe --headless cmd.exe` [`windows_cmd_line`].
/// `--headless` keeps the console out of sight; a visible one was an empty
/// window that stopped the host when closed. `system_root` is
/// `%SystemRoot%` in the task (which expands it) and the real directory in
/// the logon entry. Starting it now is [`windows_start_now`].
pub fn windows_action(spec: &ServiceSpec, system_root: &str) -> (String, String) {
    (
        format!("{system_root}\\System32\\conhost.exe"),
        format!(
            "--headless {} {}",
            windows_quote(&format!("{system_root}\\System32\\cmd.exe")),
            windows_cmd_line(spec)
        ),
    )
}

/// The Task Scheduler XML: at logon of `user` (DOMAIN\\name, or empty for
/// any user), interactive token, restart on failure, no time limit.
/// Environment variables are passed as `--env`-free arguments: the task
/// runs `cmd.exe /d /c set K=V&& … && driver args` only when `env` is
/// non-empty, since tasks have no environment block.
pub fn windows_task_xml(spec: &ServiceSpec, user: &str, task_name: &str) -> String {
    let (command, arguments) = windows_action(spec, "%SystemRoot%");
    let user_id = if user.is_empty() {
        String::new()
    } else {
        format!("      <UserId>{}</UserId>\n", xml_escape(user))
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<!-- Generated by cua-host: unattended access through cua-spacesd in the
     user's interactive session (a Windows service runs in session 0 and
     cannot see the desktop, so this logon task is the interactive helper). -->
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Cua unattended access (cua-spacesd)</Description>
    <URI>\{name}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
{user_id}    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
{user_id}      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>999</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{wd}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
        name = xml_escape(task_name),
        command = xml_escape(&command),
        arguments = xml_escape(&arguments),
        wd = xml_escape(&spec.working_dir.to_string_lossy()),
    )
}

// ---------------------------------------------------------------- runners

/// Windows: start console helpers (schtasks, reg) without a console window;
/// the Spaces app calls them from a GUI process.
fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn run(cmd: &mut Command) -> Result<String> {
    no_window(cmd);
    let shown = format!("{cmd:?}");
    let out = cmd
        .output()
        .map_err(|e| Error::Service(format!("{shown}: {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(Error::Service(format!("{shown} failed: {}", text.trim())))
    }
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn is_root() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// systemd.
pub struct Systemd {
    /// System unit (root) vs user unit.
    pub system: bool,
    /// Unit directory.
    pub unit_dir: PathBuf,
    /// The unit names of this home ([`ServiceNames`]).
    pub names: ServiceNames,
    /// The `systemctl` to run (tests pass a stand-in).
    pub systemctl: PathBuf,
}

impl Systemd {
    /// A system unit when running as root, else a user unit under `home`.
    pub fn for_current_user(user_home: &Path, names: ServiceNames) -> Self {
        let (system, unit_dir) = if is_root() {
            (true, PathBuf::from("/etc/systemd/system"))
        } else {
            (false, user_home.join(".config/systemd/user"))
        };
        Self {
            system,
            unit_dir,
            names,
            systemctl: PathBuf::from("systemctl"),
        }
    }

    fn unit(&self) -> String {
        self.names.systemd_unit()
    }

    fn unit_path(&self) -> PathBuf {
        self.unit_dir.join(format!("{}.service", self.unit()))
    }

    fn remove_legacy(&self) {
        let legacy = self.unit_dir.join(format!("{LEGACY_SERVICE_NAME}.service"));
        if legacy.exists() {
            let _ = run(self
                .systemctl()
                .args(["disable", "--now", LEGACY_SERVICE_NAME]));
            let _ = std::fs::remove_file(legacy);
        }
    }

    fn systemctl(&self) -> Command {
        let mut c = Command::new(&self.systemctl);
        if !self.system {
            c.arg("--user");
        }
        c
    }
}

impl ServiceManager for Systemd {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Systemd
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.remove_legacy();
        write_file(&self.unit_path(), &systemd_unit(spec, self.system))?;
        run(self.systemctl().arg("daemon-reload"))?;
        run(self.systemctl().args(["enable", &self.unit()]))?;
        Ok(())
    }
    fn start(&self) -> Result<()> {
        run(self.systemctl().args(["restart", &self.unit()])).map(drop)
    }
    fn stop(&self) -> Result<()> {
        run(self.systemctl().args(["stop", &self.unit()])).map(drop)
    }
    fn uninstall(&self) -> Result<()> {
        let _ = run(self.systemctl().args(["disable", "--now", &self.unit()]));
        let _ = std::fs::remove_file(self.unit_path());
        self.remove_legacy();
        self.retire_legacy_names();
        let _ = run(self.systemctl().arg("daemon-reload"));
        Ok(())
    }
    fn retire_legacy_names(&self) -> Vec<String> {
        let mut retired = Vec::new();
        for name in self.names.legacy().systemd {
            let path = self.unit_dir.join(format!("{name}.service"));
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !self.names.owns(&content) {
                // Another home's service (production's, most often):
                // never touched.
                continue;
            }
            tracing::info!(unit = %name, "retiring this home's host service from its old unit name");
            let _ = run(self.systemctl().args(["disable", "--now", &name]));
            let _ = std::fs::remove_file(&path);
            retired.push(name);
        }
        if !retired.is_empty() {
            let _ = run(self.systemctl().arg("daemon-reload"));
        }
        retired
    }
    fn state(&self) -> ServiceState {
        let installed = self.unit_path().exists();
        let active = self
            .systemctl()
            .args(["is-active", &self.unit()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        ServiceState {
            installed,
            running: active == "active",
            kind: "systemd".into(),
            detail: format!(
                "{} ({})",
                self.unit_path().display(),
                if active.is_empty() {
                    "unknown"
                } else {
                    &active
                }
            ),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool {
        crate::ports::os_port_free(addr, transport)
    }
}

/// launchd LaunchAgent (GUI domain of the current user).
pub struct Launchd {
    /// `~/Library/LaunchAgents`.
    pub agents_dir: PathBuf,
    /// The user id (`gui/<uid>`).
    pub uid: u32,
    /// The label of this home ([`ServiceNames`]).
    pub names: ServiceNames,
}

impl Launchd {
    /// The current user's agents directory and uid.
    pub fn for_current_user(user_home: &Path, names: ServiceNames) -> Self {
        #[cfg(unix)]
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 0;
        Self {
            agents_dir: user_home.join("Library/LaunchAgents"),
            uid,
            names,
        }
    }

    fn plist_path(&self) -> PathBuf {
        self.agents_dir
            .join(format!("{}.plist", self.names.launchd_label()))
    }

    fn target(&self) -> String {
        format!("gui/{}/{}", self.uid, self.names.launchd_label())
    }

    fn remove_legacy(&self) {
        let legacy = self
            .agents_dir
            .join(format!("{LEGACY_LAUNCHD_LABEL}.plist"));
        if legacy.exists() {
            let target = format!("gui/{}/{LEGACY_LAUNCHD_LABEL}", self.uid);
            let _ = run(Command::new("launchctl").args(["bootout", &target]));
            let _ = std::fs::remove_file(legacy);
        }
    }
}

impl ServiceManager for Launchd {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Launchd
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.remove_legacy();
        write_file(
            &self.plist_path(),
            &launchd_plist(spec, &self.names.launchd_label()),
        )
    }
    fn start(&self) -> Result<()> {
        let plist = self.plist_path().to_string_lossy().into_owned();
        launchd_restart(
            &format!("gui/{}", self.uid),
            &self.target(),
            &plist,
            &mut |args: &[&str]| run(Command::new("launchctl").args(args)),
            &mut |d| std::thread::sleep(d),
        )
    }
    fn stop(&self) -> Result<()> {
        let target = self.target();
        if !launchd_loaded(&target, &mut |args: &[&str]| {
            run(Command::new("launchctl").args(args))
        }) {
            // Already stopped: nothing to do (not an error).
            return Ok(());
        }
        run(Command::new("launchctl").args(["bootout", &target])).map(drop)
    }
    fn uninstall(&self) -> Result<()> {
        let _ = run(Command::new("launchctl").args(["bootout", &self.target()]));
        let _ = std::fs::remove_file(self.plist_path());
        self.remove_legacy();
        self.retire_legacy_names();
        Ok(())
    }
    fn retire_legacy_names(&self) -> Vec<String> {
        let mut retired = Vec::new();
        for label in self.names.legacy().launchd {
            let path = self.agents_dir.join(format!("{label}.plist"));
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !self.names.owns(&content) {
                continue;
            }
            let target = format!("gui/{}/{label}", self.uid);
            let _ = run(Command::new("launchctl").args(["bootout", &target]));
            let _ = std::fs::remove_file(&path);
            retired.push(label);
        }
        retired
    }
    fn state(&self) -> ServiceState {
        let printed = Command::new("launchctl")
            .args(["print", &self.target()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        ServiceState {
            installed: self.plist_path().exists(),
            running: printed.contains("state = running"),
            kind: "launchd".into(),
            detail: self.plist_path().display().to_string(),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool {
        crate::ports::os_port_free(addr, transport)
    }
}

/// How long [`launchd_restart`] waits for a bootout to finish.
const LAUNCHD_BOOTOUT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// Bootstrap attempts before [`launchd_restart`] gives up.
const LAUNCHD_BOOTSTRAP_ATTEMPTS: u32 = 3;

/// `launchctl print <target>` succeeds: the service is loaded.
fn launchd_loaded(target: &str, launchctl: &mut dyn FnMut(&[&str]) -> Result<String>) -> bool {
    launchctl(&["print", target]).is_ok()
}

/// (Re)starts a LaunchAgent so that running it twice, or while it is
/// loaded, never fails:
///
/// 1. `enable` clears a "disabled" override (left by turning the item off
///    in Login Items, or by an old `launchctl disable`), which otherwise
///    makes `bootstrap` fail with `5: Input/output error`.
/// 2. A loaded service is booted out, and this waits until launchd has
///    really let go of it: `bootout` returns before the teardown ends, and
///    a `bootstrap` in that window fails with `5: Input/output error` (or
///    `37: Operation already in progress`).
/// 3. `bootstrap`; when it fails but the service is loaded after all (a
///    race with launchd's own `RunAtLoad`/`KeepAlive`), `kickstart -k`
///    restarts it in place instead. Otherwise retried with a short backoff.
///
/// The error left after every attempt names the service in plain words,
/// with launchd's own message at the end.
pub(crate) fn launchd_restart(
    domain: &str,
    target: &str,
    plist: &str,
    launchctl: &mut dyn FnMut(&[&str]) -> Result<String>,
    sleep: &mut dyn FnMut(std::time::Duration),
) -> Result<()> {
    let step = std::time::Duration::from_millis(100);
    let _ = launchctl(&["enable", target]);
    if launchd_loaded(target, launchctl) {
        let _ = launchctl(&["bootout", target]);
        let mut waited = std::time::Duration::ZERO;
        while waited < LAUNCHD_BOOTOUT_WAIT && launchd_loaded(target, launchctl) {
            sleep(step);
            waited += step;
        }
    }
    let mut last = String::new();
    for attempt in 0..LAUNCHD_BOOTSTRAP_ATTEMPTS {
        if attempt > 0 {
            sleep(std::time::Duration::from_millis(500 * u64::from(attempt)));
        }
        match launchctl(&["bootstrap", domain, plist]) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        if launchd_loaded(target, launchctl) {
            // Loaded after all: restart it in place.
            match launchctl(&["kickstart", "-k", target]) {
                Ok(_) => return Ok(()),
                Err(e) => last = e.to_string(),
            }
        }
    }
    let why = last
        .split_once(" failed: ")
        .map_or(last.as_str(), |(_, why)| why)
        .trim()
        .trim_start_matches("service: ")
        .to_string();
    Err(Error::Service(format!(
        "macOS did not start the Cua host service (launchd: {why}); \
         try again, and if it keeps failing turn on cua-spacesd in System Settings > \
         General > Login Items & Extensions, or restart this Mac"
    )))
}

/// Ends and deletes the pre-rename task, if any (best effort).
fn remove_legacy_task() {
    let query = Command::new("schtasks")
        .args(["/Query", "/TN", LEGACY_WINDOWS_TASK_NAME])
        .output();
    if query.is_ok_and(|o| o.status.success()) {
        let _ = run(Command::new("schtasks").args(["/End", "/TN", LEGACY_WINDOWS_TASK_NAME]));
        let _ =
            run(Command::new("schtasks").args(["/Delete", "/TN", LEGACY_WINDOWS_TASK_NAME, "/F"]));
    }
}

const WINDOWS_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// The logon-entry command line: the task's action with real paths.
pub fn windows_run_value(spec: &ServiceSpec, system_root: &str) -> String {
    let (command, arguments) = windows_action(spec, system_root);
    format!("{} {arguments}", windows_quote(&command))
}

/// Windows: a scheduled task at logon, or, where Task Scheduler refuses this
/// user (CUA-1702: `schtasks /Create` said "Access is denied" from a normal,
/// non-elevated desktop session), a per-user logon entry (HKCU `Run`) plus a
/// hidden process started now. Both run the driver in the user's own
/// session with no administrator rights; the task adds restart on failure.
///
/// The task and the logon entry are named after this cua home
/// ([`ServiceNames`]), so a staging host never ends, replaces or deletes
/// the production one on the same PC (CUA-3512).
pub struct WindowsTask {
    /// Where the task XML is written before `schtasks /Create /XML`.
    pub xml_path: PathBuf,
    /// `DOMAIN\\user` the logon trigger fires for ("" = any user).
    pub user: String,
    /// The task and logon-entry names of this home.
    pub names: ServiceNames,
}

impl WindowsTask {
    /// The current user (`USERDOMAIN\\USERNAME`).
    pub fn for_current_user(host_dir: &Path, names: ServiceNames) -> Self {
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(d), Ok(u)) if !d.is_empty() && !u.is_empty() => format!("{d}\\{u}"),
            (_, Ok(u)) => u,
            _ => String::new(),
        };
        Self {
            xml_path: host_dir.join("task.xml"),
            user,
            names,
        }
    }

    fn task(&self) -> String {
        self.names.windows_task()
    }

    fn run_value(&self) -> String {
        self.names.windows_run_value()
    }

    /// The spec the logon entry runs (`service.json` next to the XML).
    fn spec_path(&self) -> PathBuf {
        self.xml_path.with_file_name("service.json")
    }

    fn run_entry_installed(&self) -> bool {
        run_entry(&self.run_value()).is_some()
    }

    fn remove_run_entry(&self) {
        if self.run_entry_installed() {
            let _ = run(Command::new("reg").args([
                "delete",
                WINDOWS_RUN_KEY,
                "/v",
                &self.run_value(),
                "/f",
            ]));
        }
        let _ = std::fs::remove_file(self.spec_path());
    }

    fn install_run_entry(&self, spec: &ServiceSpec) -> Result<()> {
        std::fs::write(
            self.spec_path(),
            serde_json::to_vec_pretty(spec).map_err(|e| Error::Internal(e.to_string()))?,
        )?;
        run(Command::new("reg").args([
            "add",
            WINDOWS_RUN_KEY,
            "/v",
            &self.run_value(),
            "/t",
            "REG_SZ",
            "/d",
            &windows_run_value(spec, &system_root()),
            "/f",
        ]))
        .map(drop)
    }

    fn spec(&self) -> Result<ServiceSpec> {
        serde_json::from_slice(&std::fs::read(self.spec_path())?)
            .map_err(|e| Error::Service(format!("service.json: {e}")))
    }

    /// This home's driver, as the runners start it (`<host>/bin/…`).
    fn home_driver(&self) -> PathBuf {
        self.names
            .host_dir
            .join("bin")
            .join(crate::driver::binary_name())
    }

    /// Whether a task by this home's name exists and runs this home
    /// (`Some(false)`: the name is another's, such as another Windows user's
    /// production host, since task names are machine-wide; never ended,
    /// replaced or deleted from here).
    fn task_is_ours(&self) -> Option<bool> {
        task_xml(&self.task()).map(|xml| self.names.owns(&xml))
    }

    fn kill_home_driver(&self) {
        for pid in windows_procs::pids_of(&self.home_driver()) {
            let _ = run(Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]));
        }
    }
}

/// The data of the HKCU `Run` value `name`, when it exists.
fn run_entry(name: &str) -> Option<String> {
    let out = no_window(Command::new("reg").args(["query", WINDOWS_RUN_KEY, "/v", name]))
        .output()
        .ok()?;
    out.status.success().then(|| console_text(&out.stdout))
}

/// The XML of the scheduled task `name`, when it exists.
fn task_xml(name: &str) -> Option<String> {
    let out = no_window(Command::new("schtasks").args(["/Query", "/TN", name, "/XML"]))
        .output()
        .ok()?;
    out.status.success().then(|| console_text(&out.stdout))
}

/// What `schtasks` and `reg` print into a pipe: UTF-8 when it is, else the
/// console (OEM) code page, so a user folder with letters outside ASCII
/// (`C:\Users\José`) still matches [`ServiceNames::owns`].
fn console_text(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::{CP_OEMCP, MultiByteToWideChar};
        let Ok(len) = i32::try_from(bytes.len()) else {
            return String::from_utf8_lossy(bytes).into_owned();
        };
        // SAFETY: the input and output buffers are sized and their lengths
        // passed; the first call only measures.
        unsafe {
            let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, std::ptr::null_mut(), 0);
            if n > 0 {
                let mut wide = vec![0u16; n as usize];
                let n = MultiByteToWideChar(CP_OEMCP, 0, bytes.as_ptr(), len, wide.as_mut_ptr(), n);
                if n > 0 {
                    return String::from_utf16_lossy(&wide[..n as usize]);
                }
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn system_root() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
}

/// Whether a `schtasks` failure is Task Scheduler refusing this user.
fn access_denied(e: &Error) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    // English text, and the HRESULT localized Windows prints.
    s.contains("access is denied") || s.contains("0x80070005")
}

/// How [`WindowsTask`] starts the logon entry's driver now, without waiting
/// for the next sign-in (CUA-3513): `cmd.exe` with the logon entry's own
/// `/d /c set …&& driver … >> log` line, in a console that has no window
/// (`CREATE_NO_WINDOW`), in its own process group.
///
/// Not the logon entry's `conhost.exe --headless` wrapper: started by a
/// program, headless conhost takes its standard input and output for the
/// pseudo-console's pipes, and with them closed (a background program has
/// none to give it) it shuts the console down at once, so the driver never
/// ran and no `driver.log` was written. At sign-in Explorer starts it with
/// no standard handles, which is why only the next sign-in brought the host
/// up. A console without a window is what `--headless` gives at sign-in.
pub fn windows_start_now(spec: &ServiceSpec, system_root: &str) -> (String, String) {
    (
        format!("{system_root}\\System32\\cmd.exe"),
        windows_cmd_line(spec),
    )
}

/// `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`, plus
/// `CREATE_BREAKAWAY_FROM_JOB` on the first try so the driver outlives a
/// job that kills its processes on close (a terminal's, an installer's).
#[cfg(windows)]
const START_NOW_FLAGS: u32 = 0x0800_0000 | 0x0000_0200;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

impl WindowsTask {
    fn start_run_entry(&self) -> Result<()> {
        let _ = self.stop();
        let spec = self.spec()?;
        let (program, line) = windows_start_now(&spec, &system_root());
        let spawn = |flags: u32| {
            let mut cmd = Command::new(&program);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // The line is a cmd.exe line, passed as is.
                cmd.raw_arg(&line);
                cmd.creation_flags(flags);
            }
            #[cfg(not(windows))]
            {
                let _ = flags;
                cmd.arg(&line);
            }
            cmd.current_dir(&spec.working_dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        };
        #[cfg(windows)]
        let spawned =
            spawn(START_NOW_FLAGS | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| spawn(START_NOW_FLAGS));
        #[cfg(not(windows))]
        let spawned = spawn(0);
        spawned
            .map(drop)
            .map_err(|e| Error::Service(format!("start {program}: {e}")))
    }
}

impl ServiceManager for WindowsTask {
    fn kind(&self) -> RunnerKind {
        RunnerKind::WindowsTask
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        // schtasks reads UTF-16 (the header says so) but also accepts UTF-8
        // without a BOM when the declaration matches; write UTF-16LE + BOM.
        let xml = windows_task_xml(spec, &self.user, &self.task());
        let mut bytes = vec![0xFF, 0xFE];
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        if let Some(parent) = self.xml_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.xml_path, bytes)?;
        remove_legacy_task();
        if self.task_is_ours() == Some(false) {
            // Task names are machine-wide: this one is another Windows
            // user's host. Never replaced; this user's runs from the logon
            // entry instead.
            tracing::info!(task = %self.task(), "the task name is another user's; using the logon entry");
            return self.install_run_entry(spec);
        }
        let created = run(Command::new("schtasks").args([
            "/Create",
            "/TN",
            &self.task(),
            "/XML",
            &self.xml_path.to_string_lossy(),
            "/F",
        ]));
        match created {
            Ok(_) => {
                self.remove_run_entry();
                Ok(())
            }
            Err(e) if access_denied(&e) => {
                tracing::info!("scheduled task refused ({e}); using the logon entry");
                self.install_run_entry(spec)
            }
            Err(e) => Err(e),
        }
    }
    fn start(&self) -> Result<()> {
        if !self.run_entry_installed() {
            if self.task_is_ours() == Some(false) {
                return Err(Error::Service(format!(
                    "the scheduled task {} runs another Windows user's host; run `{} host setup` again",
                    self.task(),
                    crate::cli()
                )));
            }
            let _ = run(Command::new("schtasks").args(["/End", "/TN", &self.task()]));
            return run(Command::new("schtasks").args(["/Run", "/TN", &self.task()])).map(drop);
        }
        self.start_run_entry()
    }
    fn stop(&self) -> Result<()> {
        if !self.run_entry_installed() {
            if self.task_is_ours() == Some(false) {
                return Ok(());
            }
            let ended = run(Command::new("schtasks").args(["/End", "/TN", &self.task()]));
            // `/End` stops only the task's own process (conhost): cmd.exe
            // and the driver under it keep running, holding the ports, so a
            // setup again moved to other ports and its new driver never
            // started (CUA-1749, CUA-3513).
            self.kill_home_driver();
            return ended.map(drop);
        }
        if let Ok(spec) = self.spec() {
            for pid in windows_procs::pids_of(&spec.program) {
                let _ = run(Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]));
            }
        }
        Ok(())
    }
    fn uninstall(&self) -> Result<()> {
        // The logon entry's spec names the driver; the task has none, and
        // runs this home's own (`<host>/bin`).
        let program = Some(
            self.spec()
                .ok()
                .map(|s| s.program)
                .unwrap_or_else(|| self.home_driver()),
        );
        if self.run_entry_installed() {
            let _ = self.stop();
        }
        self.remove_run_entry();
        if self.task_is_ours() == Some(true) {
            let _ = run(Command::new("schtasks").args(["/End", "/TN", &self.task()]));
            let _ = run(Command::new("schtasks").args(["/Delete", "/TN", &self.task(), "/F"]));
        }
        // `/End` stops only the task's own process (conhost): the cmd.exe
        // and the driver under it keep running, the driver still online and
        // cmd.exe holding driver.log open, so ~/.cua/host could not be
        // removed (CUA-1749).
        for pid in program
            .map(|p| windows_procs::pids_of(&p))
            .unwrap_or_default()
        {
            let _ = run(Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]));
        }
        let _ = std::fs::remove_file(&self.xml_path);
        remove_legacy_task();
        self.retire_legacy_names();
        Ok(())
    }
    fn retire_legacy_names(&self) -> Vec<String> {
        let legacy = self.names.legacy();
        let mut retired = Vec::new();
        for name in legacy.windows_task {
            let Some(xml) = task_xml(&name) else { continue };
            if !self.names.owns(&xml) {
                // Another home's task (production's, most often): never
                // ended or deleted.
                continue;
            }
            tracing::info!(task = %name, "retiring this home's host task from its old name");
            let _ = run(Command::new("schtasks").args(["/End", "/TN", &name]));
            let _ = run(Command::new("schtasks").args(["/Delete", "/TN", &name, "/F"]));
            retired.push(name);
        }
        for name in legacy.windows_run {
            let Some(data) = run_entry(&name) else {
                continue;
            };
            if !self.names.owns(&data) {
                continue;
            }
            tracing::info!(value = %name, "retiring this home's logon entry from its old name");
            let _ = run(Command::new("reg").args(["delete", WINDOWS_RUN_KEY, "/v", &name, "/f"]));
            retired.push(name);
        }
        if !retired.is_empty() {
            // The driver the old task or entry started is this home's own
            // (`owns` checked its path); the new one starts it again.
            self.kill_home_driver();
        }
        retired
    }
    fn state(&self) -> ServiceState {
        if self.run_entry_installed() {
            let pids = self
                .spec()
                .map(|s| windows_procs::pids_of(&s.program))
                .unwrap_or_default();
            return ServiceState {
                installed: true,
                running: !pids.is_empty(),
                kind: "windows-task".into(),
                detail: format!(
                    "logon entry {WINDOWS_RUN_KEY}\\{}; pids {pids:?}",
                    self.run_value()
                ),
            };
        }
        let out = no_window(Command::new("schtasks").args([
            "/Query",
            "/TN",
            &self.task(),
            "/FO",
            "CSV",
            "/NH",
        ]))
        .output();
        let (installed, text) = match out {
            Ok(o) if o.status.success() && self.task_is_ours() != Some(false) => {
                (true, String::from_utf8_lossy(&o.stdout).to_string())
            }
            _ => (false, String::new()),
        };
        ServiceState {
            installed,
            running: text.contains("Running"),
            kind: "windows-task".into(),
            detail: text.trim().to_string(),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool {
        crate::ports::os_port_free(addr, transport)
    }
}

/// Running processes by executable path (the logon entry has no pidfile:
/// the driver it starts at logon is not our child).
mod windows_procs {
    use std::path::Path;

    #[cfg(windows)]
    pub fn pids_of(program: &Path) -> Vec<u32> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        };
        // `envs/staging` joins into a Windows path with a forward slash;
        // Windows reports the image with backslashes. Compare both with
        // backslashes, so the staging home's driver is found (CUA-3512).
        let want = program.to_string_lossy().replace('/', "\\").to_lowercase();
        let want_name = program
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let mut out = vec![];
        // SAFETY: plain Win32 calls on handles we own and close; buffers
        // are sized and their lengths passed.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                let len = e
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
                if name == want_name {
                    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
                    if !h.is_null() {
                        let mut buf = [0u16; 1024];
                        let mut n = buf.len() as u32;
                        if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n) != 0
                            && String::from_utf16_lossy(&buf[..n as usize])
                                .replace('/', "\\")
                                .to_lowercase()
                                == want
                        {
                            out.push(e.th32ProcessID);
                        }
                        CloseHandle(h);
                    }
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        out
    }

    #[cfg(not(windows))]
    pub fn pids_of(_program: &Path) -> Vec<u32> {
        vec![]
    }
}

/// Systemd when `systemd_dir` (`/run/systemd/system`) exists, else the plain
/// process. A seam for tests: it checks the filesystem.
fn linux_runner(systemd_dir: &Path) -> RunnerKind {
    if systemd_dir.is_dir() {
        RunnerKind::Systemd
    } else {
        RunnerKind::Process
    }
}

/// The warning `cua host setup` prints when `requested` was `Auto` and the
/// stored runner `resolved` is the plain process; `None` otherwise.
pub fn auto_fallback_warning(requested: RunnerKind, resolved: &str) -> Option<&'static str> {
    (requested == RunnerKind::Auto && resolved == RunnerKind::Process.as_str()).then_some(
        "warning: systemd is not running here, so `--runner auto` set the host up as a plain process.\n\
It will not come back after a reboot or a crash. To fix it, either:\n\
\x20 - turn on systemd (on WSL: systemd=true under [boot] in /etc/wsl.conf, then wsl --shutdown) and run `cua host setup --runner systemd` again with the same options, or\n\
\x20 - run `cua host start` after each boot (from your init system or container start script).\n\
Pass `--runner process` to keep the plain process without this warning.",
    )
}

/// The note `cua host status` prints under a `process` runner.
pub const PROCESS_RUNNER_NOTE: &str = "  note: the process runner does not start the host again after a reboot or a crash; run `cua host start` after each boot.";

/// A detached process with a pidfile: the fallback where no init system
/// runs (containers). Not restarted on reboot or crash; `cua host setup`
/// warns about it ([`auto_fallback_warning`]).
pub struct ProcessRunner {
    /// Directory for `service.json` and `driver.pid`.
    pub dir: PathBuf,
}

impl ProcessRunner {
    fn spec_path(&self) -> PathBuf {
        self.dir.join("service.json")
    }
    fn pid_path(&self) -> PathBuf {
        self.dir.join("driver.pid")
    }
    fn pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.pid_path())
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

/// Whether `pid` is a live (non-zombie) process.
pub fn process_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // Field 3 (after the parenthesised comm) is the state.
            return stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|s| s != "Z" && s != "X");
        }
        false
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // SAFETY: signal 0 only checks for existence/permission.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

impl ServiceManager for ProcessRunner {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Process
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(
            self.spec_path(),
            serde_json::to_vec_pretty(spec).map_err(|e| Error::Internal(e.to_string()))?,
        )?;
        Ok(())
    }
    fn start(&self) -> Result<()> {
        let _ = self.stop();
        let spec: ServiceSpec = serde_json::from_slice(&std::fs::read(self.spec_path())?)
            .map_err(|e| Error::Service(format!("service.json: {e}")))?;
        if let Some(parent) = spec.log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&spec.log_path)?;
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            cmd.process_group(0);
        }
        let child = cmd
            .spawn()
            .map_err(|e| Error::Service(format!("start {}: {e}", spec.program.display())))?;
        std::fs::write(self.pid_path(), child.id().to_string())?;
        // Reap it from a thread so it never lingers as a zombie of ours.
        let mut child = child;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        let Some(pid) = self.pid() else {
            return Ok(());
        };
        #[cfg(unix)]
        {
            // `start` made the driver the leader of its own process group:
            // signal the whole group, so what it started there goes too
            // (the host's cua daemon, which it starts on the first Space
            // create; CUA-1804). Only while the pid still leads that group:
            // a reused pid gets a signal of its own, never its group.
            // SAFETY: getpgid only reads.
            let group = unsafe { libc::getpgid(pid as libc::pid_t) } == pid as libc::pid_t;
            let target = if group {
                -(pid as libc::pid_t)
            } else {
                pid as libc::pid_t
            };
            // SAFETY: plain signal delivery to the pid (group) we started.
            unsafe { libc::kill(target, libc::SIGTERM) };
            let alive = || {
                if group {
                    // SAFETY: signal 0 only checks for existence.
                    unsafe { libc::kill(target, 0) == 0 }
                } else {
                    process_alive(pid)
                }
            };
            for _ in 0..50 {
                if !alive() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if alive() {
                // SAFETY: as above.
                unsafe { libc::kill(target, libc::SIGKILL) };
            }
        }
        #[cfg(not(unix))]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output();
        }
        let _ = std::fs::remove_file(self.pid_path());
        Ok(())
    }
    fn uninstall(&self) -> Result<()> {
        self.stop()?;
        let _ = std::fs::remove_file(self.spec_path());
        Ok(())
    }
    fn state(&self) -> ServiceState {
        let pid = self.pid();
        ServiceState {
            installed: self.spec_path().exists(),
            running: pid.is_some_and(process_alive),
            kind: "process".into(),
            detail: pid.map(|p| format!("pid {p}")).unwrap_or_default(),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool {
        crate::ports::os_port_free(addr, transport)
    }
}

// ------------------------------------------------------------- app helper

/// `~/.cua/helper/service.json` (Local: `~/.cua-local/helper/service.json`):
/// what the Cua Spaces app's helper runs (a [`ServiceSpec`] as JSON, 0600).
/// A fixed path under the user's home, per channel: the helper's launchd
/// plist is sealed in the app and cannot name a cua home, so the app's
/// `CuaHome` Info.plist key (stamped per channel) says where it is. Read by
/// `ServiceLauncher.swift`.
pub fn helper_spec_path(user_home: &Path) -> PathBuf {
    helper_spec_path_for(CuaChannel::current(), user_home)
}

/// [`helper_spec_path`] for `channel`: [`CuaChannel::helper_spec_path`]
/// below `user_home`.
pub fn helper_spec_path_for(channel: CuaChannel, user_home: &Path) -> PathBuf {
    user_home
        .join(channel.home_dir_name())
        .join("helper")
        .join("service.json")
}

/// The service's launcher inside `app`: the app's own executable (what
/// `config.driver_bin` names for the app-helper runner; `<launcher>
/// check-permissions` answers for the app's permissions).
pub fn app_helper_launcher(app: &Path) -> PathBuf {
    app.join(APP_EXECUTABLE)
}

/// Whether `app` is a Cua Spaces app of `channel` that carries the helper.
///
/// The channel is told by the helper's plist, which is named after the
/// channel's launchd label ([`app_helper_plist`]): a Stable app has only
/// `com.trycua.spaces.helper.plist`, a Local app only
/// `com.trycua.spaces.local.helper.plist`. So an app of the other channel
/// never matches, and one channel never registers, starts or writes the spec
/// of the other's helper.
pub fn has_app_helper(app: &Path, channel: CuaChannel) -> bool {
    app.join(app_helper_plist(channel)).is_file()
        && (app.join(APP_HELPER_DAEMON).is_file()
            || app.join(APP_HELPER_DAEMON_BEFORE_0_15).is_file())
        && app.join(APP_EXECUTABLE).is_file()
}

/// Whether `launcher` is a Cua Spaces app's executable (a host set up with
/// the app-helper runner names it as its driver).
pub fn is_app_helper_launcher(launcher: &Path) -> bool {
    launcher.ends_with(APP_EXECUTABLE)
        && launcher
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .is_some_and(|app| app.extension().is_some_and(|e| e == "app"))
}

/// The installed Cua Spaces app that carries the helper, if any (macOS), of
/// this process's channel ([`CuaChannel::current`]); see
/// [`find_app_helper_for`].
pub fn find_app_helper() -> Option<PathBuf> {
    find_app_helper_for(CuaChannel::current())
}

/// The installed Cua Spaces app of `channel` that carries the helper, if
/// any (macOS): `CUA_SPACES_APP` when set (empty or `none`: no app), else
/// the app this process runs from (the app itself, or the `cua` it links
/// onto PATH), else where the channel installs its app
/// ([`CuaChannel::app_path`]: `/Applications/Cua Spaces.app`, or
/// `~/Applications/Cua Spaces Local.app`), else `~/Applications` under the
/// channel's app name.
///
/// An app of the other channel is never picked, whichever way it was named
/// ([`has_app_helper`]): a Local CLI must never drive Stable's helper, nor
/// the Stable CLI the Local one.
pub fn find_app_helper_for(channel: CuaChannel) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let spaces_app = std::env::var_os("CUA_SPACES_APP");
    let exe_app = std::env::current_exe()
        .and_then(|e| e.canonicalize())
        .ok()
        .and_then(|exe| app_containing(&exe));
    let candidates = app_helper_candidates(channel, exe_app, std::env::var_os("HOME").as_deref());
    pick_app_helper(channel, spaces_app.as_deref(), candidates)
}

/// `X.app` for `.../X.app/Contents/MacOS/<exe>`.
fn app_containing(exe: &Path) -> Option<PathBuf> {
    exe.parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|app| app.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// The places to look for `channel`'s app, best first: the app this process
/// runs from, where the channel installs its app, then `~/Applications`
/// under the channel's app name. Never the other channel's.
fn app_helper_candidates(
    channel: CuaChannel,
    exe_app: Option<PathBuf>,
    home: Option<&std::ffi::OsStr>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = exe_app.into_iter().collect();
    candidates.push(channel.app_path());
    if let Some(home) = home {
        candidates.push(
            PathBuf::from(home)
                .join("Applications")
                .join(format!("{}.app", channel.app_name())),
        );
    }
    candidates
}

/// The app of `channel` with the helper among `candidates`, or the one
/// `CUA_SPACES_APP` names (`spaces_app`; empty or `none`: no app, and the
/// candidates are not consulted).
fn pick_app_helper(
    channel: CuaChannel,
    spaces_app: Option<&std::ffi::OsStr>,
    candidates: Vec<PathBuf>,
) -> Option<PathBuf> {
    if let Some(v) = spaces_app {
        let p = PathBuf::from(v);
        return (!v.is_empty() && v != "none" && has_app_helper(&p, channel)).then_some(p);
    }
    candidates
        .into_iter()
        .find(|app| has_app_helper(app, channel))
}

/// What `CuaSpacesMac --service-helper <command>` reports: the
/// `SMAppService` status of the helper (`enabled`, `not-registered`,
/// `requires-approval`, `not-found`) and an error, if the command failed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct HelperRegistration {
    /// The status after the command.
    pub status: String,
    /// Why the command failed.
    #[serde(default)]
    pub error: Option<String>,
}

/// The Cua Spaces app's login item: the app's own executable
/// (`CuaSpacesMac --service-helper run`), registered with
/// `SMAppService.agent` by that executable. It reads [`helper_spec_path`]
/// and runs the cua-spacesd sealed in the app with it, in the GUI session;
/// macOS holds the app responsible for it, so Screen Recording and
/// Accessibility are granted to "Cua Spaces" (bundle id
/// `com.trycua.spaces.macos`, the Cua team), once, and survive app updates.
/// The Local app's helper is its own (launchd label
/// `com.trycua.spaces.local.helper`, spec in `~/.cua-local`), beside
/// Stable's.
/// (A separate helper bundle got two names in System Settings: macOS lists
/// a login item's Screen Recording under the app that contains it.)
pub struct AppHelper {
    /// The app (`/Applications/Cua Spaces.app`).
    pub app: PathBuf,
    /// [`helper_spec_path_for`] the app's channel.
    pub spec_path: PathBuf,
    /// The user id (`gui/<uid>`).
    pub uid: u32,
    /// The channel of `app`: names its launchd label and the app in
    /// messages.
    pub channel: CuaChannel,
}

impl AppHelper {
    /// The helper of `app` (of this process's channel) for the current user.
    pub fn for_current_user(app: PathBuf, user_home: &Path) -> Self {
        Self::for_channel(CuaChannel::current(), app, user_home)
    }

    /// The helper of `app`, the app of `channel`, for the current user.
    pub fn for_channel(channel: CuaChannel, app: PathBuf, user_home: &Path) -> Self {
        #[cfg(unix)]
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 0;
        Self {
            app,
            spec_path: helper_spec_path_for(channel, user_home),
            uid,
            channel,
        }
    }

    fn target(&self) -> String {
        format!("gui/{}/{}", self.uid, self.channel.helper_label())
    }

    /// `CuaSpacesMac --service-helper <command>`.
    pub fn registration(&self, command: &str) -> Result<HelperRegistration> {
        let exe = self.app.join(APP_EXECUTABLE);
        let out = Command::new(&exe)
            .args(["--service-helper", command])
            .output()
            .map_err(|e| Error::Service(format!("{}: {e}", exe.display())))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let reg: HelperRegistration = text
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str(l.trim()).ok())
            .ok_or_else(|| {
                Error::Service(format!(
                    "Cua Spaces could not {command} its helper: {}{}",
                    text.trim(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ))
            })?;
        Ok(reg)
    }

    fn loaded(&self) -> bool {
        run(Command::new("launchctl").args(["print", &self.target()])).is_ok()
    }

    /// launchd could not start the helper's program at its last try.
    fn spawn_failed(&self) -> bool {
        Command::new("launchctl")
            .args(["print", &self.target()])
            .output()
            .map(|o| {
                let p = String::from_utf8_lossy(&o.stdout);
                p.contains("spawn failed") || p.contains("last exit code = 78")
            })
            .unwrap_or(false)
    }
}

impl ServiceManager for AppHelper {
    fn kind(&self) -> RunnerKind {
        RunnerKind::AppHelper
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        let mut spec = spec.clone();
        // The service runs only the app's own cua-spacesd; the spec says
        // which launcher it was written for.
        spec.program = app_helper_launcher(&self.app);
        let json =
            serde_json::to_string_pretty(&spec).map_err(|e| Error::Internal(e.to_string()))?;
        crate::host::write_secret(&self.spec_path, &json)?;
        // A registration made by another copy of the app (replaced since:
        // an update, a reinstall, a downgrade and back) can leave launchd
        // resolving the helper in a bundle that is gone ("Invalid or missing
        // Program", exit 78). Registering again from this app binds it here.
        if !self.state().running {
            let _ = self.registration("unregister");
        }
        let reg = self.registration("register")?;
        helper_registered(&reg, self.channel.app_name())
    }
    fn start(&self) -> Result<()> {
        // A fresh registration can take a moment to reach launchd.
        let mut waited = std::time::Duration::ZERO;
        let step = std::time::Duration::from_millis(200);
        while !self.loaded() && waited < std::time::Duration::from_secs(5) {
            std::thread::sleep(step);
            waited += step;
        }
        let kick = || run(Command::new("launchctl").args(["kickstart", "-k", &self.target()]));
        let mut result = kick();
        // Started but gone at once with launchd's "could not find the
        // program" (a stale bundle): register from this app and try once more.
        std::thread::sleep(std::time::Duration::from_secs(2));
        if result.is_ok() && !self.state().running && self.spawn_failed() {
            let _ = self.registration("unregister");
            helper_registered(&self.registration("register")?, self.channel.app_name())?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            result = kick();
        }
        result.map(drop).map_err(|e| {
            let name = self.channel.app_name();
            Error::Service(format!(
                "macOS did not start {name} ({e}); if it keeps failing, turn on \
                     {name} in System Settings > General > Login Items & Extensions"
            ))
        })
    }
    fn stop(&self) -> Result<()> {
        if !self.state().running {
            return Ok(());
        }
        // The launcher passes it on and exits 0, which launchd leaves
        // stopped (KeepAlive SuccessfulExit=false).
        run(Command::new("launchctl").args(["kill", "SIGTERM", &self.target()])).map(drop)
    }
    fn uninstall(&self) -> Result<()> {
        let _ = self.stop();
        let _ = std::fs::remove_file(&self.spec_path);
        // Unregistering also takes it out of Login Items.
        let _ = self.registration("unregister");
        Ok(())
    }
    fn state(&self) -> ServiceState {
        let printed = Command::new("launchctl")
            .args(["print", &self.target()])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
        ServiceState {
            installed: self.spec_path.is_file() && printed.is_some(),
            running: printed
                .as_deref()
                .is_some_and(|p| p.contains("state = running")),
            kind: RunnerKind::AppHelper.as_str().into(),
            detail: format!("{} in {}", self.channel.app_name(), self.app.display()),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool {
        crate::ports::os_port_free(addr, transport)
    }
}

/// A registration that did not leave the helper allowed to run, in words.
fn helper_registered(reg: &HelperRegistration, name: &str) -> Result<()> {
    match (reg.status.as_str(), &reg.error) {
        ("enabled", _) => Ok(()),
        ("requires-approval", _) => Err(Error::Service(format!(
            "macOS needs you to allow {name}: turn on {name} in System Settings > \
             General > Login Items & Extensions, then try again"
        ))),
        (status, Some(e)) => Err(Error::Service(format!(
            "{name} could not register {name} ({status}): {e}"
        ))),
        (status, None) => Err(Error::Service(format!(
            "{name} could not register {name} ({status})"
        ))),
    }
}

/// The app-helper runner on a Mac whose Cua Spaces app (with the helper)
/// is gone: nothing installed, nothing runs, and nothing can start.
pub struct MissingAppHelper;

impl ServiceManager for MissingAppHelper {
    fn kind(&self) -> RunnerKind {
        RunnerKind::AppHelper
    }
    fn install(&self, _: &ServiceSpec) -> Result<()> {
        Err(missing_app())
    }
    fn start(&self) -> Result<()> {
        Err(missing_app())
    }
    fn stop(&self) -> Result<()> {
        Ok(())
    }
    fn uninstall(&self) -> Result<()> {
        Ok(())
    }
    fn state(&self) -> ServiceState {
        ServiceState {
            installed: false,
            running: false,
            kind: RunnerKind::AppHelper.as_str().into(),
            detail: format!("{}: the Cua Spaces app is not installed", app_helper_name()),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, transport: crate::ports::Transport) -> bool {
        crate::ports::os_port_free(addr, transport)
    }
}

fn missing_app() -> Error {
    Error::Service(format!(
        "{name} lives in the Cua Spaces app, which is not installed; install it \
         again, or run `{cli} host setup --runner launchd`",
        name = app_helper_name(),
        cli = crate::cli()
    ))
}

/// The hand-written LaunchAgent an install made before the helper (`cua
/// host setup` without the app, or Cua Spaces 0.9.4 and earlier), as found
/// on this Mac.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LegacyAgent {
    /// `~/Library/LaunchAgents/com.trycua.spacesd.host.plist` exists.
    pub plist: bool,
    /// launchd has it loaded.
    pub loaded: bool,
    /// It runs now.
    pub running: bool,
}

impl LegacyAgent {
    /// Anything of it is left.
    pub fn present(&self) -> bool {
        self.plist || self.loaded
    }
}

impl Launchd {
    /// What is left of this LaunchAgent.
    pub fn legacy(&self) -> LegacyAgent {
        let printed = Command::new("launchctl")
            .args(["print", &self.target()])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
        LegacyAgent {
            plist: self.plist_path().is_file(),
            loaded: printed.is_some(),
            running: printed
                .as_deref()
                .is_some_and(|p| p.contains("state = running")),
        }
    }

    /// The plist's path.
    pub fn plist_file(&self) -> PathBuf {
        self.plist_path()
    }

    /// Stops it and takes it out of launchd, leaving the plist in place.
    pub fn bootout(&self) -> Result<()> {
        let target = self.target();
        if !launchd_loaded(&target, &mut |args: &[&str]| {
            run(Command::new("launchctl").args(args))
        }) {
            return Ok(());
        }
        let _ = run(Command::new("launchctl").args(["bootout", &target]));
        // bootout returns before the teardown ends.
        for _ in 0..50 {
            if !launchd_loaded(&target, &mut |args: &[&str]| {
                run(Command::new("launchctl").args(args))
            }) {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err(Error::Service(format!(
            "launchd did not let go of {target}"
        )))
    }
}

/// Records calls; for tests of everything above the runner.
#[derive(Default)]
pub struct FakeServiceManager {
    /// `install`, `start`, `stop`, `uninstall` in order.
    pub calls: Mutex<Vec<String>>,
    /// The last installed spec.
    pub spec: Mutex<Option<ServiceSpec>>,
    running: Mutex<bool>,
    /// `start` fails while this is set.
    pub fail_start: Mutex<bool>,
    /// Ports another program holds (any address, TCP and UDP alike): never
    /// free. Every other port is free, so no test binds a real one.
    pub held_by_others: Mutex<Vec<u16>>,
    /// Ports this service holds while it runs (free once it is stopped).
    pub held_while_running: Mutex<Vec<u16>>,
    /// Old names `retire_legacy_names` reports retiring (once).
    pub legacy_names: Mutex<Vec<String>>,
}

impl ServiceManager for FakeServiceManager {
    fn kind(&self) -> RunnerKind {
        RunnerKind::Process
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        self.calls.lock().unwrap().push("install".into());
        *self.spec.lock().unwrap() = Some(spec.clone());
        Ok(())
    }
    fn start(&self) -> Result<()> {
        self.calls.lock().unwrap().push("start".into());
        if *self.fail_start.lock().unwrap() {
            return Err(Error::Service("the service would not start".into()));
        }
        *self.running.lock().unwrap() = true;
        Ok(())
    }
    fn stop(&self) -> Result<()> {
        self.calls.lock().unwrap().push("stop".into());
        *self.running.lock().unwrap() = false;
        Ok(())
    }
    fn uninstall(&self) -> Result<()> {
        self.calls.lock().unwrap().push("uninstall".into());
        *self.spec.lock().unwrap() = None;
        *self.running.lock().unwrap() = false;
        Ok(())
    }
    fn retire_legacy_names(&self) -> Vec<String> {
        let retired = std::mem::take(&mut *self.legacy_names.lock().unwrap());
        if !retired.is_empty() {
            self.calls.lock().unwrap().push("retire".into());
        }
        retired
    }
    fn state(&self) -> ServiceState {
        ServiceState {
            installed: self.spec.lock().unwrap().is_some(),
            running: *self.running.lock().unwrap(),
            kind: "fake".into(),
            detail: String::new(),
        }
    }
    fn port_free(&self, addr: std::net::SocketAddr, _: crate::ports::Transport) -> bool {
        let port = addr.port();
        !self.held_by_others.lock().unwrap().contains(&port)
            && !(*self.running.lock().unwrap()
                && self.held_while_running.lock().unwrap().contains(&port))
    }
}

/// The manager for `kind` (resolving `Auto`), for the current user.
/// `names` are the cua home's ([`ServiceNames::for_home`]).
pub fn manager_for(
    kind: RunnerKind,
    user_home: &Path,
    host_dir: &Path,
    names: ServiceNames,
) -> Box<dyn ServiceManager> {
    match kind.resolve() {
        RunnerKind::Systemd => Box::new(Systemd::for_current_user(user_home, names)),
        RunnerKind::Launchd => Box::new(Launchd::for_current_user(user_home, names)),
        RunnerKind::AppHelper => match find_app_helper() {
            Some(app) => Box::new(AppHelper::for_current_user(app, user_home)),
            // The app was removed (or replaced by one without the helper):
            // nothing can run it; report it as not installed.
            None => Box::new(MissingAppHelper),
        },
        RunnerKind::WindowsTask => Box::new(WindowsTask::for_current_user(host_dir, names)),
        RunnerKind::Process | RunnerKind::Auto => Box::new(ProcessRunner {
            dir: host_dir.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {

    /// A scripted `launchctl`: `loaded` says whether `print` succeeds (by
    /// call), `bootstrap` whether each bootstrap succeeds.
    struct FakeLaunchctl {
        calls: Vec<String>,
        loaded: Box<dyn FnMut(usize) -> bool>,
        bootstraps: Vec<bool>,
        prints: usize,
    }

    impl FakeLaunchctl {
        fn new(loaded: impl FnMut(usize) -> bool + 'static, bootstraps: Vec<bool>) -> Self {
            Self {
                calls: vec![],
                loaded: Box::new(loaded),
                bootstraps,
                prints: 0,
            }
        }
        fn call(&mut self, args: &[&str]) -> Result<String> {
            self.calls.push(args[0].to_string());
            let fail = |m: &str| {
                Err(Error::Service(format!(
                    "\"launchctl\" \"{}\" failed: {m}",
                    args[0]
                )))
            };
            match args[0] {
                "print" => {
                    self.prints += 1;
                    if (self.loaded)(self.prints) {
                        Ok("state = running".into())
                    } else {
                        fail("Could not find service")
                    }
                }
                "bootstrap" => {
                    if self.bootstraps.is_empty() || self.bootstraps.remove(0) {
                        Ok(String::new())
                    } else {
                        fail("Bootstrap failed: 5: Input/output error")
                    }
                }
                _ => Ok(String::new()),
            }
        }
        fn restart(&mut self) -> Result<()> {
            launchd_restart(
                "gui/501",
                "gui/501/x",
                "/p.plist",
                &mut |a: &[&str]| self.call(a),
                &mut |_| {},
            )
        }
    }

    #[test]
    fn launchd_start_when_not_loaded_bootstraps_once() {
        let mut lc = FakeLaunchctl::new(|_| false, vec![true]);
        lc.restart().unwrap();
        assert_eq!(lc.calls, ["enable", "print", "bootstrap"]);
    }

    #[test]
    fn launchd_start_waits_for_the_bootout_to_finish() {
        // Loaded for the first three prints: bootout is asynchronous.
        let mut lc = FakeLaunchctl::new(|n| n <= 3, vec![true]);
        lc.restart().unwrap();
        assert_eq!(
            lc.calls,
            [
                "enable",
                "print",
                "bootout",
                "print",
                "print",
                "print",
                "bootstrap"
            ]
        );
    }

    #[test]
    fn launchd_start_kickstarts_a_service_that_is_loaded_after_all() {
        // Not loaded at first; bootstrap fails (launchd loaded it itself).
        let mut lc = FakeLaunchctl::new(|n| n >= 2, vec![false]);
        lc.restart().unwrap();
        assert_eq!(
            lc.calls,
            ["enable", "print", "bootstrap", "print", "kickstart"]
        );
    }

    #[test]
    fn launchd_start_retries_then_says_what_failed_in_words() {
        let mut lc = FakeLaunchctl::new(|_| false, vec![false, false, true]);
        lc.restart().unwrap();
        assert_eq!(lc.calls.iter().filter(|c| *c == "bootstrap").count(), 3);

        let mut lc = FakeLaunchctl::new(|_| false, vec![false, false, false]);
        let e = lc.restart().unwrap_err().to_string();
        assert!(
            e.contains("macOS did not start the Cua host service"),
            "{e}"
        );
        assert!(e.contains("Bootstrap failed: 5: Input/output error"), "{e}");
        assert!(!e.contains("\"launchctl\""), "{e}");
    }
    use super::*;

    #[test]
    fn auto_detection_falls_back_to_a_process_without_systemd() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join("system");
        assert_eq!(linux_runner(&probe), RunnerKind::Process);
        std::fs::create_dir(&probe).unwrap();
        assert_eq!(linux_runner(&probe), RunnerKind::Systemd);
    }

    #[test]
    fn only_auto_resolved_to_a_process_warns() {
        let text = auto_fallback_warning(RunnerKind::Auto, "process").expect("warns");
        assert!(text.contains("will not come back after a reboot"), "{text}");
        assert!(text.contains("host setup --runner systemd"), "{text}");
        assert!(text.contains("host start"), "{text}");
        for (req, resolved) in [
            (RunnerKind::Process, "process"),
            (RunnerKind::Auto, "systemd"),
            (RunnerKind::Auto, "launchd"),
            (RunnerKind::Auto, "app-helper"),
            (RunnerKind::Auto, "windows-task"),
        ] {
            assert_eq!(auto_fallback_warning(req, resolved), None, "{resolved}");
        }
    }

    fn spec() -> ServiceSpec {
        ServiceSpec {
            program: "/home/ada/.cua/host/bin/cua-spacesd".into(),
            args: vec![
                "join".into(),
                "--relay".into(),
                "https://relay.cua.ai".into(),
                "--relay-token-file".into(),
                "/home/ada/.cua/host/machine token".into(),
            ],
            env: vec![("CUA_ENV_LOG".into(), "info".into())],
            log_path: "/home/ada/.cua/host/driver.log".into(),
            working_dir: "/home/ada".into(),
        }
    }

    #[test]
    fn systemd_user_and_system_units() {
        let user = systemd_unit(&spec(), false);
        assert!(user.contains("WantedBy=default.target"));
        assert!(user.contains("graphical-session.target"));
        assert!(!user.contains("OOMScoreAdjust"));
        assert!(user.contains(
            "ExecStart=/home/ada/.cua/host/bin/cua-spacesd join --relay https://relay.cua.ai --relay-token-file \"/home/ada/.cua/host/machine token\"\n"
        ));
        assert!(user.contains("Environment=CUA_ENV_LOG=info\n"));
        assert!(user.contains("Restart=always"));
        // A Space or the daemon killed for memory does not stop the service.
        assert!(user.contains("OOMPolicy=continue\n"));
        assert!(user.contains("StandardOutput=append:/home/ada/.cua/host/driver.log"));
        let system = systemd_unit(&spec(), true);
        assert!(system.contains("WantedBy=multi-user.target"));
        assert!(system.contains("OOMScoreAdjust=-1000"));
    }

    #[test]
    fn systemd_quotes_specials() {
        assert_eq!(systemd_quote("plain-arg"), "plain-arg");
        assert_eq!(systemd_quote("a b"), "\"a b\"");
        assert_eq!(systemd_quote("100%"), "\"100%%\"");
        assert_eq!(systemd_quote(""), "\"\"");
    }

    #[test]
    fn launchd_plist_is_a_gui_agent() {
        let mut s = spec();
        s.args.push("<&>".into());
        let p = launchd_plist(&s, "com.trycua.spacesd.host");
        assert!(p.contains("<string>com.trycua.spacesd.host</string>"));
        assert!(p.contains("<key>LimitLoadToSessionType</key>\n  <string>Aqua</string>"));
        assert!(p.contains("<key>KeepAlive</key>\n  <true/>"));
        assert!(p.contains("<string>&lt;&amp;&gt;</string>"));
        assert!(p.contains("<key>CUA_ENV_LOG</key>\n    <string>info</string>"));
        assert!(p.contains("<string>/home/ada/.cua/host/machine token</string>"));
        // Well-formed enough for plutil: every <dict>/<array> closes.
        assert_eq!(p.matches("<dict>").count(), p.matches("</dict>").count());
        assert_eq!(p.matches("<array>").count(), p.matches("</array>").count());
    }

    #[test]
    fn windows_task_runs_interactively_at_logon() {
        let mut s = spec();
        s.program = r"C:\Users\Ada\.cua\host\bin\cua-spacesd.exe".into();
        s.log_path = r"C:\Users\Ada\.cua\host\driver.log".into();
        let x = windows_task_xml(&s, r"PC\Ada", WINDOWS_TASK_NAME);
        assert!(x.contains("<LogonTrigger>"));
        assert!(x.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(x.contains(r"<UserId>PC\Ada</UserId>"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains("<RestartOnFailure>"));
        assert!(x.contains(r"<URI>\Cua\SpacesdHost</URI>"));
        // Arguments: env chain, quoted path with a space, log redirect.
        assert!(x.contains("set &quot;CUA_ENV_LOG=info&quot;&amp;&amp; "));
        assert!(x.contains("&quot;/home/ada/.cua/host/machine token&quot;"));
        assert!(x.contains(r"&gt;&gt; C:\Users\Ada\.cua\host\driver.log 2&gt;&amp;1"));
        let any = windows_task_xml(&s, "", WINDOWS_TASK_NAME);
        assert!(!any.contains("<UserId>"));
    }

    #[test]
    fn windows_driver_runs_in_a_headless_console() {
        let mut s = spec();
        s.program = r"C:\Users\Ada\.cua\host\bin\cua-spacesd.exe".into();
        s.log_path = r"C:\Users\Ada\.cua\host\driver.log".into();
        let x = windows_task_xml(&s, r"PC\Ada", WINDOWS_TASK_NAME);
        assert!(x.contains(r"<Command>%SystemRoot%\System32\conhost.exe</Command>"));
        assert!(x.contains(r"<Arguments>--headless %SystemRoot%\System32\cmd.exe /d /c set "));
        // The logon entry runs the same line with real paths.
        let v = windows_run_value(&s, r"C:\Windows");
        assert!(v.starts_with(
            r"C:\Windows\System32\conhost.exe --headless C:\Windows\System32\cmd.exe /d /c set "
        ));
        assert!(v.contains(r"C:\Users\Ada\.cua\host\bin\cua-spacesd.exe"));
        assert!(v.ends_with(r">> C:\Users\Ada\.cua\host\driver.log 2>&1"));
        let v = windows_run_value(&s, r"C:\Program Files\Win");
        assert!(v.starts_with(r#""C:\Program Files\Win\System32\conhost.exe" --headless "#));
    }

    #[test]
    fn the_logon_entry_starts_now_without_headless_conhost() {
        // CUA-3513: started by a program, `conhost --headless` takes its
        // closed standard handles for the pseudo-console and exits, so the
        // driver never ran until the next sign-in. Starting now runs the
        // logon entry's own cmd.exe line in a console without a window.
        let mut s = spec();
        s.program = r"C:\Users\Ada\.cua\envs\staging\host\bin\cua-spacesd.exe".into();
        s.log_path = r"C:\Users\Ada\.cua\envs\staging\host\driver.log".into();
        let (program, line) = windows_start_now(&s, r"C:\Windows");
        assert_eq!(program, r"C:\Windows\System32\cmd.exe");
        assert!(
            !line.contains("conhost") && !line.contains("--headless"),
            "{line}"
        );
        assert!(
            line.starts_with("/d /c set \"CUA_ENV_LOG=info\"&& "),
            "{line}"
        );
        assert!(line.ends_with(r">> C:\Users\Ada\.cua\envs\staging\host\driver.log 2>&1"));
        // The same command the logon entry runs at sign-in.
        let at_logon = windows_run_value(&s, r"C:\Windows");
        assert!(at_logon.ends_with(&line), "{at_logon}");
        #[cfg(windows)]
        {
            assert_eq!(START_NOW_FLAGS & 0x0000_0008, 0, "never DETACHED_PROCESS");
            assert_ne!(START_NOW_FLAGS & 0x0800_0000, 0, "CREATE_NO_WINDOW");
        }
    }

    #[test]
    fn a_refused_scheduled_task_is_recognized() {
        // What schtasks printed on a normal Windows 11 desktop (CUA-1702).
        let e = Error::Service(
            r#""schtasks" "/Create" "/TN" "Cua\SpacesdHost" "/XML" "C:\Users\a\.cua\host\task.xml" "/F" failed: ERROR: Access is denied."#.into(),
        );
        assert!(access_denied(&e));
        assert!(access_denied(&Error::Service(
            "FEHLER: (0x80070005)".into()
        )));
        assert!(!access_denied(&Error::Service(
            "ERROR: The task XML is malformed.".into()
        )));
    }

    #[test]
    fn windows_quoting_follows_the_crt_rules() {
        assert_eq!(windows_quote("plain"), "plain");
        assert_eq!(windows_quote("a b"), "\"a b\"");
        assert_eq!(windows_quote(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(
            windows_quote(r"C:\dir with space\"),
            r#""C:\dir with space\\""#
        );
    }

    #[test]
    fn runner_names_round_trip() {
        for k in [
            RunnerKind::Auto,
            RunnerKind::Systemd,
            RunnerKind::Launchd,
            RunnerKind::AppHelper,
            RunnerKind::WindowsTask,
            RunnerKind::Process,
        ] {
            assert_eq!(RunnerKind::parse(k.as_str()).unwrap(), k);
        }
        assert!(RunnerKind::parse("init.d").is_err());
        assert_ne!(RunnerKind::Auto.resolve(), RunnerKind::Auto);
    }

    #[test]
    fn the_app_helper_is_found_by_its_bundle_layout() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Cua Spaces.app");
        let stable = CuaChannel::Stable;
        assert!(!has_app_helper(&app, stable));
        for f in [app_helper_plist(stable).as_str(), APP_EXECUTABLE] {
            std::fs::create_dir_all(app.join(f).parent().unwrap()).unwrap();
            std::fs::write(app.join(f), "").unwrap();
        }
        assert!(
            !has_app_helper(&app, stable),
            "an app without its own cua-spacesd (0.9.4)"
        );
        std::fs::create_dir_all(app.join(APP_HELPER_DAEMON).parent().unwrap()).unwrap();
        std::fs::write(app.join(APP_HELPER_DAEMON), "").unwrap();
        assert!(has_app_helper(&app, stable));
        // An app before 0.15 kept it in Contents/MacOS.
        std::fs::remove_file(app.join(APP_HELPER_DAEMON)).unwrap();
        std::fs::write(app.join(APP_HELPER_DAEMON_BEFORE_0_15), "").unwrap();
        assert!(has_app_helper(&app, stable));
        let launcher = app_helper_launcher(&app);
        assert_eq!(launcher, app.join(APP_EXECUTABLE));
        assert!(is_app_helper_launcher(&launcher));
        assert!(!is_app_helper_launcher(Path::new(
            "/usr/local/bin/CuaSpacesMac"
        )));
        assert!(!is_app_helper_launcher(Path::new(
            "/Users/ada/.cua/host/bin/cua-spacesd"
        )));
        assert_eq!(
            helper_spec_path_for(stable, Path::new("/Users/ada")),
            Path::new("/Users/ada/.cua/helper/service.json")
        );
    }

    #[test]
    fn the_app_helper_daemon_is_outside_contents_macos() {
        // Any executable in Contents/MacOS is the app to LaunchServices once
        // it starts AppKit; the daemon there took over the app's launch.
        assert!(APP_HELPER_DAEMON.starts_with("Contents/Helpers/"));
        assert!(!APP_HELPER_DAEMON.starts_with("Contents/MacOS/"));
    }

    /// A Cua Spaces app of `channel` with the helper, in `parent`.
    fn fake_app(parent: &Path, channel: CuaChannel) -> PathBuf {
        let app = parent.join(format!("{}.app", channel.app_name()));
        for f in [
            app_helper_plist(channel).as_str(),
            APP_HELPER_DAEMON,
            APP_EXECUTABLE,
        ] {
            std::fs::create_dir_all(app.join(f).parent().unwrap()).unwrap();
            std::fs::write(app.join(f), "").unwrap();
        }
        app
    }

    #[test]
    fn stable_keeps_its_helper_names() {
        let stable = CuaChannel::Stable;
        assert_eq!(stable.helper_label(), "com.trycua.spaces.helper");
        assert_eq!(
            app_helper_plist(stable),
            "Contents/Library/LaunchAgents/com.trycua.spaces.helper.plist"
        );
        assert_eq!(
            helper_spec_path_for(stable, Path::new("/Users/ada")),
            Path::new("/Users/ada/.cua/helper/service.json")
        );
        assert_eq!(stable.host_label(), "com.trycua.spacesd.host");
        let user = Path::new("/Users/ada");
        assert_eq!(
            ServiceNames::for_home(stable, Path::new("/Users/ada/.cua"), user).launchd_label(),
            "com.trycua.spacesd.host"
        );
        let helper = AppHelper::for_channel(
            stable,
            PathBuf::from("/Applications/Cua Spaces.app"),
            Path::new("/Users/ada"),
        );
        assert_eq!(
            helper.target(),
            format!("gui/{}/com.trycua.spaces.helper", helper.uid)
        );
        assert_eq!(
            helper.spec_path,
            Path::new("/Users/ada/.cua/helper/service.json")
        );
    }

    #[test]
    fn local_has_its_own_helper_names() {
        let local = CuaChannel::Local;
        assert_eq!(local.helper_label(), "com.trycua.spaces.local.helper");
        assert_eq!(
            app_helper_plist(local),
            "Contents/Library/LaunchAgents/com.trycua.spaces.local.helper.plist"
        );
        assert_eq!(
            helper_spec_path_for(local, Path::new("/Users/ada")),
            Path::new("/Users/ada/.cua-local/helper/service.json")
        );
        // The plain runner: Local's own home adds no suffix; another home
        // still gets one, so two Local test homes stay apart.
        let user = Path::new("/Users/ada");
        let label =
            |home: &str| ServiceNames::for_home(local, Path::new(home), user).launchd_label();
        assert_eq!(
            label("/Users/ada/.cua-local"),
            "com.trycua.spacesd.host.local"
        );
        assert_eq!(
            label("/Users/ada/.cua-local/envs/prod"),
            "com.trycua.spacesd.host.local-prod"
        );
        let other = label("/tmp/h");
        assert!(
            other.starts_with("com.trycua.spacesd.host.local-"),
            "{other}"
        );
        // Stable's home is not Local's: it is a separate home to Local.
        assert!(label("/Users/ada/.cua").starts_with("com.trycua.spacesd.host.local-"));
        let helper = AppHelper::for_channel(
            local,
            PathBuf::from("/Users/ada/Applications/Cua Spaces Local.app"),
            Path::new("/Users/ada"),
        );
        assert_eq!(
            helper.target(),
            format!("gui/{}/com.trycua.spaces.local.helper", helper.uid)
        );
        assert_eq!(
            helper.spec_path,
            Path::new("/Users/ada/.cua-local/helper/service.json")
        );
        // Nothing of Local's is Stable's.
        for (l, s) in [
            (local.helper_label(), CuaChannel::Stable.helper_label()),
            (local.host_label(), CuaChannel::Stable.host_label()),
        ] {
            assert_ne!(l, s);
        }
        assert_ne!(
            helper_spec_path_for(local, Path::new("/Users/ada")),
            helper_spec_path_for(CuaChannel::Stable, Path::new("/Users/ada"))
        );
    }

    #[test]
    fn an_app_of_the_other_channel_is_never_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let stable_app = fake_app(dir.path(), CuaChannel::Stable);
        let local_app = fake_app(dir.path(), CuaChannel::Local);
        assert_ne!(stable_app, local_app);

        // Each app is the helper of its own channel only.
        assert!(has_app_helper(&stable_app, CuaChannel::Stable));
        assert!(has_app_helper(&local_app, CuaChannel::Local));
        assert!(!has_app_helper(&stable_app, CuaChannel::Local));
        assert!(!has_app_helper(&local_app, CuaChannel::Stable));

        // Both installed: each channel finds its own, whatever the order.
        let both = vec![stable_app.clone(), local_app.clone()];
        let both_rev = vec![local_app.clone(), stable_app.clone()];
        for found in [both, both_rev] {
            assert_eq!(
                pick_app_helper(CuaChannel::Stable, None, found.clone()),
                Some(stable_app.clone())
            );
            assert_eq!(
                pick_app_helper(CuaChannel::Local, None, found),
                Some(local_app.clone())
            );
        }
        // Only the other channel's app installed: no helper.
        assert_eq!(
            pick_app_helper(CuaChannel::Local, None, vec![stable_app.clone()]),
            None
        );
        assert_eq!(
            pick_app_helper(CuaChannel::Stable, None, vec![local_app.clone()]),
            None
        );

        // `CUA_SPACES_APP` cannot point a channel at the other one's app.
        let spaces_app = |p: &Path| Some(p.as_os_str().to_owned());
        assert_eq!(
            pick_app_helper(
                CuaChannel::Local,
                spaces_app(&stable_app).as_deref(),
                vec![]
            ),
            None
        );
        assert_eq!(
            pick_app_helper(
                CuaChannel::Stable,
                spaces_app(&local_app).as_deref(),
                vec![]
            ),
            None
        );
        assert_eq!(
            pick_app_helper(CuaChannel::Local, spaces_app(&local_app).as_deref(), vec![]),
            Some(local_app.clone())
        );
        // `none` and empty still mean no app, whatever is installed.
        for off in ["none", ""] {
            assert_eq!(
                pick_app_helper(
                    CuaChannel::Stable,
                    Some(std::ffi::OsStr::new(off)),
                    vec![stable_app.clone()]
                ),
                None
            );
        }
    }

    #[test]
    fn each_channel_looks_only_for_its_own_app() {
        let home = Some(std::ffi::OsStr::new("/Users/ada"));
        let names = |channel| -> Vec<String> {
            app_helper_candidates(channel, None, home)
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert!(
            names(CuaChannel::Stable)
                .iter()
                .all(|n| n == "Cua Spaces.app")
        );
        assert!(
            names(CuaChannel::Local)
                .iter()
                .all(|n| n == "Cua Spaces Local.app")
        );
        assert_eq!(
            app_helper_candidates(CuaChannel::Stable, None, home)[0],
            Path::new("/Applications/Cua Spaces.app")
        );
        // The app this process runs from comes first.
        let exe = Path::new("/Users/ada/Applications/Cua Spaces Local.app/Contents/MacOS/cua");
        let from = app_containing(exe);
        assert_eq!(
            from.as_deref(),
            Some(Path::new("/Users/ada/Applications/Cua Spaces Local.app"))
        );
        assert_eq!(
            app_helper_candidates(CuaChannel::Local, from.clone(), home)[0],
            from.unwrap()
        );
        assert_eq!(app_containing(Path::new("/usr/local/bin/cua")), None);
    }

    #[test]
    fn a_helper_registration_says_what_to_do_in_words() {
        let reg = |status: &str, error: Option<&str>| HelperRegistration {
            status: status.into(),
            error: error.map(Into::into),
        };
        assert!(helper_registered(&reg("enabled", None), "Cua Spaces").is_ok());
        let e = helper_registered(&reg("requires-approval", None), "Cua Spaces")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Login Items"), "{e}");
        let e = helper_registered(&reg("not-found", Some("Invalid argument")), "Cua Spaces")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("Cua Spaces") && e.contains("Invalid argument"),
            "{e}"
        );
        // The JSON CuaSpacesMac --service-helper prints.
        let r: HelperRegistration = serde_json::from_str(
            r#"{"error":"Operation not permitted","status":"not-registered"}"#,
        )
        .unwrap();
        assert_eq!(r.status, "not-registered");
    }

    #[test]
    fn without_the_app_the_helper_runner_reports_and_refuses_in_words() {
        let m = MissingAppHelper;
        assert!(!m.state().installed && !m.state().running);
        assert_eq!(m.state().kind, "app-helper");
        assert!(
            m.start()
                .unwrap_err()
                .to_string()
                .contains("--runner launchd")
        );
        assert!(m.uninstall().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn process_runner_starts_reports_and_stops_a_harmless_process() {
        // Runs `sleep` from a temp dir: no host effects.
        let dir = tempfile::tempdir().unwrap();
        let runner = ProcessRunner {
            dir: dir.path().join("host"),
        };
        assert!(!runner.state().installed);
        runner
            .install(&ServiceSpec {
                program: "/bin/sleep".into(),
                args: vec!["30".into()],
                env: vec![],
                log_path: dir.path().join("host/driver.log"),
                working_dir: dir.path().into(),
            })
            .unwrap();
        runner.start().unwrap();
        let state = runner.state();
        assert!(state.installed && state.running, "{state:?}");
        runner.uninstall().unwrap();
        let state = runner.state();
        assert!(!state.installed && !state.running, "{state:?}");
    }

    /// What the driver starts in its process group (the host's cua
    /// daemon, which outlives the `cua daemon start` that spawned it) stops
    /// with it (CUA-1804).
    #[cfg(unix)]
    #[test]
    fn process_runner_stops_what_the_driver_left_running_in_its_group() {
        let dir = tempfile::tempdir().unwrap();
        let child_pid = dir.path().join("child.pid");
        let runner = ProcessRunner {
            dir: dir.path().join("host"),
        };
        // A subshell starts a background `sleep` and exits, as `cua daemon
        // start` does: the sleep is orphaned, still in the driver's group.
        let script = format!(
            "(sleep 300 & echo $! > '{}'); exec sleep 300",
            child_pid.display()
        );
        runner
            .install(&ServiceSpec {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script],
                env: vec![],
                log_path: dir.path().join("host/driver.log"),
                working_dir: dir.path().into(),
            })
            .unwrap();
        runner.start().unwrap();
        let mut pid = None;
        for _ in 0..50 {
            pid = std::fs::read_to_string(&child_pid)
                .ok()
                .and_then(|p| p.trim().parse::<u32>().ok());
            if pid.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let pid = pid.expect("the background child started");
        assert!(process_alive(pid));
        runner.uninstall().unwrap();
        // The orphan is reaped by init (launchd) a moment after it dies.
        let mut alive = true;
        for _ in 0..50 {
            alive = process_alive(pid);
            if !alive {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if alive {
            // Never leave it behind, whatever the assertion says.
            // SAFETY: the pid this test started.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
        assert!(!alive, "the driver's background child outlived uninstall");
    }
}

#[cfg(test)]
mod instance_tests {
    use super::*;
    use cua_home::channel::CuaChannel::{Local, Stable};

    fn names(channel: CuaChannel, home: &str) -> ServiceNames {
        ServiceNames::for_home(channel, Path::new(home), Path::new("/home/ada"))
    }

    #[test]
    fn production_keeps_the_names_it_always_had() {
        let n = names(Stable, "/home/ada/.cua");
        assert_eq!(n.suffix, "");
        assert!(n.is_default());
        assert!(!n.other_env());
        assert_eq!(n.systemd_unit(), "cua-spacesd-host");
        assert_eq!(n.windows_task(), r"Cua\SpacesdHost");
        assert_eq!(n.windows_run_value(), "CuaSpacesdHost");
        assert_eq!(n.launchd_label(), "com.trycua.spacesd.host");
        // Nothing to retire: the old names are its own.
        let l = n.legacy();
        assert!(l.systemd.is_empty() && l.windows_task.is_empty() && l.windows_run.is_empty());
        assert!(l.launchd.is_empty());
    }

    #[test]
    fn staging_has_names_of_its_own_on_every_os() {
        let prod = names(Stable, "/home/ada/.cua");
        let staging = names(Stable, "/home/ada/.cua/envs/staging");
        assert_eq!(staging.suffix, "-staging");
        assert!(staging.other_env());
        assert_eq!(staging.systemd_unit(), "cua-spacesd-host-staging");
        assert_eq!(staging.windows_task(), r"Cua\SpacesdHost-staging");
        assert_eq!(staging.windows_run_value(), "CuaSpacesdHost-staging");
        assert_eq!(staging.launchd_label(), "com.trycua.spacesd.host-staging");
        assert_ne!(staging.systemd_unit(), prod.systemd_unit());
        assert_ne!(staging.windows_task(), prod.windows_task());
        assert_ne!(staging.windows_run_value(), prod.windows_run_value());
        assert_ne!(staging.launchd_label(), prod.launchd_label());
    }

    #[test]
    fn every_home_has_its_own_names() {
        let homes = [
            (Stable, "/home/ada/.cua"),
            (Stable, "/home/ada/.cua/envs/staging"),
            (Local, "/home/ada/.cua-local"),
            (Local, "/home/ada/.cua-local/envs/prod"),
            (Stable, "/home/ada/fb/home"),
            (Stable, "/home/ada/other"),
            (Local, "/home/ada/.cua"),
        ];
        let all: Vec<ServiceNames> = homes.iter().map(|(c, h)| names(*c, h)).collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.systemd_unit(), b.systemd_unit(), "{a:?} {b:?}");
                assert_ne!(a.windows_task(), b.windows_task(), "{a:?} {b:?}");
                assert_ne!(a.windows_run_value(), b.windows_run_value(), "{a:?} {b:?}");
                assert_ne!(a.launchd_label(), b.launchd_label(), "{a:?} {b:?}");
            }
        }
        assert_eq!(
            names(Local, "/home/ada/.cua-local").systemd_unit(),
            "cua-spacesd-host-local"
        );
        assert_eq!(
            names(Local, "/home/ada/.cua-local/envs/prod").windows_task(),
            r"Cua\SpacesdHost-local-prod"
        );
        // A separate home keeps the hash builds before CUA-3512 gave it.
        let custom = names(Stable, "/home/ada/fb/home");
        assert_eq!(custom.suffix.len(), 9);
        assert_eq!(
            custom.suffix,
            format!("-{}", home_hash(Path::new("/home/ada/fb/home")))
        );
        assert!(!custom.other_env());
        assert!(
            custom
                .legacy()
                .systemd
                .iter()
                .all(|n| n != &custom.systemd_unit())
        );
    }

    #[test]
    fn staging_retires_its_old_names_only_when_they_run_it() {
        let staging = names(Stable, "/home/ada/.cua/envs/staging");
        let l = staging.legacy();
        let hash = home_hash(Path::new("/home/ada/.cua/envs/staging"));
        assert_eq!(
            l.systemd,
            vec![
                "cua-spacesd-host".to_string(),
                format!("cua-spacesd-host-{hash}")
            ]
        );
        assert_eq!(l.windows_task, vec![r"Cua\SpacesdHost".to_string()]);
        assert_eq!(l.windows_run, vec!["CuaSpacesdHost".to_string()]);
        // Production's unit never counts as staging's, although staging's
        // home is inside production's.
        let prod_unit = systemd_unit(
            &ServiceSpec {
                program: "/home/ada/.cua/host/bin/cua-spacesd".into(),
                log_path: "/home/ada/.cua/host/driver.log".into(),
                ..Default::default()
            },
            false,
        );
        assert!(!staging.owns(&prod_unit));
        assert!(names(Stable, "/home/ada/.cua").owns(&prod_unit));
        let staging_unit = systemd_unit(
            &ServiceSpec {
                program: "/home/ada/.cua/envs/staging/host/bin/cua-spacesd".into(),
                log_path: "/home/ada/.cua/envs/staging/host/driver.log".into(),
                ..Default::default()
            },
            false,
        );
        assert!(staging.owns(&staging_unit));
        assert!(!names(Stable, "/home/ada/.cua").owns(&staging_unit));
    }

    #[test]
    fn windows_ownership_reads_the_task_and_the_logon_entry() {
        let home =
            |h: &str| ServiceNames::for_home(Stable, Path::new(h), Path::new(r"C:\Users\Ada"));
        let prod = home(r"C:\Users\Ada\.cua");
        let staging = home(r"C:\Users\Ada\.cua\envs\staging");
        if cfg!(windows) {
            assert_eq!(staging.suffix, "-staging");
        }
        let spec = |dir: &str| ServiceSpec {
            program: format!(r"{dir}\bin\cua-spacesd.exe").into(),
            log_path: format!(r"{dir}\driver.log").into(),
            ..Default::default()
        };
        let prod_spec = spec(r"C:\Users\Ada\.cua\host");
        let staging_spec = spec(r"C:\Users\Ada\.cua\envs\staging\host");
        if cfg!(windows) {
            let prod_xml = windows_task_xml(&prod_spec, r"PC\Ada", &prod.windows_task());
            let prod_run = windows_run_value(&prod_spec, r"C:\Windows");
            assert!(prod.owns(&prod_xml) && prod.owns(&prod_run));
            assert!(!staging.owns(&prod_xml) && !staging.owns(&prod_run));
            let staging_xml = windows_task_xml(&staging_spec, r"PC\Ada", &staging.windows_task());
            assert!(staging.owns(&staging_xml) && !prod.owns(&staging_xml));
            assert!(staging_xml.contains(r"<URI>\Cua\SpacesdHost-staging</URI>"));
            // `envs/staging` joins with a forward slash; Windows writes and
            // reports backslashes, in any case.
            let joined = home(r"C:\Users\Ada\.cua\envs/staging");
            assert!(joined.owns(r"C:\USERS\ada\.cua\envs\staging\host\driver.log"));
            assert!(!joined.owns(r"C:\Users\Ada\.cua\host\driver.log"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_staging_setup_never_touches_the_production_unit() {
        // CUA-3512: a real Systemd runner (with a stand-in systemctl that
        // records its calls) sets staging up next to production's unit.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path();
        let unit_dir = user.join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir).unwrap();
        let calls = user.join("calls.log");
        let systemctl = user.join("systemctl");
        std::fs::write(
            &systemctl,
            format!("#!/bin/sh\necho \"$*\" >> '{}'\n", calls.display()),
        )
        .unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = |home: &Path| Systemd {
            system: false,
            unit_dir: unit_dir.clone(),
            names: ServiceNames::for_home(Stable, home, user),
            systemctl: systemctl.clone(),
        };
        let spec_of = |home: &Path| ServiceSpec {
            program: home.join("host/bin/cua-spacesd"),
            args: vec!["join".into()],
            log_path: home.join("host/driver.log"),
            ..Default::default()
        };
        let prod_home = user.join(".cua");
        let staging_home = user.join(".cua/envs/staging");
        let prod = runner(&prod_home);
        prod.install(&spec_of(&prod_home)).unwrap();
        let prod_unit = std::fs::read_to_string(unit_dir.join("cua-spacesd-host.service")).unwrap();
        std::fs::write(&calls, "").unwrap();

        let staging = runner(&staging_home);
        assert!(staging.retire_legacy_names().is_empty());
        staging.install(&spec_of(&staging_home)).unwrap();
        staging.start().unwrap();
        staging.stop().unwrap();
        staging.uninstall().unwrap();

        assert_eq!(
            std::fs::read_to_string(unit_dir.join("cua-spacesd-host.service")).unwrap(),
            prod_unit,
            "production's unit was rewritten"
        );
        let log = std::fs::read_to_string(&calls).unwrap();
        for line in log.lines() {
            assert!(
                !line.split_whitespace().any(|w| w == "cua-spacesd-host"),
                "staging ran `systemctl {line}` on production's unit"
            );
        }
        assert!(log.contains("enable cua-spacesd-host-staging"), "{log}");
        assert!(log.contains("restart cua-spacesd-host-staging"), "{log}");
        assert!(!unit_dir.join("cua-spacesd-host-staging.service").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staging_moves_off_the_production_unit_it_had_taken() {
        // A build before CUA-3512 wrote staging's command into
        // production's unit; the next staging setup retires it from there
        // (and leaves a production unit alone).
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path();
        let unit_dir = user.join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir).unwrap();
        let systemctl = user.join("systemctl");
        std::fs::write(&systemctl, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let staging_home = user.join(".cua/envs/staging");
        let staging = Systemd {
            system: false,
            unit_dir: unit_dir.clone(),
            names: ServiceNames::for_home(Stable, &staging_home, user),
            systemctl: systemctl.clone(),
        };
        let staging_spec = ServiceSpec {
            program: staging_home.join("host/bin/cua-spacesd"),
            log_path: staging_home.join("host/driver.log"),
            ..Default::default()
        };
        let hashed = format!("cua-spacesd-host-{}", home_hash(&staging_home));
        for name in ["cua-spacesd-host", hashed.as_str()] {
            std::fs::write(
                unit_dir.join(format!("{name}.service")),
                systemd_unit(&staging_spec, false),
            )
            .unwrap();
        }
        let retired = staging.retire_legacy_names();
        assert_eq!(
            retired,
            vec!["cua-spacesd-host".to_string(), hashed.clone()]
        );
        assert!(!unit_dir.join("cua-spacesd-host.service").exists());
        assert!(!unit_dir.join(format!("{hashed}.service")).exists());

        // Production's own unit stays where it is.
        let prod_home = user.join(".cua");
        let prod_spec = ServiceSpec {
            program: prod_home.join("host/bin/cua-spacesd"),
            log_path: prod_home.join("host/driver.log"),
            ..Default::default()
        };
        std::fs::write(
            unit_dir.join("cua-spacesd-host.service"),
            systemd_unit(&prod_spec, false),
        )
        .unwrap();
        assert!(staging.retire_legacy_names().is_empty());
        assert!(unit_dir.join("cua-spacesd-host.service").exists());
    }
}
