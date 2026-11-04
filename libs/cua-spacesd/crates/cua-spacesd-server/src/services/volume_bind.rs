// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Named volumes in the guest: folders of the mounted Cua Volume made
//! available at other paths (`AttachVolumeRequest.binds`).
//!
//! - Linux: a bind mount of `<mount root>/<source>` at the target, read-only
//!   when asked, made as root: by `cua-spacesd volume-bind` (this process is
//!   root, or through `sudo -n`) or by the root volume helper, the way the
//!   FUSE mount itself is. Undone before the volume unmounts.
//! - macOS: a symbolic link at the target to the folder under the NFS mount.
//!   A link cannot be read-only; the client's view of the folder is (its
//!   writes fail with a permission error).
//! - Windows: not supported (`volume.mount` is a preview there).

use std::path::{Component, Path, PathBuf};

use cua_proto::env::v1::VolumeBind;

/// Most binds one attach may carry.
pub const MAX_BINDS: usize = 32;

/// One planned bind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    /// The folder, under the volume's mount point.
    pub source: PathBuf,
    /// Where it appears.
    pub target: PathBuf,
    pub read_only: bool,
}

/// Checks `binds` and resolves each source under `mount_root`.
pub fn plan(mount_root: &Path, binds: &[VolumeBind]) -> Result<Vec<Bind>, String> {
    if binds.len() > MAX_BINDS {
        return Err(format!(
            "{} binds; at most {MAX_BINDS} per Space",
            binds.len()
        ));
    }
    let mut out: Vec<Bind> = vec![];
    for b in binds {
        let rel = Path::new(b.source.trim_matches('/'));
        if b.source.trim_matches('/').is_empty()
            || !rel.components().all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "bind source {:?}: use a folder of the volume without `.` or `..`",
                b.source
            ));
        }
        let target = PathBuf::from(&b.target);
        if !target.is_absolute()
            || !target
                .components()
                .all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
            || b.target.split('/').any(|seg| seg == "." || seg == "..")
            || target == Path::new("/")
        {
            return Err(format!(
                "bind target {:?}: use an absolute path without `.` or `..`",
                b.target
            ));
        }
        if target.starts_with(mount_root) || mount_root.starts_with(&target) {
            return Err(format!(
                "bind target {:?} overlaps the volume's own mount point {}",
                b.target,
                mount_root.display()
            ));
        }
        if let Some(other) = out
            .iter()
            .find(|o| target.starts_with(&o.target) || o.target.starts_with(&target))
        {
            return Err(format!(
                "bind targets {} and {} overlap",
                other.target.display(),
                target.display()
            ));
        }
        out.push(Bind {
            source: mount_root.join(rel),
            target,
            read_only: b.read_only,
        });
    }
    Ok(out)
}

/// Makes the binds. Folders that are missing in the volume are created
/// first when the bind is writable. On failure nothing is left in place.
pub async fn apply(binds: &[Bind]) -> Result<Vec<PathBuf>, String> {
    let mut done: Vec<PathBuf> = vec![];
    for b in binds {
        let r = apply_one(b).await;
        match r {
            Ok(()) => done.push(b.target.clone()),
            Err(e) => {
                release(&done).await;
                return Err(e);
            }
        }
    }
    Ok(done)
}

async fn apply_one(b: &Bind) -> Result<(), String> {
    let src = b.source.clone();
    let writable = !b.read_only;
    // The folder lives in the volume: ask the client for it (blocking calls
    // on the mount, so off the async runtime).
    tokio::task::spawn_blocking(move || {
        if src.is_dir() {
            Ok(())
        } else if writable {
            std::fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))
        } else {
            Err(format!(
                "{}: the folder does not exist in the volume (a read-only mount cannot create it)",
                src.display()
            ))
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    platform::bind(b).await
}

/// Undoes the binds at `targets` (best effort).
pub async fn release(targets: &[PathBuf]) {
    for t in targets {
        if let Err(e) = platform::unbind(t).await {
            tracing::warn!(target = %t.display(), error = %e, "volume bind did not end");
        }
    }
}

/// Undoes what a previous driver left (start-up).
pub async fn clear_stale() {
    platform::clear().await;
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(30);

    fn root() -> bool {
        // SAFETY: geteuid never fails.
        unsafe { libc::geteuid() == 0 }
    }

    /// Whether `exe` is the cua-spacesd binary, the only one with a
    /// `volume-bind` subcommand.
    pub(crate) fn is_spacesd(exe: &Path) -> bool {
        exe.file_stem()
            .is_some_and(|n| n.to_string_lossy().starts_with("cua-spacesd"))
    }

    async fn run_self(args: &[String]) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // An embedder (the language-binding fixtures, a test harness) links
        // this server core into its own binary: running that again with
        // `volume-bind` starts another copy of the embedder, which does the
        // same, without end.
        if !is_spacesd(&exe) {
            return Err(format!(
                "{} is not cua-spacesd, which runs volume-bind",
                exe.display()
            ));
        }
        let mut cmd = if root() {
            tokio::process::Command::new(&exe)
        } else {
            let mut c = tokio::process::Command::new("sudo");
            c.arg("-n").arg(&exe);
            c
        };
        let out = tokio::time::timeout(
            TIMEOUT,
            cmd.arg("volume-bind")
                .args(args)
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| "volume-bind timed out".to_string())?
        .map_err(|e| format!("volume-bind: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    fn via_helper() -> bool {
        !root() && Path::new(super::super::volume::HELPER_SOCKET).exists()
    }

    pub async fn bind(b: &Bind) -> Result<(), String> {
        if via_helper() {
            return super::super::volume::helper_call(serde_json::json!({
                "op": "bind", "source": b.source, "path": b.target, "read_only": b.read_only,
            }))
            .await;
        }
        let mut args = vec![
            "--source".to_string(),
            b.source.to_string_lossy().into_owned(),
            "--target".into(),
            b.target.to_string_lossy().into_owned(),
        ];
        if b.read_only {
            args.push("--read-only".into());
        }
        run_self(&args).await
    }

    pub async fn unbind(target: &Path) -> Result<(), String> {
        if via_helper() {
            return super::super::volume::helper_call(
                serde_json::json!({"op": "unbind", "path": target}),
            )
            .await;
        }
        run_self(&[
            "--unbind".into(),
            "--target".into(),
            target.to_string_lossy().into_owned(),
        ])
        .await
    }

    pub async fn clear() {
        let r = if via_helper() {
            super::super::volume::helper_call(serde_json::json!({"op": "unbind_all"})).await
        } else if root() || which_sudo() {
            run_self(&["--clear".into()]).await
        } else {
            Ok(())
        };
        if let Err(e) = r {
            tracing::debug!(error = %e, "no stale volume binds cleared");
        }
    }

    fn which_sudo() -> bool {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("sudo").is_file()))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub async fn bind(b: &Bind) -> Result<(), String> {
        let (src, target) = (b.source.clone(), b.target.clone());
        tokio::task::spawn_blocking(move || link(&src, &target))
            .await
            .map_err(|e| e.to_string())?
    }

    /// A link at `target` to `source`, replacing a link or an empty folder.
    fn link(source: &Path, target: &Path) -> Result<(), String> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        match std::fs::symlink_metadata(target) {
            Ok(m) if m.file_type().is_symlink() => {
                std::fs::remove_file(target).map_err(|e| format!("{}: {e}", target.display()))?
            }
            Ok(m) if m.is_dir() => std::fs::remove_dir(target).map_err(|_| {
                format!(
                    "{} is a folder with files in it; choose another path",
                    target.display()
                )
            })?,
            Ok(_) => return Err(format!("{} exists and is a file", target.display())),
            Err(_) => {}
        }
        std::os::unix::fs::symlink(source, target)
            .map_err(|e| format!("link {}: {e}", target.display()))
    }

    pub async fn unbind(target: &Path) -> Result<(), String> {
        let t = target.to_path_buf();
        tokio::task::spawn_blocking(move || {
            // Only a link we made (to somewhere in a Cua Volume).
            if std::fs::symlink_metadata(&t).is_ok_and(|m| m.file_type().is_symlink()) {
                std::fs::remove_file(&t).map_err(|e| format!("{}: {e}", t.display()))?;
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    pub async fn clear() {}
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::*;

    pub async fn bind(_: &Bind) -> Result<(), String> {
        Err("named volumes are not supported on this system".into())
    }

    pub async fn unbind(_: &Path) -> Result<(), String> {
        Ok(())
    }

    pub async fn clear() {}
}

// Every test here is Unix (binds are POSIX paths in a Unix guest).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn b(source: &str, target: &str, ro: bool) -> VolumeBind {
        VolumeBind {
            source: source.into(),
            target: target.into(),
            read_only: ro,
        }
    }

    // Volume binds exist on Linux and macOS guests only; these plans use
    // Unix absolute paths (`/mnt/x`), which are not absolute on Windows.
    #[cfg(unix)]
    #[test]
    fn the_plan_resolves_sources_under_the_mount() {
        let root = Path::new("/volume");
        let p = plan(
            root,
            &[
                b("volumes/shared/", "/mnt/shared", true),
                b("/volumes/data/in", "/data", false),
            ],
        )
        .unwrap();
        assert_eq!(
            p,
            vec![
                Bind {
                    source: "/volume/volumes/shared".into(),
                    target: "/mnt/shared".into(),
                    read_only: true
                },
                Bind {
                    source: "/volume/volumes/data/in".into(),
                    target: "/data".into(),
                    read_only: false
                },
            ]
        );
        assert!(plan(root, &[]).unwrap().is_empty());
    }

    // Volume binds exist on Linux and macOS guests only; these plans use
    // Unix absolute paths (`/mnt/x`), which are not absolute on Windows.
    #[cfg(unix)]
    #[test]
    fn the_plan_refuses_escapes_and_overlaps() {
        let root = Path::new("/volume");
        let bad = |binds: &[VolumeBind]| plan(root, binds).unwrap_err();
        assert!(bad(&[b("../etc", "/mnt/x", false)]).contains("without"));
        assert!(bad(&[b("volumes/a/../../x", "/mnt/x", false)]).contains("without"));
        assert!(bad(&[b("", "/mnt/x", false)]).contains("folder of the volume"));
        assert!(bad(&[b("volumes/a", "relative", false)]).contains("absolute"));
        assert!(bad(&[b("volumes/a", "/mnt/../etc", false)]).contains("absolute"));
        assert!(bad(&[b("volumes/a", "/", false)]).contains("absolute"));
        assert!(bad(&[b("volumes/a", "/mnt/./x", false)]).contains("absolute"));
        assert!(bad(&[b("volumes/a", "/volume/x", false)]).contains("overlaps"));
        assert!(bad(&[
            b("volumes/a", "/mnt/x", false),
            b("volumes/b", "/mnt/x/y", true)
        ])
        .contains("overlap"));
        let many: Vec<VolumeBind> = (0..=MAX_BINDS)
            .map(|i| b("volumes/a", &format!("/mnt/{i}"), false))
            .collect();
        assert!(bad(&many).contains("at most"));
        // Two binds of one volume at different places are fine.
        assert!(plan(
            root,
            &[
                b("volumes/a", "/mnt/a1", false),
                b("volumes/a", "/mnt/a2", true)
            ]
        )
        .is_ok());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_link_stands_in_for_a_bind_on_macos() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("vol/volumes/shared");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.txt"), "hi").unwrap();
        let target = t.path().join("deep/er/shared");
        let bind = Bind {
            source: src.clone(),
            target: target.clone(),
            read_only: false,
        };
        let done = apply(std::slice::from_ref(&bind)).await.unwrap();
        assert_eq!(done, vec![target.clone()]);
        assert_eq!(std::fs::read_to_string(target.join("a.txt")).unwrap(), "hi");
        // Again: replaced.
        apply(std::slice::from_ref(&bind)).await.unwrap();
        // A folder with files at the target is not replaced.
        release(&done).await;
        assert!(!target.exists());
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("mine"), "x").unwrap();
        assert!(apply(std::slice::from_ref(&bind))
            .await
            .unwrap_err()
            .contains("choose another"));
        // A missing writable source is created; a read-only one is an error.
        let new = Bind {
            source: t.path().join("vol/volumes/new"),
            target: t.path().join("n"),
            read_only: false,
        };
        apply(std::slice::from_ref(&new)).await.unwrap();
        assert!(new.source.is_dir());
        let ro = Bind {
            source: t.path().join("vol/volumes/none"),
            target: t.path().join("r"),
            read_only: true,
        };
        assert!(apply(&[ro]).await.unwrap_err().contains("does not exist"));
    }

    /// Only the cua-spacesd binary has a `volume-bind` subcommand: an
    /// embedder that links this core in must not be run again with it.
    #[cfg(target_os = "linux")]
    #[test]
    fn only_cua_spacesd_runs_volume_bind() {
        use super::platform::is_spacesd;
        assert!(is_spacesd(Path::new("/usr/local/bin/cua-spacesd")));
        assert!(!is_spacesd(Path::new("/x/target/debug/cua-test-fixtures")));
        assert!(!is_spacesd(Path::new("/x/target/debug/deps/watch-1a2b")));
    }
}
