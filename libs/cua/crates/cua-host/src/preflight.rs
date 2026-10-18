//! Preflight checks for `cua host setup` on a Mac that may have nobody
//! logged in at the console (set up over ssh, or run with `-y` / piped).
//!
//! The launchd LaunchAgent [`crate::service::launchd_plist`] generates
//! always carries `LimitLoadToSessionType=Aqua`: it only runs in the GUI
//! (Aqua) session of a logged-in user, because that is the session screen
//! capture and accessibility input run in with the user's own TCC grants.
//! `launchctl bootstrap gui/<uid>` fails when that session does not exist,
//! often with no useful message, and used to be reached only after the
//! service was already installed. [`check`] runs first and fails loudly
//! with the exact remediation instead.
//!
//! It also reports whether the macOS permissions cua-spacesd needs (Screen
//! Recording, Accessibility) are already granted, by asking the installed
//! driver binary for its own live TCC status (`<driver_bin>
//! check-permissions`): those grants are per-executable, so only the
//! driver binary itself can answer for itself. Nothing here grants or
//! opens anything; see [`crate::permission_hints`] for what to tell the
//! user to open.
//!
//! [`SessionProbe`] is the seam: [`SystemProbe`] is the real
//! implementation (shells out to `launchctl`, `stat`, and the driver
//! binary); tests use a fake.

use crate::{Error, Result};
use std::path::Path;

/// What the preflight found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreflightReport {
    /// A GUI (Aqua) session is active for this user (always `true` on
    /// platforms that do not need one: nothing here is macOS-only by
    /// accident, but the check itself only runs on macOS).
    pub gui_session: bool,
    /// The console user (`stat -f%Su /dev/console`), when it could be
    /// read. `None` when nobody is logged in at the console (macOS
    /// reports the console as owned by `root` in that case) or it could
    /// not be determined.
    pub console_user: Option<String>,
    /// Screen Recording is granted to the driver binary. `None`: could
    /// not be determined (not macOS, the driver is not installed yet, or
    /// it predates `check-permissions`).
    pub screen_recording: Option<bool>,
    /// Accessibility is granted to the driver binary.
    pub accessibility: Option<bool>,
}

/// Seam over the OS calls the preflight needs, so it is testable with a
/// fake instead of a real GUI session, console and driver binary.
pub trait SessionProbe: Send + Sync {
    /// Whether `gui/<uid>` has a bootstrapped GUI session: what
    /// `launchctl bootstrap gui/<uid> ...` needs to succeed.
    fn gui_session(&self, uid: u32) -> bool;
    /// The console user, when it can be read (see
    /// [`PreflightReport::console_user`]).
    fn console_user(&self) -> Option<String>;
    /// `driver_bin`'s own live TCC status as `(screen_recording,
    /// accessibility)`, or `None` when it could not be determined.
    fn permission_status(&self, driver_bin: &Path) -> Option<(bool, bool)>;
}

/// The real [`SessionProbe`]: shells out to `launchctl print`, `stat`, and
/// the driver binary's own `check-permissions`. Never grants or opens
/// anything; every call here is read-only.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemProbe;

impl SessionProbe for SystemProbe {
    fn gui_session(&self, uid: u32) -> bool {
        std::process::Command::new("launchctl")
            .args(["print", &format!("gui/{uid}")])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn console_user(&self) -> Option<String> {
        let out = std::process::Command::new("stat")
            .args(["-f%Su", "/dev/console"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let user = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // Nobody logged in at the console: macOS reports it as root's.
        if user.is_empty() || user == "root" {
            None
        } else {
            Some(user)
        }
    }

    fn permission_status(&self, driver_bin: &Path) -> Option<(bool, bool)> {
        if !driver_bin.is_file() {
            return None;
        }
        let stdout = check_permissions_output(driver_bin)?;
        let v: serde_json::Value = serde_json::from_slice(&stdout).ok()?;
        Some((
            v.get("screen_recording")?.as_bool()?,
            v.get("accessibility")?.as_bool()?,
        ))
    }
}

/// How long `<driver> check-permissions` may take.
const PERMISSION_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// `<driver_bin> check-permissions`'s stdout, when it exits 0 in time.
///
/// On macOS the child disclaims responsibility: TCC answers for the
/// process macOS holds responsible, which for a plain child is its parent
/// (the app, or the terminal `cua` runs in), not the driver the
/// LaunchAgent runs. Disclaimed, the driver answers for itself, and a
/// fresh process each time sees a grant made a moment ago.
fn check_permissions_output(driver_bin: &Path) -> Option<Vec<u8>> {
    #[cfg(target_os = "macos")]
    {
        disclaimed::output(driver_bin, "check-permissions", PERMISSION_CHECK_TIMEOUT)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = PERMISSION_CHECK_TIMEOUT;
        let out = std::process::Command::new(driver_bin)
            .arg("check-permissions")
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    }
}

#[cfg(target_os = "macos")]
mod disclaimed {
    use std::ffi::CString;
    use std::io::Read as _;
    use std::os::fd::FromRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;
    use std::time::Duration;

    unsafe extern "C" {
        // libsystem_secinit; used by Chromium, LLDB and Xcode for the same
        // reason. Present on every macOS cua supports.
        fn responsibility_spawnattrs_setdisclaim(
            attrs: *mut libc::posix_spawnattr_t,
            disclaim: libc::c_int,
        ) -> libc::c_int;
    }

    /// Runs `program arg` responsible for itself; its stdout when it exits
    /// 0 within `timeout` (killed otherwise).
    pub(super) fn output(program: &Path, arg: &str, timeout: Duration) -> Option<Vec<u8>> {
        let path = CString::new(program.as_os_str().as_bytes()).ok()?;
        let arg = CString::new(arg).ok()?;
        let devnull = CString::new("/dev/null").ok()?;
        let argv = [
            path.as_ptr() as *mut libc::c_char,
            arg.as_ptr() as *mut libc::c_char,
            std::ptr::null_mut(),
        ];
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: plain libc calls on locals; every resource is released
        // on every path below.
        unsafe {
            if libc::pipe(fds.as_mut_ptr()) != 0 {
                return None;
            }
            let (read_fd, write_fd) = (fds[0], fds[1]);
            let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
            let mut attrs: libc::posix_spawnattr_t = std::mem::zeroed();
            libc::posix_spawn_file_actions_init(&mut actions);
            libc::posix_spawnattr_init(&mut attrs);
            libc::posix_spawn_file_actions_addopen(
                &mut actions,
                0,
                devnull.as_ptr(),
                libc::O_RDONLY,
                0,
            );
            libc::posix_spawn_file_actions_adddup2(&mut actions, write_fd, 1);
            libc::posix_spawn_file_actions_addopen(
                &mut actions,
                2,
                devnull.as_ptr(),
                libc::O_WRONLY,
                0,
            );
            libc::posix_spawn_file_actions_addclose(&mut actions, read_fd);
            libc::posix_spawn_file_actions_addclose(&mut actions, write_fd);
            responsibility_spawnattrs_setdisclaim(&mut attrs, 1);
            let mut pid: libc::pid_t = 0;
            let spawned = libc::posix_spawn(
                &mut pid,
                path.as_ptr(),
                &actions,
                &attrs,
                argv.as_ptr(),
                *libc::_NSGetEnviron(),
            );
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attrs);
            libc::close(write_fd);
            let reader = std::fs::File::from_raw_fd(read_fd);
            if spawned != 0 {
                return None;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut out = Vec::new();
                let _ = reader.take(64 * 1024).read_to_end(&mut out);
                let _ = tx.send(out);
            });
            let out = rx.recv_timeout(timeout).ok();
            if out.is_none() {
                libc::kill(pid, libc::SIGKILL);
            }
            let mut status = 0;
            libc::waitpid(pid, &mut status, 0);
            let ok = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
            out.filter(|_| ok)
        }
    }
}

/// This process's user id (`0` on platforms without one).
#[cfg(unix)]
pub fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

/// This process's user id (`0` on platforms without one).
#[cfg(not(unix))]
pub fn current_uid() -> u32 {
    0
}

/// Runs the preflight before bootstrapping the LaunchAgent: without a GUI
/// (Aqua) session for this account, `launchctl bootstrap gui/<uid>` fails.
/// A machine set up over ssh with nobody logged in at the console hit
/// exactly that, previously with no clear explanation. Callers only need
/// this ahead of the launchd runner (macOS); it does not gate on the
/// platform itself, so it is exercised the same way on every OS the tests
/// run on, through [`SessionProbe`].
pub fn check(probe: &dyn SessionProbe, driver_bin: &Path) -> Result<PreflightReport> {
    let uid = current_uid();
    let gui_session = probe.gui_session(uid);
    let console_user = probe.console_user();
    if !gui_session {
        let who = console_user.as_deref().unwrap_or("nobody");
        return Err(Error::Service(format!(
            "no GUI (Aqua) session for this account (console user: {who}); cua-spacesd's \
             LaunchAgent needs one for screen sharing and input, and `launchctl bootstrap \
             gui/{uid}` would fail here. Log in at the console once, or turn on automatic \
             login (System Settings > Users & Groups > Login Options > Automatic login), \
             then run `cua host setup` again."
        )));
    }
    let (screen_recording, accessibility) = match probe.permission_status(driver_bin) {
        Some((sr, ax)) => (Some(sr), Some(ax)),
        None => (None, None),
    };
    Ok(PreflightReport {
        gui_session,
        console_user,
        screen_recording,
        accessibility,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeProbe {
        gui: bool,
        console_user: Option<String>,
        permissions: Option<(bool, bool)>,
        /// uid the test expects `gui_session` to be asked about.
        want_uid: Mutex<Option<u32>>,
    }

    impl SessionProbe for FakeProbe {
        fn gui_session(&self, uid: u32) -> bool {
            *self.want_uid.lock().unwrap_or_else(|e| e.into_inner()) = Some(uid);
            self.gui
        }
        fn console_user(&self) -> Option<String> {
            self.console_user.clone()
        }
        fn permission_status(&self, _driver_bin: &Path) -> Option<(bool, bool)> {
            self.permissions
        }
    }

    #[test]
    fn no_gui_session_fails_with_the_remediation() {
        let probe = FakeProbe {
            gui: false,
            console_user: None,
            ..Default::default()
        };
        let err = check(&probe, Path::new("/tmp/cua-spacesd")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no GUI"), "{msg}");
        assert!(msg.contains("Automatic login"), "{msg}");
        assert!(msg.contains("cua host setup"), "{msg}");
        assert_eq!(
            *probe.want_uid.lock().unwrap_or_else(|e| e.into_inner()),
            Some(current_uid()),
            "checked gui/<uid> for this process's own uid"
        );
    }

    #[test]
    fn no_gui_session_names_the_console_user_when_known() {
        let probe = FakeProbe {
            gui: false,
            console_user: Some("ada".into()),
            ..Default::default()
        };
        let err = check(&probe, Path::new("/tmp/cua-spacesd")).unwrap_err();
        assert!(err.to_string().contains("console user: ada"));
    }

    #[test]
    fn a_gui_session_passes_and_carries_permission_status() {
        let probe = FakeProbe {
            gui: true,
            console_user: Some("ada".into()),
            permissions: Some((true, false)),
            ..Default::default()
        };
        let report = check(&probe, Path::new("/tmp/cua-spacesd")).unwrap();
        assert!(report.gui_session);
        assert_eq!(report.console_user.as_deref(), Some("ada"));
        assert_eq!(report.screen_recording, Some(true));
        assert_eq!(report.accessibility, Some(false));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_disclaimed_child_runs_and_its_stdout_is_read() {
        let d = std::time::Duration::from_secs(5);
        let out = super::disclaimed::output(Path::new("/bin/echo"), "hi", d).unwrap();
        assert_eq!(out, b"hi\n");
        // A non-zero exit is no answer.
        assert!(super::disclaimed::output(Path::new("/usr/bin/false"), "x", d).is_none());
        assert!(super::disclaimed::output(Path::new("/nonexistent/bin"), "x", d).is_none());
        // A child that does not answer in time is killed.
        let t = std::time::Instant::now();
        assert!(
            super::disclaimed::output(
                Path::new("/bin/sleep"),
                "30",
                std::time::Duration::from_millis(300)
            )
            .is_none()
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn unknown_permission_status_is_not_a_failure() {
        let probe = FakeProbe {
            gui: true,
            permissions: None,
            ..Default::default()
        };
        let report = check(&probe, Path::new("/tmp/cua-spacesd")).unwrap();
        assert!(report.gui_session);
        assert_eq!(report.screen_recording, None);
        assert_eq!(report.accessibility, None);
    }
}
