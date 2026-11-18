//! Bounded scratch files: screenshots `cua do` saves without `--save`, and
//! the leftovers older versions wrote to the temp directory.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::layout::Layout;

/// Screenshots and snapshots kept (newest first).
pub const KEEP: usize = 20;
/// Temp-dir leftovers (`cua_screenshot_*.png`, `cua_snapshot_*.png`) older
/// than this are removed.
pub const STALE_TEMP: Duration = Duration::from_secs(24 * 3600);
/// File-name prefixes of the temp-dir leftovers.
pub const TEMP_PREFIXES: [&str; 2] = ["cua_screenshot_", "cua_snapshot_"];

/// A new file `<screenshots>/<prefix><stamp>.<ext>`; older files beyond
/// [`KEEP`] and stale temp leftovers are removed first.
pub fn new_file(layout: &Layout, prefix: &str, stamp: &str, ext: &str) -> std::io::Result<PathBuf> {
    let dir = layout.screenshots();
    std::fs::create_dir_all(&dir)?;
    prune_dir(&dir, KEEP.saturating_sub(1));
    prune_temp(&std::env::temp_dir(), SystemTime::now());
    let mut path = dir.join(format!("{prefix}{stamp}.{ext}"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{prefix}{stamp}-{n}.{ext}"));
    }
    Ok(path)
}

/// Keeps the `keep` newest files of `dir`; returns what it removed.
pub fn prune_dir(dir: &Path, keep: usize) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let doomed: Vec<PathBuf> = files.into_iter().skip(keep).map(|(_, p)| p).collect();
    for p in &doomed {
        let _ = std::fs::remove_file(p);
    }
    doomed
}

/// Removes temp-dir leftovers older than [`STALE_TEMP`] at `now`.
pub fn prune_temp(tmp: &Path, now: SystemTime) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(tmp) else {
        return vec![];
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !TEMP_PREFIXES.iter().any(|p| name.starts_with(p)) || !name.ends_with(".png") {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > STALE_TEMP);
        if old && e.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(e.path()).is_ok()
        {
            out.push(e.path());
        }
    }
    out
}

/// Both prunes, for `cua cache prune` and the daemon.
pub fn prune_all(layout: &Layout) -> Vec<PathBuf> {
    let mut out = prune_dir(&layout.screenshots(), KEEP);
    out.extend(prune_temp(&std::env::temp_dir(), SystemTime::now()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_are_bounded_and_temp_leftovers_age_out() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().join("home"));
        let t0 = SystemTime::now() - Duration::from_secs(10_000);
        let mut made = vec![];
        for i in 0..KEEP + 5 {
            let p = new_file(&l, "cua_screenshot_", &format!("s{i:02}"), "png").unwrap();
            std::fs::write(&p, b"png").unwrap();
            cua_vmm::disk::mark_used_at(&p, t0 + Duration::from_secs(i as u64));
            made.push(p);
        }
        let left = std::fs::read_dir(l.screenshots()).unwrap().count();
        assert_eq!(left, KEEP);
        assert!(made.last().unwrap().exists() && !made[0].exists());

        let tmp = d.path().join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let old = tmp.join("cua_snapshot_1.png");
        let new = tmp.join("cua_screenshot_2.png");
        let other = tmp.join("someone_else.png");
        for p in [&old, &new, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        let now = SystemTime::now();
        cua_vmm::disk::mark_used_at(&old, now - Duration::from_secs(3 * 86_400));
        cua_vmm::disk::mark_used_at(&other, now - Duration::from_secs(3 * 86_400));
        assert_eq!(prune_temp(&tmp, now), vec![old.clone()]);
        assert!(new.exists() && other.exists());
    }
}
