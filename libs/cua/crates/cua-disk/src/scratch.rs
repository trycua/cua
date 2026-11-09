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

/// Trajectories kept at most this big (`CUA_TRAJECTORY_MAX`, a size).
pub const TRAJECTORY_MAX_BYTES: u64 = 2 << 30;
/// Trajectory sessions older than this are removed (`CUA_TRAJECTORY_DAYS`).
pub const TRAJECTORY_MAX_AGE: Duration = Duration::from_secs(30 * 86_400);
/// A session touched this recently is being recorded and is never removed.
pub const TRAJECTORY_ACTIVE: Duration = Duration::from_secs(3600);

/// The trajectory caps from the environment, else the defaults.
pub fn trajectory_caps() -> (u64, Duration) {
    let bytes = std::env::var("CUA_TRAJECTORY_MAX")
        .ok()
        .and_then(|v| cua_vmm::disk::parse_size(&v))
        .unwrap_or(TRAJECTORY_MAX_BYTES);
    let age = std::env::var("CUA_TRAJECTORY_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|d| Duration::from_secs(d * 86_400))
        .unwrap_or(TRAJECTORY_MAX_AGE);
    (bytes, age)
}

/// Removes trajectory sessions (`<root>/<machine>/<session>/`) older than
/// `max_age`, then the oldest ones until the rest fits `max_bytes`. A
/// session touched within [`TRAJECTORY_ACTIVE`] is kept. Returns what it
/// removed.
pub fn prune_trajectories(
    root: &Path,
    max_bytes: u64,
    max_age: Duration,
    now: SystemTime,
) -> Vec<PathBuf> {
    let mut sessions: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    let Ok(machines) = std::fs::read_dir(root) else {
        return vec![];
    };
    for m in machines.flatten() {
        if !m.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(m.path()) else {
            continue;
        };
        for s in rd.flatten() {
            if !s.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let path = s.path();
            let modified = newest_mtime(&path).unwrap_or(SystemTime::UNIX_EPOCH);
            sessions.push((modified, cua_vmm::disk::allocated_size(&path), path));
        }
    }
    sessions.sort_by_key(|s| s.0);
    let mut total: u64 = sessions.iter().map(|s| s.1).sum();
    let mut out = Vec::new();
    for (modified, size, path) in sessions {
        let age = now.duration_since(modified).unwrap_or_default();
        if age < TRAJECTORY_ACTIVE {
            continue;
        }
        if (age > max_age || total > max_bytes) && std::fs::remove_dir_all(&path).is_ok() {
            total = total.saturating_sub(size);
            out.push(path);
        }
    }
    out
}

fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut best = std::fs::metadata(dir).ok()?.modified().ok();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let m = if e.file_type().is_ok_and(|t| t.is_dir()) {
            newest_mtime(&e.path())
        } else {
            e.metadata().ok().and_then(|m| m.modified().ok())
        };
        if m > best {
            best = m;
        }
    }
    best
}

/// All prunes, for `cua cache prune` and the daemon.
pub fn prune_all(layout: &Layout) -> Vec<PathBuf> {
    let mut out = prune_dir(&layout.screenshots(), KEEP);
    out.extend(prune_temp(&std::env::temp_dir(), SystemTime::now()));
    let (bytes, age) = trajectory_caps();
    out.extend(prune_trajectories(
        &layout.trajectories(),
        bytes,
        age,
        SystemTime::now(),
    ));
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

    #[test]
    fn trajectories_age_out_and_shrink_to_the_cap_but_never_touch_a_live_session() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("trajectories");
        let now = SystemTime::now();
        let mk = |machine: &str, name: &str, age_days: u64, bytes: usize| {
            let dir = root.join(machine).join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join("shot.png");
            std::fs::write(&f, vec![1u8; bytes]).unwrap();
            let t = now - Duration::from_secs(age_days * 86_400);
            std::fs::File::options()
                .write(true)
                .open(&f)
                .unwrap()
                .set_modified(t)
                .unwrap();
            std::fs::File::open(&dir).unwrap().set_modified(t).unwrap();
            dir
        };
        let ancient = mk("a", "s1", 45, 10);
        let old_big = mk("a", "s2", 10, 200_000);
        let newer = mk("b", "s3", 2, 200_000);
        let live = mk("b", "s4", 0, 200_000);
        let removed = prune_trajectories(&root, 450_000, TRAJECTORY_MAX_AGE, now);
        assert!(removed.contains(&ancient), "{removed:?}");
        assert!(
            removed.contains(&old_big),
            "over the cap, oldest first: {removed:?}"
        );
        assert!(newer.exists() && live.exists());
        // Nothing to do once it fits.
        assert!(prune_trajectories(&root, 450_000, TRAJECTORY_MAX_AGE, now).is_empty());
        // A cap of zero still keeps the live session.
        prune_trajectories(&root, 0, TRAJECTORY_MAX_AGE, now);
        assert!(live.exists() && !newer.exists());
    }
}
