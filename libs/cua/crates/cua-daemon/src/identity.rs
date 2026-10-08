//! Which build a running daemon is, so two app bundles (or an app and a
//! CLI) never silently share a daemon of another build.
//!
//! A daemon reports its executable (symlinks resolved) and a build id (the
//! version plus the file's size and modification time, as they were when it
//! started) in `GetInfoResponse`. A client inside an app bundle that ships
//! its own `cua` ([`bundled_cua`]) expects exactly that daemon: another
//! app's daemon, or its own after the app was rebuilt or updated, is a
//! stranger ([`stranger`]). Its Spaces would run with that app's build and
//! permissions (macOS Local Network access among them).
//!
//! Two installed apps (the SwiftUI app and the Electron app, or two
//! versions of one) may run side by side, and each must not keep replacing
//! the other's daemon. So a stranger is replaced only when it is older than
//! this build, or its version cannot be read ([`verdict`]): `cua daemon
//! start` replaces it and a client refuses it with a clear message. A
//! stranger of the same or a newer version from another executable is kept
//! and used. This app's own executable, rebuilt or updated since its daemon
//! started, is always replaced.

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

/// What an app's host does with the running daemon ([`verdict`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The app's own `cua`, as it is now.
    Own,
    /// Another executable's daemon of the same or a newer version: used as
    /// it is. Says whose.
    Keep(String),
    /// A stranger to replace (older, its version unreadable, or this app's
    /// own executable rebuilt or updated since it started). Says why.
    Replace(String),
}

/// The version a daemon reports: `version`, else the one in its `build_id`
/// (`<version>-<size hex>-<mtime ns hex>`).
fn reported_version<'a>(version: &'a str, build_id: &'a str) -> &'a str {
    if !version.trim().is_empty() {
        return version.trim();
    }
    build_id.rsplitn(3, '-').nth(2).unwrap_or("")
}

/// What a client of this build bound to `expected` does with the daemon
/// reporting `version`, `executable` and `build_id`: its own, one to keep
/// (another executable's, not older than [`crate::VERSION`]), or one to
/// replace.
pub fn verdict(version: &str, executable: &str, build_id: &str, expected: &Path) -> Verdict {
    verdict_for(version, executable, build_id, expected, crate::VERSION)
}

fn verdict_for(
    version: &str,
    executable: &str,
    build_id: &str,
    expected: &Path,
    ours: &str,
) -> Verdict {
    let Some(why) = stranger(executable, build_id, expected) else {
        return Verdict::Own;
    };
    // This app's own executable, changed since its daemon started: the
    // daemon runs a build that is gone.
    let same_file = of(expected).is_some_and(|w| w.executable == executable);
    let theirs = reported_version(version, build_id);
    match (
        crate::client::version_key(theirs),
        crate::client::version_key(ours),
    ) {
        (Some(t), Some(o)) if !same_file && t >= o => Verdict::Keep(format!(
            "{} (cua {theirs}, not older than this app's {ours})",
            if executable.is_empty() {
                "another build"
            } else {
                executable
            }
        )),
        (None, _) if !same_file => Verdict::Replace(format!(
            "{why}, and its version ({}) cannot be read",
            if theirs.is_empty() { "none" } else { theirs }
        )),
        (Some(_), Some(_)) if !same_file => Verdict::Replace(format!(
            "{why}, an older cua ({theirs}) than this app's ({ours})"
        )),
        _ => Verdict::Replace(why),
    }
}

/// For a client inside an app bundle ([`bundled_cua`]): `Err` when the
/// daemon `info` describes is a stranger to replace ([`verdict`]) that this
/// client must not use, `Ok` otherwise: its own, another app's of the same
/// or a newer version, and always outside a bundle, where any daemon is
/// shared.
pub fn check(info: &cua_proto::daemon::v1::GetInfoResponse) -> crate::Result<()> {
    let Some(own) = bundled_cua() else {
        return Ok(());
    };
    match verdict(&info.version, &info.executable, &info.build_id, &own) {
        Verdict::Own | Verdict::Keep(_) => Ok(()),
        Verdict::Replace(why) => Err(stranger_error(info.pid, &why, &own)),
    }
}

/// The error for a Spaces tool the running daemon does not have (another
/// app's build, kept because it is not older): which daemon, and how to
/// get this app's own.
pub fn missing_tool_error(tool: &str, info: &cua_proto::daemon::v1::GetInfoResponse) -> String {
    let whose = if info.executable.is_empty() {
        "another build".to_string()
    } else {
        info.executable.clone()
    };
    format!(
        "Spaces tool {tool}: the running cua daemon (pid {}, cua {}, {whose}) does not have it. \
         It is another app's build; stop it with `cua daemon stop` (or quit that app) and \
         reopen Cua Spaces to start its own",
        info.pid, info.version
    )
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

    /// Two installed apps side by side: each replaces the other's daemon
    /// only when it is older (or of an unknown version), and keeps one of
    /// the same or a newer version, so neither keeps replacing the other.
    #[cfg(unix)]
    #[test]
    fn a_stranger_is_replaced_only_when_older() {
        let dir = tempfile::tempdir().unwrap();
        let swift = dir.path().join("Swift/Cua Spaces.app/Contents/MacOS/cua");
        let electron = dir
            .path()
            .join("Electron/Cua Spaces.app/Contents/Resources/native/cua");
        for p in [&swift, &electron] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"cua").unwrap();
        }
        let id = |p: &std::path::Path, v: &str| {
            let i = of(p).unwrap();
            let build = format!("{v}{}", &i.build_id[crate::VERSION.len()..]);
            (i.executable, build)
        };
        for (own, other) in [(&electron, &swift), (&swift, &electron)] {
            let decide = |theirs: &str, ours: &str| {
                let (exe, build) = id(other, theirs);
                verdict_for(theirs, &exe, &build, own, ours)
            };
            // Older: replaced.
            assert!(
                matches!(decide("0.4.0", "0.4.1"), Verdict::Replace(w) if w.contains("older cua (0.4.0)"))
            );
            assert!(matches!(
                decide("0.4.1-beta.1", "0.4.1"),
                Verdict::Replace(_)
            ));
            // The same or newer: kept.
            assert!(
                matches!(decide("0.4.1", "0.4.1"), Verdict::Keep(w) if w.contains("cua 0.4.1"))
            );
            assert!(matches!(decide("0.5.0", "0.4.1"), Verdict::Keep(_)));
            // Unreadable: replaced.
            assert!(
                matches!(decide("dev", "0.4.1"), Verdict::Replace(w) if w.contains("cannot be read"))
            );
            // Its own daemon.
            let (exe, build) = id(own, crate::VERSION);
            assert_eq!(
                verdict_for(crate::VERSION, &exe, &build, own, crate::VERSION),
                Verdict::Own
            );
        }
        // The version is read from the build id when the field is empty.
        let (exe, build) = id(&swift, "0.5.0");
        assert!(matches!(
            verdict_for("", &exe, &build, &electron, "0.4.1"),
            Verdict::Keep(_)
        ));
        // This app's own executable rebuilt or updated since its daemon
        // started: replaced, whatever the version.
        let (exe, build) = id(&electron, "9.9.9");
        std::fs::write(&electron, b"updated").unwrap();
        assert!(
            matches!(verdict_for("9.9.9", &exe, &build, &electron, "0.4.1"), Verdict::Replace(w) if w.contains("rebuilt or updated"))
        );
    }

    #[test]
    fn a_missing_tool_names_the_daemon_and_the_way_out() {
        let info = cua_proto::daemon::v1::GetInfoResponse {
            version: "0.5.0".into(),
            pid: 7,
            executable: "/Applications/Cua Spaces.app/Contents/MacOS/cua".into(),
            ..Default::default()
        };
        let m = missing_tool_error("agent_keys.list", &info);
        assert!(m.starts_with("Spaces tool agent_keys.list: the running cua daemon (pid 7, cua 0.5.0, /Applications/Cua Spaces.app/Contents/MacOS/cua)"), "{m}");
        assert!(m.contains("cua daemon stop"), "{m}");
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

    #[test]
    fn the_electron_app_ships_its_cua_in_resources_native() {
        let dir = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "cua.exe" } else { "cua" };
        let touch = |p: &std::path::Path| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        };
        // macOS: `Contents/Resources/native/cua`, for the app, its helper
        // apps and the `cua` itself.
        let app = dir.path().join("Cua Spaces.app/Contents");
        let main = app.join("MacOS/Cua Spaces");
        let helper = app.join("Frameworks/Cua Spaces Helper.app/Contents/MacOS/Cua Spaces Helper");
        let cua = app.join("Resources/native").join(exe);
        touch(&main);
        touch(&helper);
        assert_eq!(bundled_cua_of(&main), None);
        touch(&cua);
        let want = cua.canonicalize().unwrap();
        assert_eq!(bundled_cua_of(&main), Some(want.clone()));
        assert_eq!(bundled_cua_of(&helper), Some(want.clone()));
        assert_eq!(bundled_cua_of(&cua), Some(want));
        // Windows and Linux: `resources/native/cua` beside the executable.
        let install = dir.path().join("Programs/Cua Spaces");
        let main = install.join(if cfg!(windows) {
            "Cua Spaces.exe"
        } else {
            "cua-spaces"
        });
        let cua = install.join("resources/native").join(exe);
        touch(&main);
        assert_eq!(bundled_cua_of(&main), None);
        touch(&cua);
        let want = cua.canonicalize().unwrap();
        assert_eq!(bundled_cua_of(&main), Some(want.clone()));
        assert_eq!(bundled_cua_of(&cua), Some(want));
        // A `cua` in some other `native` directory is not an app's.
        let loose = dir.path().join("build/native").join(exe);
        touch(&loose);
        assert_eq!(bundled_cua_of(&loose), None);
    }

    /// The Electron app's `cua` and the SwiftUI app's are different
    /// executables, so each app's `cua daemon start` replaces the other's
    /// daemon rather than reuse it.
    #[cfg(unix)]
    #[test]
    fn one_apps_daemon_is_a_stranger_to_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let swift = dir.path().join("Swift/Cua Spaces.app/Contents/MacOS/cua");
        let electron = dir
            .path()
            .join("Electron/Cua Spaces.app/Contents/Resources/native/cua");
        for p in [&swift, &electron] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"cua").unwrap();
        }
        let own = bundled_cua_of(&electron).unwrap();
        let theirs = of(&swift).unwrap();
        assert_eq!(
            stranger(&theirs.executable, &theirs.build_id, &own),
            Some(format!("it runs {}", theirs.executable))
        );
        let mine = of(&electron).unwrap();
        assert_eq!(stranger(&mine.executable, &mine.build_id, &own), None);
    }
}
