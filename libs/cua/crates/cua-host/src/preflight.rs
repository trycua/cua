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
        let out = std::process::Command::new(driver_bin)
            .arg("check-permissions")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        Some((
            v.get("screen_recording")?.as_bool()?,
            v.get("accessibility")?.as_bool()?,
        ))
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
