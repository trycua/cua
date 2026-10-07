//! Which build a running daemon is, so two app bundles (or an app and a
//! CLI) never silently share a daemon of another build.
//!
//! A daemon reports its executable (symlinks resolved) and a build id (the
//! version plus the file's size and modification time, as they were when it
//! started) in `GetInfoResponse`. A client inside an app bundle that ships
//! its own `cua` ([`bundled_cua`]) expects exactly that daemon: another
//! app's daemon, or its own after the app was rebuilt or updated, is a
//! stranger ([`stranger`]). Its Spaces would run with that app's build and
//! permissions (macOS Local Network access among them), so `cua daemon
//! start` replaces it and a client refuses it with a clear message.

use std::path::Path;
use std::time::UNIX_EPOCH;

/// A daemon executable's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The executable, symlinks resolved.
    pub executable: String,
    /// `<version>-<size hex>-<mtime ns hex>`.
    pub build_id: String,
}

/// The identity of the executable at `exe`, as it is now.
pub fn of(exe: &Path) -> Option<Identity> {
    let real = exe.canonicalize().ok()?;
    let meta = std::fs::metadata(&real).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(Identity {
        executable: real.display().to_string(),
        build_id: format!("{}-{:x}-{mtime:x}", crate::VERSION, meta.len()),
    })
}

pub use cua_home::{bundled_cua, bundled_cua_of};

/// Why a daemon reporting `executable` and `build_id` is not the one at
/// `expected`, or `None` when it is.
pub fn stranger(executable: &str, build_id: &str, expected: &Path) -> Option<String> {
    let Some(want) = of(expected) else {
        return Some(format!("{} cannot be read", expected.display()));
    };
    if executable.is_empty() {
        return Some("it predates daemon identity (an older build)".into());
    }
    if executable != want.executable {
        return Some(format!("it runs {executable}"));
    }
    if build_id != want.build_id {
        return Some(format!(
            "{executable} was rebuilt or updated since it started"
        ));
    }
    None
}

/// For a client inside an app bundle ([`bundled_cua`]): `Err` when the
/// daemon `info` describes is not the bundle's own `cua` (a stranger this
/// client must not use), `Ok` otherwise (and always outside a bundle, where
/// any daemon is shared).
pub fn check(info: &cua_proto::daemon::v1::GetInfoResponse) -> crate::Result<()> {
    let Some(own) = bundled_cua() else {
        return Ok(());
    };
    match stranger(&info.executable, &info.build_id, &own) {
        None => Ok(()),
        Some(why) => Err(stranger_error(info.pid, &why, &own)),
    }
}

/// The error for a client that will not use a stranger daemon: what runs,
/// and how to get this app's own.
pub fn stranger_error(pid: u32, why: &str, expected: &Path) -> crate::Error {
    crate::Error::DaemonNotRunning(format!(
        "the running cua daemon (pid {pid}) is not this app's: {why}. Run `{} daemon start` \
         to replace it with this app's, or stop it with `cua daemon stop`",
        expected.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_daemon_is_known_by_its_executable_and_build() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cua");
        std::fs::write(&exe, b"one").unwrap();
        let me = of(&exe).unwrap();
        assert!(me.build_id.starts_with(&format!("{}-3-", crate::VERSION)));
        // Its own daemon.
        assert_eq!(stranger(&me.executable, &me.build_id, &exe), None);
        // Through a symlink: the same file.
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&exe, &link).unwrap();
        assert_eq!(stranger(&me.executable, &me.build_id, &link), None);
        // Another executable.
        let other = dir.path().join("other");
        std::fs::write(&other, b"one").unwrap();
        let them = of(&other).unwrap();
        assert_eq!(
            stranger(&them.executable, &them.build_id, &exe),
            Some(format!("it runs {}", them.executable))
        );
        // A daemon that predates identity.
        assert!(stranger("", "", &exe).unwrap().contains("older build"));
        // The same path, rebuilt since the daemon started.
        std::fs::write(&exe, b"rebuilt").unwrap();
        assert!(
            stranger(&me.executable, &me.build_id, &exe)
                .unwrap()
                .ends_with("was rebuilt or updated since it started")
        );
    }

    #[test]
    fn only_an_app_bundle_with_its_own_cua_expects_a_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Cua Spaces.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let app = macos.join("CuaSpacesMac");
        std::fs::write(&app, b"").unwrap();
        // No bundled cua: nothing expected.
        assert_eq!(bundled_cua_of(&app), None);
        // The bundled CLI carries the host's executable suffix.
        let cua = macos.join(if cfg!(windows) { "cua.exe" } else { "cua" });
        std::fs::write(&cua, b"").unwrap();
        let want = cua.canonicalize().unwrap();
        assert_eq!(bundled_cua_of(&app), Some(want.clone()));
        assert_eq!(bundled_cua_of(&cua), Some(want));
        // A CLI outside any bundle shares whatever daemon runs.
        assert_eq!(bundled_cua_of(&dir.path().join("cua")), None);
    }
}
