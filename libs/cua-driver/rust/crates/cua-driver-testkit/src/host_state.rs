//! Per-daemon isolation from the developer's real per-user state.
//!
//! The driver resolves its own persistent state (Computer History admission,
//! the `~/.cua-driver` config, telemetry markers, extension installs, release
//! channel, libei restore tokens, ...) from `HOME`, the XDG base directories,
//! and on Windows `LOCALAPPDATA` / `USERPROFILE`. A test-owned daemon that
//! inherits those values reads whatever the developer's installed product left
//! behind, so the same test can pass in CI and fail on a maintainer's machine
//! (for example, a real `admission.json` that makes a debug daemon request
//! History admission and exit before binding, #4094).
//!
//! Isolation is deliberately limited to the locations the driver derives its
//! own state from. Locations the operating system uses to discover installed
//! apps stay on the host, because tests launch real apps through the daemon:
//!
//! - Windows keeps `USERPROFILE` and `APPDATA`. The shell expands the per-user
//!   known folders (Start Menu, `shell:AppsFolder`) through `%USERPROFILE%`,
//!   and a redirected profile hides registered apps such as Microsoft Edge.
//!   The driver's `USERPROFILE`-derived state goes through its own
//!   `CUA_DRIVER_RS_HOME` override instead.
//! - Linux pins `XDG_DATA_HOME` to the host data directory so user `.desktop`
//!   entries remain discoverable after `HOME` moves.
//!
//! Every testkit spawner therefore points those variables at a fresh temporary
//! directory owned by the spawned daemon. Callers keep full control:
//!
//! - any state variable in the caller's `env` wins over the isolated default,
//!   so a test that needs one root across several spawns creates its own
//!   tempdir and passes it explicitly; and
//! - [`SHARE_HOST_STATE`] in the caller's `env` disables isolation entirely for
//!   tests that intentionally observe host-owned state. The marker itself is
//!   never forwarded to the child.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Environment marker that opts a testkit spawn out of state isolation.
///
/// Include it in the `env` slice passed to a testkit constructor. It is
/// consumed by the testkit and is not forwarded to the spawned driver.
pub const SHARE_HOST_STATE: (&str, &str) = ("CUA_TESTKIT_SHARE_HOST_STATE", "1");

/// Variables that locate per-user driver state on the current platform,
/// relative to the isolated root.
#[cfg(target_os = "windows")]
const STATE_VARIABLES: &[(&str, &str)] = &[
    ("HOME", ""),
    ("LOCALAPPDATA", "AppData/Local"),
    ("CUA_DRIVER_RS_HOME", ".cua-driver"),
];

#[cfg(not(target_os = "windows"))]
const STATE_VARIABLES: &[(&str, &str)] = &[
    ("HOME", ""),
    ("XDG_CONFIG_HOME", ".config"),
    ("XDG_STATE_HOME", ".local/state"),
];

/// Host app-discovery locations that must survive the `HOME` redirect.
fn host_discovery_env() -> Vec<(&'static str, PathBuf)> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            });
        if let Some(data_home) = data_home {
            return vec![("XDG_DATA_HOME", data_home)];
        }
    }
    Vec::new()
}

/// A temporary per-user state root for one test-owned driver.
///
/// Testkit transports create one automatically. Tests that launch a driver
/// through another path (for example the SDK's embedded host) create one
/// directly and pass [`IsolatedStateRoot::env`] to that launcher. The
/// directory is removed on drop, so keep the value alive for the child's
/// lifetime.
pub struct IsolatedStateRoot {
    dir: tempfile::TempDir,
}

impl IsolatedStateRoot {
    /// Create a fresh, empty per-user state root.
    pub fn new() -> std::io::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("cua-driver-test-home-")
            .tempdir()?;
        for (_, relative) in STATE_VARIABLES {
            if !relative.is_empty() {
                std::fs::create_dir_all(dir.path().join(relative))?;
            }
        }
        Ok(Self { dir })
    }

    /// Create an isolated root unless the caller opted into host state.
    pub(crate) fn for_env(env: &[(&str, &str)]) -> Option<Self> {
        if shares_host_state(env) {
            return None;
        }
        Self::new()
            .inspect_err(|error| {
                eprintln!("[testkit] create isolated driver state root failed: {error}")
            })
            .ok()
    }

    /// Root directory standing in for the user's home.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Per-user state variables pointing into this root, plus any host
    /// app-discovery location that must stay visible after `HOME` moves.
    pub fn env(&self) -> Vec<(&'static str, PathBuf)> {
        STATE_VARIABLES
            .iter()
            .map(|(name, relative)| (*name, self.dir.path().join(relative)))
            .chain(host_discovery_env())
            .collect()
    }

    /// Point every state variable at this root. Call before applying the
    /// caller's environment so explicit caller values take precedence.
    pub(crate) fn apply(&self, command: &mut Command) {
        command.envs(self.env());
    }
}

/// Apply an optional isolated root, then the caller's environment minus the
/// testkit-only [`SHARE_HOST_STATE`] marker.
pub(crate) fn apply_env(
    command: &mut Command,
    state_root: Option<&IsolatedStateRoot>,
    env: &[(&str, &str)],
) {
    if let Some(root) = state_root {
        root.apply(command);
    }
    for (key, value) in env {
        if *key != SHARE_HOST_STATE.0 {
            command.env(key, value);
        }
    }
}

pub(crate) fn shares_host_state(env: &[(&str, &str)]) -> bool {
    env.iter().any(|(key, _)| *key == SHARE_HOST_STATE.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envs(command: &Command) -> Vec<(String, Option<String>)> {
        command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn value_of(command: &Command, name: &str) -> Option<String> {
        envs(command)
            .into_iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value)
    }

    #[test]
    fn isolated_root_redirects_every_state_variable() {
        let root = IsolatedStateRoot::for_env(&[]).expect("isolated root");
        let mut command = Command::new("unused");
        apply_env(&mut command, Some(&root), &[]);
        for (name, relative) in STATE_VARIABLES {
            let value = value_of(&command, name).unwrap_or_else(|| panic!("{name} not set"));
            assert_eq!(Path::new(&value), root.path().join(relative));
            assert!(Path::new(&value).is_dir(), "{name} directory must exist");
        }
    }

    #[test]
    fn host_app_discovery_locations_are_not_redirected() {
        let root = IsolatedStateRoot::new().expect("isolated root");
        let env = root.env();
        for name in ["USERPROFILE", "APPDATA", "XDG_DATA_HOME"] {
            if let Some((_, value)) = env.iter().find(|(key, _)| *key == name) {
                assert!(
                    !value.starts_with(root.path()),
                    "{name} locates installed apps and must stay on the host"
                );
            }
        }
    }

    #[test]
    fn caller_values_override_isolated_defaults() {
        let root = IsolatedStateRoot::for_env(&[("HOME", "/caller/home")]).expect("root");
        let mut command = Command::new("unused");
        apply_env(&mut command, Some(&root), &[("HOME", "/caller/home")]);
        assert_eq!(value_of(&command, "HOME").as_deref(), Some("/caller/home"));
    }

    #[test]
    fn share_marker_disables_isolation_and_is_not_forwarded() {
        assert!(IsolatedStateRoot::for_env(&[SHARE_HOST_STATE]).is_none());
        let mut command = Command::new("unused");
        apply_env(&mut command, None, &[SHARE_HOST_STATE, ("A", "b")]);
        assert_eq!(value_of(&command, SHARE_HOST_STATE.0), None);
        assert_eq!(value_of(&command, "HOME"), None);
        assert_eq!(value_of(&command, "A").as_deref(), Some("b"));
    }
}
