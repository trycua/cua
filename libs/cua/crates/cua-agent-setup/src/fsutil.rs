//! File helpers: atomic writes that keep permissions and symlinks,
//! timestamped backups, and content hashes.

use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The file a write to `path` must land in: a symlinked config (dotfile
/// managers) is updated through the link instead of being replaced by a
/// regular file.
pub fn write_target(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    // Bounded: a symlink loop must not spin.
    for _ in 0..16 {
        match std::fs::read_link(&p) {
            Ok(t) => {
                p = if t.is_absolute() {
                    t
                } else {
                    p.parent().map(|d| d.join(&t)).unwrap_or(t)
                };
            }
            Err(_) => break,
        }
    }
    p
}

/// Writes `bytes` to `path` atomically: a temporary file in the same
/// directory, flushed, given the old file's permissions (0600 for a new
/// file on Unix, configs can hold secrets), then renamed over the target.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let target = write_target(path);
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let old_perms = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".into());
    let tmp = dir.join(format!(".{name}.cua-tmp-{}", std::process::id()));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        // New files are owner-only on Unix; Windows keeps the default ACL.
        #[cfg(unix)]
        let old_perms = old_perms.or_else(|| {
            use std::os::unix::fs::PermissionsExt;
            Some(std::fs::Permissions::from_mode(0o600))
        });
        if let Some(p) = old_perms {
            std::fs::set_permissions(&tmp, p)?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(&target, e));
    }
    Ok(())
}

/// Copies `path` to `<path>.cua-backup-<UTC timestamp>` and returns the
/// backup path. `None` when `path` does not exist.
pub fn backup(path: &Path) -> Result<Option<PathBuf>> {
    let target = write_target(path);
    if !target.is_file() {
        return Ok(None);
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut dest = target.with_file_name(format!("{name}.cua-backup-{stamp}"));
    // Two backups in the same second get a numeric suffix.
    let mut n = 1;
    while dest.exists() && n < 1000 {
        dest = target.with_file_name(format!("{name}.cua-backup-{stamp}-{n}"));
        n += 1;
    }
    std::fs::copy(&target, &dest).map_err(|e| Error::io(&target, e))?;
    Ok(Some(dest))
}

/// SHA-256 of bytes, hex.
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// SHA-256 over a directory tree (relative paths and contents, sorted).
/// `None` when `dir` is missing.
pub fn hash_dir(dir: &Path) -> Option<String> {
    if !dir.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    collect(dir, dir, &mut files, 0);
    files.sort();
    let mut h = Sha256::new();
    for (rel, bytes) in files {
        h.update(rel.as_bytes());
        h.update([0]);
        h.update(sha256(&bytes).as_bytes());
        h.update([0]);
    }
    Some(hex::encode(h.finalize()))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            collect(root, &p, out, depth + 1);
        } else if ft.is_file()
            && let Ok(bytes) = std::fs::read(&p)
        {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_keeps_mode_and_follows_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
            let link = d.path().join("link.json");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            atomic_write(&link, b"{\"a\":1}").unwrap();
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\":1}");
            let mode = std::fs::metadata(&real).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o640);
        }
        let fresh = d.path().join("sub/new.json");
        atomic_write(&fresh, b"x").unwrap();
        assert_eq!(std::fs::read(&fresh).unwrap(), b"x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // No temp files left behind.
        let names: Vec<_> = std::fs::read_dir(d.path().join("sub"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn backups_are_unique_and_skip_missing_files() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("c.toml");
        assert!(backup(&f).unwrap().is_none());
        std::fs::write(&f, "a = 1").unwrap();
        let b1 = backup(&f).unwrap().unwrap();
        let b2 = backup(&f).unwrap().unwrap();
        assert_ne!(b1, b2);
        assert_eq!(std::fs::read_to_string(b1).unwrap(), "a = 1");
    }

    #[test]
    fn dir_hash_changes_with_content() {
        let d = tempfile::tempdir().unwrap();
        assert!(hash_dir(&d.path().join("none")).is_none());
        std::fs::create_dir_all(d.path().join("s/r")).unwrap();
        std::fs::write(d.path().join("s/SKILL.md"), "a").unwrap();
        let h1 = hash_dir(&d.path().join("s")).unwrap();
        std::fs::write(d.path().join("s/r/x.md"), "b").unwrap();
        let h2 = hash_dir(&d.path().join("s")).unwrap();
        assert_ne!(h1, h2);
    }
}
