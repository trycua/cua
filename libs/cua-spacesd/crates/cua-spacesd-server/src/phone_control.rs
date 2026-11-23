// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A phone and this computer: scope, not consent.
//!
//! A phone enrolled on the account is a smaller kind of client
//! ([`cua_relay::phone`]). It reaches what a Space is, and nothing of the
//! computer underneath:
//!
//! - the thread of a Space, the picture of a screen, a viewer ticket;
//! - the pointer and the keyboard, only on a desktop this computer shares as a
//!   Space (`share_desktop`), and only when the account's role allows touching
//!   it (the owner's own phone and an editor do, a view-only share does not).
//!   That is the rule every app uses for a shared desktop, so there is no
//!   second consent for a phone: sharing the desktop is the consent;
//! - never the computer's own services: files, processes, tunnels, the
//!   driver, shutdown, creating Spaces. The list is [`cua_relay::phone`], and
//!   the machine checks it again on every call (`auth.rs`).
//!
//! What this module keeps is small and quiet:
//!
//! - **The kill switch.** `phones-off` in the state folder: while it exists
//!   this computer answers no phone at all (`cua-spacesd phone off`). The file
//!   is read on every call, so it takes effect with the next call, and a
//!   running stream is cut within a second (the stream guard in `auth.rs`).
//! - **The audit log.** `audit.jsonl`: when a phone first looked at, or
//!   touched, this computer (once a minute per phone and scope), every refusal
//!   (rate limited), and every switch of the kill switch. Time, event,
//!   account, device. No token, ticket or key is ever written.
//!
//! The state is a folder, `0700`, of small files `0600`. The whole feature is
//! the experiment `mobile` ([`mobile_enabled`]): while it is off, this
//! computer does not answer a phone at all.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A phone's use of this computer is logged no more often than this.
const USE_LOG_SECS: u64 = 60;

/// The folder inside the daemon's data dir.
pub const DIR_NAME: &str = "phone-control";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What a phone did, for the log: it looked (the short list) or it touched
/// the desktop (pointer or keyboard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The thread, the picture of the screen, a viewer ticket.
    View,
    /// The pointer or the keyboard of a desktop shared as a Space.
    Control,
}

impl Scope {
    fn event(self) -> &'static str {
        match self {
            Scope::View => "phone_looked",
            Scope::Control => "phone_controlled",
        }
    }
}

/// The state of phones on one computer.
pub struct PhoneControl {
    dir: PathBuf,
    /// Last use logged per device and scope.
    used: Mutex<BTreeMap<String, Instant>>,
}

impl std::fmt::Debug for PhoneControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PhoneControl({})", self.dir.display())
    }
}

/// Why a command did not do what it was asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The state could not be written.
    Io(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Io(e) => write!(f, "could not save: {e}"),
        }
    }
}

impl PhoneControl {
    /// The state under `<data_dir>/phone-control`.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        Self::open(data_dir.join(DIR_NAME))
    }

    /// The state in `dir` (created, `0700`, when first written).
    pub fn open(dir: PathBuf) -> Self {
        Self {
            dir,
            used: Mutex::default(),
        }
    }

    fn off_path(&self) -> PathBuf {
        self.dir.join("phones-off")
    }

    /// The audit log's path.
    pub fn audit_path(&self) -> PathBuf {
        self.dir.join("audit.jsonl")
    }

    /// Whether the kill switch is on: this computer answers no phone.
    pub fn phones_off(&self) -> bool {
        self.off_path().exists()
    }

    /// Turns the kill switch on or off.
    pub fn set_off(&self, off: bool) -> Result<(), Refused> {
        let io = |e: std::io::Error| Refused::Io(e.to_string());
        if off {
            make_private_dir(&self.dir).map_err(io)?;
            write_private(&self.off_path(), b"off\n").map_err(io)?;
        } else {
            match std::fs::remove_file(self.off_path()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e)),
            }
        }
        self.audit(if off { "phones_off" } else { "phones_on" }, "", "", "");
        Ok(())
    }

    fn audit(&self, event: &str, account: &str, device: &str, detail: &str) {
        let line = serde_json::json!({
            "t": now_secs(), "event": event, "account": account, "device": device,
            "detail": detail,
        });
        let _ = (|| -> std::io::Result<()> {
            make_private_dir(&self.dir)?;
            let mut f = open_private_append(&self.audit_path())?;
            writeln!(f, "{line}")
        })();
    }

    /// Logs that a phone looked at, or touched, this computer (at most once a
    /// minute per phone and scope).
    pub fn note_use(&self, account: &str, device: &str, scope: Scope) {
        let key = format!("{}:{device}", scope.event());
        let mut used = self.used.lock().expect("phone use");
        let now = Instant::now();
        let due = used
            .get(&key)
            .is_none_or(|t| now.duration_since(*t) >= Duration::from_secs(USE_LOG_SECS));
        if due {
            used.insert(key, now);
            drop(used);
            self.audit(scope.event(), account, device, "");
        }
    }

    /// Logs a refused call (rate limited like a use).
    pub fn note_refused(&self, account: &str, device: &str, what: &str) {
        let key = format!("refused:{device}");
        let mut used = self.used.lock().expect("phone use");
        let now = Instant::now();
        let due = used
            .get(&key)
            .is_none_or(|t| now.duration_since(*t) >= Duration::from_secs(USE_LOG_SECS));
        if due {
            used.insert(key, now);
            drop(used);
            self.audit("phone_refused", account, device, what);
        }
    }

    /// The last `n` audit lines.
    pub fn audit_tail(&self, n: usize) -> Vec<String> {
        let text = std::fs::read_to_string(self.audit_path()).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(n)..]
            .iter()
            .map(|l| l.to_string())
            .collect()
    }
}

#[cfg(unix)]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(unix)]
fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

// ---- the experiment -----------------------------------------------------------

/// The experiment this feature is. Mirrors `cua_vmm::experiments` for the one
/// switch the daemon needs (the daemon does not link the rest): the process
/// environment (`CUA_EXPERIMENTS=mobile` on, `-mobile` off) wins over the
/// switch file `<cua home>/experiments/mobile` (`on`), and absent means off.
pub const EXPERIMENT: &str = "mobile";

/// What `CUA_EXPERIMENTS` (`value`) says about [`EXPERIMENT`].
pub fn env_says(value: Option<&str>) -> Option<bool> {
    let mut said = None;
    for word in value?.split(|c: char| c == ',' || c.is_whitespace()) {
        let word = word.trim().to_ascii_lowercase().replace('_', "-");
        if let Some(off) = word.strip_prefix('-') {
            if off == EXPERIMENT {
                said = Some(false);
            }
        } else if word == EXPERIMENT {
            said = Some(true);
        }
    }
    said
}

/// Whether the mobile experiment is on, for the cua home `home` (the one the
/// host policy names; `None`: `CUA_HOME`, else `~/.cua`).
pub fn mobile_enabled(home: Option<&Path>) -> bool {
    if let Some(v) = env_says(std::env::var("CUA_EXPERIMENTS").ok().as_deref()) {
        return v;
    }
    let home = home.map(Path::to_path_buf).or_else(|| {
        std::env::var_os("CUA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cua")))
    });
    let Some(home) = home else { return false };
    std::fs::read_to_string(home.join("experiments").join(EXPERIMENT))
        .is_ok_and(|t| t.trim() == "on")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, PhoneControl) {
        let dir = tempfile::tempdir().unwrap();
        let s = PhoneControl::in_data_dir(dir.path());
        (dir, s)
    }

    #[test]
    fn the_kill_switch_is_a_file_read_every_time() {
        let (d, s) = store();
        assert!(!s.phones_off());
        s.set_off(true).unwrap();
        assert!(s.phones_off());
        // Another handle on the same folder (the CLI) sees it at once.
        assert!(PhoneControl::in_data_dir(d.path()).phones_off());
        s.set_off(false).unwrap();
        assert!(!s.phones_off());
        s.set_off(false).unwrap(); // already on: not an error
        let log = s.audit_tail(10).join("\n");
        assert!(
            log.contains("phones_off") && log.contains("phones_on"),
            "{log}"
        );
    }

    #[test]
    fn a_use_and_a_refusal_are_logged_once_a_minute_and_hold_no_secret() {
        let (_d, s) = store();
        for _ in 0..5 {
            s.note_use("acct", "dev_1", Scope::View);
            s.note_use("acct", "dev_1", Scope::Control);
            s.note_refused("acct", "dev_1", "FilesystemService");
        }
        let log = s.audit_tail(50);
        assert_eq!(log.len(), 3, "{log:?}");
        assert!(
            log[0].contains("phone_looked")
                && log[1].contains("phone_controlled")
                && log[2].contains("phone_refused")
        );
        assert!(log
            .iter()
            .all(|l| !l.contains("token") && !l.contains("ticket")));
        // Another phone is its own line.
        s.note_use("acct", "dev_2", Scope::View);
        assert_eq!(s.audit_tail(50).len(), 4);
    }

    #[test]
    fn the_state_is_private() {
        let (_d, s) = store();
        s.set_off(true).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let dir_mode = std::fs::metadata(&s.dir).unwrap().permissions().mode() & 0o777;
            let file_mode = std::fs::metadata(s.audit_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!((dir_mode, file_mode), (0o700, 0o600));
        }
    }

    #[test]
    fn the_experiment_is_read_from_the_environment_and_the_switch_file() {
        assert_eq!(env_says(Some("mobile")), Some(true));
        assert_eq!(env_says(Some("secure-spaces, -mobile")), Some(false));
        assert_eq!(env_says(Some("ci-runners")), None);
        assert_eq!(env_says(None), None);
    }
}
