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
//!   opens anything.
//! - Windows: a scheduled task that starts at logon with the user's
//!   interactive token, which is the interactive-session helper: a Windows
//!   service lives in session 0 and cannot capture the desktop or inject
//!   input, so the task (not a service) runs the driver in the user session.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Service name / label used everywhere.
pub const SERVICE_NAME: &str = "cua-spacesd-host";
/// launchd label.
pub const LAUNCHD_LABEL: &str = "com.trycua.spacesd.host";
/// Windows task name.
pub const WINDOWS_TASK_NAME: &str = "Cua\\SpacesdHost";

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
    /// Windows scheduled task at logon (interactive token).
    WindowsTask,
    /// A detached process with a pidfile (no init system, e.g. containers).
    Process,
}

impl RunnerKind {
    /// The runner for this machine: launchd on macOS, the scheduled task on
    /// Windows, systemd on Linux when it is PID 1, else a plain process.
    pub fn detect() -> Self {
        if cfg!(target_os = "macos") {
            RunnerKind::Launchd
        } else if cfg!(windows) {
            RunnerKind::WindowsTask
        } else if Path::new("/run/systemd/system").is_dir() {
            RunnerKind::Systemd
        } else {
            RunnerKind::Process
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
    /// `systemd`, `launchd`, `windows-task` or `process`.
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
         # Unattended access to this machine through cua-spacesd.\n\
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
pub fn launchd_plist(spec: &ServiceSpec) -> String {
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
        label = LAUNCHD_LABEL,
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

/// The Task Scheduler XML: at logon of `user` (DOMAIN\\name, or empty for
/// any user), interactive token, restart on failure, no time limit.
/// Environment variables are passed as `--env`-free arguments: the task
/// runs `cmd.exe /d /c set K=V&& … && driver args` only when `env` is
/// non-empty, since tasks have no environment block.
pub fn windows_task_xml(spec: &ServiceSpec, user: &str) -> String {
    let driver = std::iter::once(windows_quote(&spec.program.to_string_lossy()))
        .chain(spec.args.iter().map(|a| windows_quote(a)))
        .collect::<Vec<_>>()
        .join(" ");
    let log = windows_quote(&spec.log_path.to_string_lossy());
    let mut chain = String::new();
    for (k, v) in &spec.env {
        chain.push_str(&format!("set \"{k}={v}\"&& "));
    }
    let (command, arguments) = (
        "%SystemRoot%\\System32\\cmd.exe".to_string(),
        format!("/d /c {chain}{driver} >> {log} 2>&1"),
    );
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
        name = xml_escape(WINDOWS_TASK_NAME),
        command = xml_escape(&command),
        arguments = xml_escape(&arguments),
        wd = xml_escape(&spec.working_dir.to_string_lossy()),
    )
}

// ---------------------------------------------------------------- runners

fn run(cmd: &mut Command) -> Result<String> {
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
}

impl Systemd {
    /// A system unit when running as root, else a user unit under `home`.
    pub fn for_current_user(user_home: &Path) -> Self {
        if is_root() {
            Self {
                system: true,
                unit_dir: PathBuf::from("/etc/systemd/system"),
            }
        } else {
            Self {
                system: false,
                unit_dir: user_home.join(".config/systemd/user"),
            }
        }
    }

    fn unit_path(&self) -> PathBuf {
        self.unit_dir.join(format!("{SERVICE_NAME}.service"))
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
        let mut c = Command::new("systemctl");
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
        run(self.systemctl().args(["enable", SERVICE_NAME]))?;
        Ok(())
    }
    fn start(&self) -> Result<()> {
        run(self.systemctl().args(["restart", SERVICE_NAME])).map(drop)
    }
    fn stop(&self) -> Result<()> {
        run(self.systemctl().args(["stop", SERVICE_NAME])).map(drop)
    }
    fn uninstall(&self) -> Result<()> {
        let _ = run(self.systemctl().args(["disable", "--now", SERVICE_NAME]));
        let _ = std::fs::remove_file(self.unit_path());
        self.remove_legacy();
        let _ = run(self.systemctl().arg("daemon-reload"));
        Ok(())
    }
    fn state(&self) -> ServiceState {
        let installed = self.unit_path().exists();
        let active = self
            .systemctl()
            .args(["is-active", SERVICE_NAME])
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
}

/// launchd LaunchAgent (GUI domain of the current user).
pub struct Launchd {
    /// `~/Library/LaunchAgents`.
    pub agents_dir: PathBuf,
    /// The user id (`gui/<uid>`).
    pub uid: u32,
}

impl Launchd {
    /// The current user's agents directory and uid.
    pub fn for_current_user(user_home: &Path) -> Self {
        #[cfg(unix)]
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        #[cfg(not(unix))]
        let uid = 0;
        Self {
            agents_dir: user_home.join("Library/LaunchAgents"),
            uid,
        }
    }

    fn plist_path(&self) -> PathBuf {
        self.agents_dir.join(format!("{LAUNCHD_LABEL}.plist"))
    }

    fn target(&self) -> String {
        format!("gui/{}/{LAUNCHD_LABEL}", self.uid)
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
        write_file(&self.plist_path(), &launchd_plist(spec))
    }
    fn start(&self) -> Result<()> {
        let _ = run(Command::new("launchctl").args(["bootout", &self.target()]));
        run(Command::new("launchctl").args([
            "bootstrap",
            &format!("gui/{}", self.uid),
            &self.plist_path().to_string_lossy(),
        ]))
        .map(drop)
    }
    fn stop(&self) -> Result<()> {
        run(Command::new("launchctl").args(["bootout", &self.target()])).map(drop)
    }
    fn uninstall(&self) -> Result<()> {
        let _ = run(Command::new("launchctl").args(["bootout", &self.target()]));
        let _ = std::fs::remove_file(self.plist_path());
        self.remove_legacy();
        Ok(())
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

/// Windows scheduled task at logon.
pub struct WindowsTask {
    /// Where the task XML is written before `schtasks /Create /XML`.
    pub xml_path: PathBuf,
    /// `DOMAIN\\user` the logon trigger fires for ("" = any user).
    pub user: String,
}

impl WindowsTask {
    /// The current user (`USERDOMAIN\\USERNAME`).
    pub fn for_current_user(host_dir: &Path) -> Self {
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(d), Ok(u)) if !d.is_empty() && !u.is_empty() => format!("{d}\\{u}"),
            (_, Ok(u)) => u,
            _ => String::new(),
        };
        Self {
            xml_path: host_dir.join("task.xml"),
            user,
        }
    }
}

impl ServiceManager for WindowsTask {
    fn kind(&self) -> RunnerKind {
        RunnerKind::WindowsTask
    }
    fn install(&self, spec: &ServiceSpec) -> Result<()> {
        // schtasks reads UTF-16 (the header says so) but also accepts UTF-8
        // without a BOM when the declaration matches; write UTF-16LE + BOM.
        let xml = windows_task_xml(spec, &self.user);
        let mut bytes = vec![0xFF, 0xFE];
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        if let Some(parent) = self.xml_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.xml_path, bytes)?;
        remove_legacy_task();
        run(Command::new("schtasks").args([
            "/Create",
            "/TN",
            WINDOWS_TASK_NAME,
            "/XML",
            &self.xml_path.to_string_lossy(),
            "/F",
        ]))
        .map(drop)
    }
    fn start(&self) -> Result<()> {
        let _ = run(Command::new("schtasks").args(["/End", "/TN", WINDOWS_TASK_NAME]));
        run(Command::new("schtasks").args(["/Run", "/TN", WINDOWS_TASK_NAME])).map(drop)
    }
    fn stop(&self) -> Result<()> {
        run(Command::new("schtasks").args(["/End", "/TN", WINDOWS_TASK_NAME])).map(drop)
    }
    fn uninstall(&self) -> Result<()> {
        let _ = run(Command::new("schtasks").args(["/End", "/TN", WINDOWS_TASK_NAME]));
        let _ = run(Command::new("schtasks").args(["/Delete", "/TN", WINDOWS_TASK_NAME, "/F"]));
        let _ = std::fs::remove_file(&self.xml_path);
        remove_legacy_task();
        Ok(())
    }
    fn state(&self) -> ServiceState {
        let out = Command::new("schtasks")
            .args(["/Query", "/TN", WINDOWS_TASK_NAME, "/FO", "CSV", "/NH"])
            .output();
        let (installed, text) = match out {
            Ok(o) if o.status.success() => (true, String::from_utf8_lossy(&o.stdout).to_string()),
            _ => (false, String::new()),
        };
        ServiceState {
            installed,
            running: text.contains("Running"),
            kind: "windows-task".into(),
            detail: text.trim().to_string(),
        }
    }
}

/// A detached process with a pidfile: the fallback where no init system
/// runs (containers). Not restarted on reboot or crash.
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
            // SAFETY: plain signal delivery to the pid we started.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            for _ in 0..50 {
                if !process_alive(pid) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if process_alive(pid) {
                // SAFETY: as above.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
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
}

/// Records calls; for tests of everything above the runner.
#[derive(Default)]
pub struct FakeServiceManager {
    /// `install`, `start`, `stop`, `uninstall` in order.
    pub calls: Mutex<Vec<String>>,
    /// The last installed spec.
    pub spec: Mutex<Option<ServiceSpec>>,
    running: Mutex<bool>,
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
    fn state(&self) -> ServiceState {
        ServiceState {
            installed: self.spec.lock().unwrap().is_some(),
            running: *self.running.lock().unwrap(),
            kind: "fake".into(),
            detail: String::new(),
        }
    }
}

/// The manager for `kind` (resolving `Auto`), for the current user.
pub fn manager_for(kind: RunnerKind, user_home: &Path, host_dir: &Path) -> Box<dyn ServiceManager> {
    match kind.resolve() {
        RunnerKind::Systemd => Box::new(Systemd::for_current_user(user_home)),
        RunnerKind::Launchd => Box::new(Launchd::for_current_user(user_home)),
        RunnerKind::WindowsTask => Box::new(WindowsTask::for_current_user(host_dir)),
        RunnerKind::Process | RunnerKind::Auto => Box::new(ProcessRunner {
            dir: host_dir.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let p = launchd_plist(&s);
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
        let x = windows_task_xml(&s, r"PC\Ada");
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
        let any = windows_task_xml(&s, "");
        assert!(!any.contains("<UserId>"));
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
            RunnerKind::WindowsTask,
            RunnerKind::Process,
        ] {
            assert_eq!(RunnerKind::parse(k.as_str()).unwrap(), k);
        }
        assert!(RunnerKind::parse("init.d").is_err());
        assert_ne!(RunnerKind::Auto.resolve(), RunnerKind::Auto);
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
}
