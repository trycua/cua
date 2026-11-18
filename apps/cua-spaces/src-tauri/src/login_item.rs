// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Launch at login (Settings, General; the first run's Done checkbox): the
//! app registers itself with the system's own mechanism and reads back what
//! the system holds, so the toggle never shows a state the system does not
//! have.
//!
//! - Linux: an XDG autostart entry, `$XDG_CONFIG_HOME/autostart` (default
//!   `~/.config/autostart`), the AppImage's path when run from one.
//! - Windows: the current user's `Run` key, through `reg.exe`; a startup
//!   entry turned off in Task Manager (`StartupApproved`) reads as off.
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents` (the SwiftUI app,
//!   macOS's primary app, uses `SMAppService` instead).
//!
//! Each starts the app with [`AUTOSTART_ARG`], so a launch at login opens
//! the menu bar item and the notch without the main window. The app's own
//! daemon starts with the app, so the Spaces this machine provides, its
//! persistent agents and Cua Volume come back after a restart.
//!
//! The services take every path (and, on Windows, the `reg` runner) from
//! their caller, so tests run on temporary directories and a fake registry
//! and never touch the user's login items.

use std::path::{Path, PathBuf};

use cua_spaces_app_core::login_item::LoginItemStatus;

/// The argument a launch at login passes: start without the main window.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Whether this process was started at login.
pub fn launched_at_login(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().skip(1).any(|a| a == AUTOSTART_ARG)
}

/// The app as a login item.
pub trait LoginItemService: Send + Sync {
    /// What the system holds now.
    fn status(&self) -> LoginItemStatus;
    /// Registers (`on`) or unregisters the app.
    fn set(&self, on: bool) -> Result<(), String>;
}

/// Sets `on` and reads back what the system holds.
pub fn set_and_read(service: &dyn LoginItemService, on: bool) -> Result<LoginItemStatus, String> {
    service.set(on)?;
    Ok(service.status())
}

/// The entry's name (the autostart file, the LaunchAgent label, the `Run`
/// value).
pub const ENTRY_ID: &str = "com.trycua.spaces";
/// The `Run` value's name.
pub const RUN_VALUE: &str = "Cua Spaces";

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

// ---------------------------------------------------------------- Linux

/// An XDG autostart entry.
pub struct XdgAutostart {
    /// The `.desktop` file.
    pub file: PathBuf,
    /// The program to start (`None`: unknown, so not available).
    pub exec: Option<PathBuf>,
}

impl XdgAutostart {
    /// The entry in `config_home/autostart`.
    pub fn new(config_home: &Path, exec: Option<PathBuf>) -> Self {
        Self {
            file: config_home
                .join("autostart")
                .join(format!("{ENTRY_ID}.desktop")),
            exec,
        }
    }

    /// The file's text.
    pub fn entry(exec: &Path) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Cua Spaces\nComment=Keeps your Spaces, agents and Cua Volume available after a restart.\nExec={} {AUTOSTART_ARG}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            desktop_quote(&exec.to_string_lossy())
        )
    }
}

/// An `Exec` argument, quoted as the Desktop Entry spec asks.
pub fn desktop_quote(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // A literal percent sign is doubled (field codes).
    out.replace('%', "%%")
}

impl LoginItemService for XdgAutostart {
    fn status(&self) -> LoginItemStatus {
        if self.exec.is_none() {
            return LoginItemStatus::NotFound;
        }
        match std::fs::read_to_string(&self.file) {
            // `Hidden=true` or a disabled GNOME entry is off.
            Ok(text)
                if !text.lines().any(|l| {
                    let l = l.trim();
                    l == "Hidden=true" || l == "X-GNOME-Autostart-enabled=false"
                }) =>
            {
                LoginItemStatus::Enabled
            }
            _ => LoginItemStatus::NotRegistered,
        }
    }

    fn set(&self, on: bool) -> Result<(), String> {
        if !on {
            return remove(&self.file);
        }
        let exec = self
            .exec
            .as_deref()
            .ok_or("The app's location is unknown.")?;
        write_atomic(&self.file, &Self::entry(exec))
    }
}

// ---------------------------------------------------------------- macOS

/// A per-user LaunchAgent that opens the app at login.
pub struct LaunchAgent {
    /// The `.plist` file.
    pub file: PathBuf,
    /// The program to start (`None`: unknown, so not available).
    pub exec: Option<PathBuf>,
}

impl LaunchAgent {
    /// The agent in `home/Library/LaunchAgents`.
    pub fn new(home: &Path, exec: Option<PathBuf>) -> Self {
        Self {
            file: home
                .join("Library/LaunchAgents")
                .join(format!("{ENTRY_ID}.plist")),
            exec,
        }
    }

    /// The file's text.
    pub fn plist(exec: &Path) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{ENTRY_ID}</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>{AUTOSTART_ARG}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n  <key>ProcessType</key>\n  <string>Interactive</string>\n</dict>\n</plist>\n",
            xml_escape(&exec.to_string_lossy())
        )
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

impl LoginItemService for LaunchAgent {
    fn status(&self) -> LoginItemStatus {
        if self.exec.is_none() {
            return LoginItemStatus::NotFound;
        }
        if self.file.is_file() {
            LoginItemStatus::Enabled
        } else {
            LoginItemStatus::NotRegistered
        }
    }

    fn set(&self, on: bool) -> Result<(), String> {
        if !on {
            return remove(&self.file);
        }
        let exec = self
            .exec
            .as_deref()
            .ok_or("The app's location is unknown.")?;
        write_atomic(&self.file, &Self::plist(exec))
    }
}

// ---------------------------------------------------------------- Windows

/// Runs `reg.exe` with the given arguments: its stdout when it succeeded.
pub trait Reg: Send + Sync {
    /// `reg <args>`.
    fn run(&self, args: &[String]) -> Option<String>;
}

/// The real `reg.exe`, without a console window.
pub struct RegExe;

impl Reg for RegExe {
    fn run(&self, args: &[String]) -> Option<String> {
        let mut c = std::process::Command::new("reg");
        c.args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        let out = c.output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// The current user's `Run` key.
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager records a startup entry turned off.
pub const APPROVED_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// A `Run` value that starts the app at sign-in.
pub struct RunKey<R: Reg> {
    /// `reg.exe` (a fake in tests).
    pub reg: R,
    /// The program to start (`None`: unknown, so not available).
    pub exec: Option<PathBuf>,
}

impl<R: Reg> RunKey<R> {
    /// The value's data.
    pub fn command(exec: &Path) -> String {
        format!("\"{}\" {AUTOSTART_ARG}", exec.to_string_lossy())
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    /// Task Manager turned it off: the `StartupApproved` value's first byte
    /// is odd (`03 00 ...`).
    fn turned_off(&self) -> bool {
        let Some(out) = self
            .reg
            .run(&Self::args(&["query", APPROVED_KEY, "/v", RUN_VALUE]))
        else {
            return false;
        };
        out.split_whitespace()
            .skip_while(|t| *t != "REG_BINARY")
            .nth(1)
            .and_then(|hex| u8::from_str_radix(hex.get(..2)?, 16).ok())
            .is_some_and(|b| b & 1 == 1)
    }
}

impl<R: Reg> LoginItemService for RunKey<R> {
    fn status(&self) -> LoginItemStatus {
        if self.exec.is_none() {
            return LoginItemStatus::NotFound;
        }
        let present = self
            .reg
            .run(&Self::args(&["query", RUN_KEY, "/v", RUN_VALUE]))
            .is_some();
        if present && !self.turned_off() {
            LoginItemStatus::Enabled
        } else {
            LoginItemStatus::NotRegistered
        }
    }

    fn set(&self, on: bool) -> Result<(), String> {
        if !on {
            // Absent already is fine.
            if self
                .reg
                .run(&Self::args(&["query", RUN_KEY, "/v", RUN_VALUE]))
                .is_some()
                && self
                    .reg
                    .run(&Self::args(&["delete", RUN_KEY, "/v", RUN_VALUE, "/f"]))
                    .is_none()
            {
                return Err("Could not remove the startup entry.".into());
            }
            return Ok(());
        }
        let exec = self
            .exec
            .as_deref()
            .ok_or("The app's location is unknown.")?;
        let data = Self::command(exec);
        self.reg
            .run(&Self::args(&[
                "add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &data, "/f",
            ]))
            .ok_or("Could not add the startup entry.")?;
        // Turning it on here turns it back on in Task Manager too.
        if self.turned_off() {
            let _ = self.reg.run(&Self::args(&[
                "delete",
                APPROVED_KEY,
                "/v",
                RUN_VALUE,
                "/f",
            ]));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the platform's

/// The program a login launch starts: the AppImage when run from one (its
/// mounted executable moves every launch), else this executable.
pub fn launch_target(appimage: Option<String>, exe: Option<PathBuf>) -> Option<PathBuf> {
    appimage
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or(exe)
}

/// This platform's service for this process.
pub fn platform() -> Box<dyn LoginItemService> {
    let exe = launch_target(std::env::var("APPIMAGE").ok(), std::env::current_exe().ok());
    #[cfg(windows)]
    {
        Box::new(RunKey {
            reg: RegExe,
            exec: exe,
        })
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Box::new(LaunchAgent::new(&home, exe))
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_default();
        Box::new(XdgAutostart::new(&config, exe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[test]
    fn a_login_launch_is_recognised_by_its_argument() {
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert!(launched_at_login(args(&["app", "--autostart"])));
        assert!(!launched_at_login(args(&["app"])));
        assert!(!launched_at_login(args(&["--autostart"])), "not argv[0]");
    }

    #[test]
    fn xdg_autostart_round_trips_in_a_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = XdgAutostart::new(dir.path(), Some("/opt/Cua Spaces/cua-spaces".into()));
        assert_eq!(s.status(), LoginItemStatus::NotRegistered);
        assert_eq!(set_and_read(&s, true), Ok(LoginItemStatus::Enabled));
        let text = std::fs::read_to_string(dir.path().join("autostart/com.trycua.spaces.desktop"))
            .unwrap();
        assert!(
            text.contains("Exec=\"/opt/Cua Spaces/cua-spaces\" --autostart\n"),
            "{text}"
        );
        // Turned off by the desktop's own settings: off.
        std::fs::write(&s.file, text.replace("enabled=true", "enabled=false")).unwrap();
        assert_eq!(s.status(), LoginItemStatus::NotRegistered);
        assert_eq!(set_and_read(&s, false), Ok(LoginItemStatus::NotRegistered));
        assert!(!s.file.exists());
        assert_eq!(
            set_and_read(&s, false),
            Ok(LoginItemStatus::NotRegistered),
            "twice"
        );
        let unknown = XdgAutostart::new(dir.path(), None);
        assert_eq!(unknown.status(), LoginItemStatus::NotFound);
        assert!(unknown.set(true).is_err());
    }

    #[test]
    fn desktop_exec_quoting() {
        assert_eq!(desktop_quote(r#"/a "b"/$c\d%"#), r#""/a \"b\"/\$c\\d%%""#);
    }

    #[test]
    fn launch_agent_round_trips_in_a_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = LaunchAgent::new(
            dir.path(),
            Some("/Applications/A&B.app/Contents/MacOS/app".into()),
        );
        assert_eq!(s.status(), LoginItemStatus::NotRegistered);
        assert_eq!(set_and_read(&s, true), Ok(LoginItemStatus::Enabled));
        let text = std::fs::read_to_string(
            dir.path()
                .join("Library/LaunchAgents/com.trycua.spaces.plist"),
        )
        .unwrap();
        assert!(text.contains("<string>/Applications/A&amp;B.app/Contents/MacOS/app</string>"));
        assert!(text.contains("<string>--autostart</string>"));
        assert_eq!(set_and_read(&s, false), Ok(LoginItemStatus::NotRegistered));
    }

    /// An in-memory registry answering `reg query|add|delete`.
    #[derive(Default)]
    struct FakeReg(Mutex<BTreeMap<(String, String), (String, String)>>);

    impl Reg for &FakeReg {
        fn run(&self, a: &[String]) -> Option<String> {
            let mut m = self.0.lock().unwrap();
            let key = (a[1].clone(), a[3].clone());
            match a[0].as_str() {
                "query" => m
                    .get(&key)
                    .map(|(t, d)| format!("\r\n{}\r\n    {}    {t}    {d}\r\n", a[1], a[3])),
                "add" => {
                    m.insert(key, (a[5].clone(), a[7].clone()));
                    Some(String::new())
                }
                "delete" => m.remove(&key).map(|_| String::new()),
                _ => None,
            }
        }
    }

    #[test]
    fn the_run_key_round_trips_in_a_fake_registry() {
        let fake = FakeReg::default();
        let s = RunKey {
            reg: &fake,
            exec: Some(r"C:\Program Files\Cua Spaces\cua-spaces.exe".into()),
        };
        assert_eq!(s.status(), LoginItemStatus::NotRegistered);
        assert_eq!(set_and_read(&s, true), Ok(LoginItemStatus::Enabled));
        assert_eq!(
            fake.0.lock().unwrap()[&(RUN_KEY.to_string(), RUN_VALUE.to_string())].1,
            r#""C:\Program Files\Cua Spaces\cua-spaces.exe" --autostart"#
        );
        // Turned off in Task Manager: off; turning it on here clears that.
        fake.0.lock().unwrap().insert(
            (APPROVED_KEY.into(), RUN_VALUE.into()),
            ("REG_BINARY".into(), "030000000000000000000000".into()),
        );
        assert_eq!(s.status(), LoginItemStatus::NotRegistered);
        assert_eq!(set_and_read(&s, true), Ok(LoginItemStatus::Enabled));
        assert_eq!(set_and_read(&s, false), Ok(LoginItemStatus::NotRegistered));
        assert_eq!(
            set_and_read(&s, false),
            Ok(LoginItemStatus::NotRegistered),
            "twice"
        );
    }

    #[test]
    fn an_appimage_starts_by_its_own_path() {
        assert_eq!(
            launch_target(
                Some("/home/a/Cua.AppImage".into()),
                Some("/tmp/.mount_x/app".into())
            ),
            Some("/home/a/Cua.AppImage".into())
        );
        assert_eq!(
            launch_target(Some(String::new()), Some("/usr/bin/cua-spaces".into())),
            Some("/usr/bin/cua-spaces".into())
        );
    }
}
