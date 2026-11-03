//! The cache on disk: one bare mirror per repository, a record of it, and
//! the files the Space reads (`.ok`, `.url`, `.fresh`, `gitconfig`, the
//! helper).
//!
//! ```text
//! <root>/m/<key>.git     the mirror (heads and tags)
//! <root>/m/<key>.json    its record (Entry)
//! <root>/m/<key>.url     the normalized URL it mirrors
//! <root>/m/<key>.ok      the size of each pack that passed verification
//! <root>/m/<key>.fresh   unix seconds the remote's tip was last confirmed
//! <root>/gitconfig       the system git config a Space includes
//! <root>/bin/git-remote-cuarepo
//! <root>/tmp/            fills in progress
//! <root>/home/           an empty home for the daemon's own git runs
//! <root>/stats.jsonl     what the last week saved
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One verified pack of a mirror.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    /// `pack-<hash>.pack`.
    pub name: String,
    /// Its size in bytes when it was verified.
    pub size: u64,
}

/// What the cache knows of one mirror.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The normalized URL.
    pub url: String,
    /// The key (16 hex digits).
    pub key: String,
    /// When the first fill finished (Unix ms).
    pub filled_ms: u64,
    /// When the remote's tip was last confirmed and the delta fetched.
    pub refreshed_ms: u64,
    /// When it was last used (a clone seen or reported; Unix ms): what
    /// least-recently-used eviction orders by.
    pub used_ms: u64,
    /// The mirror's size on disk.
    pub bytes: u64,
    /// How long the network clone that filled it took: what a repeat clone
    /// is credited with saving.
    pub fill_secs: f64,
    /// The commit `HEAD` pointed at when verified.
    pub head: String,
    /// The packs that passed verification.
    pub packs: Vec<Pack>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub fn m_dir(root: &Path) -> PathBuf {
    root.join("m")
}

pub fn mirror_dir(root: &Path, key: &str) -> PathBuf {
    m_dir(root).join(format!("{key}.git"))
}

pub fn side(root: &Path, key: &str, ext: &str) -> PathBuf {
    m_dir(root).join(format!("{key}.{ext}"))
}

pub fn tmp_dir(root: &Path) -> PathBuf {
    root.join("tmp")
}

pub fn home_dir(root: &Path) -> PathBuf {
    root.join("home")
}

/// Writes `data` to `path` through a rename, so a reader (a Space on the
/// read-only mount) never sees half a file.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The size of everything under `path` (symlinks are not followed).
pub fn dir_size(path: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            total += dir_size(&e.path());
        } else if let Ok(m) = e.metadata() {
            total += m.len();
        }
    }
    total
}

/// Every mirror with a record and a verification stamp, least recently used
/// first.
pub fn entries(root: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(m_dir(root)) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let key = name.strip_suffix(".json")?;
            let text = std::fs::read_to_string(e.path()).ok()?;
            let entry: Entry = serde_json::from_str(&text).ok()?;
            (entry.key == key && mirror_dir(root, key).is_dir() && side(root, key, "ok").is_file())
                .then_some(entry)
        })
        .collect();
    out.sort_by_key(|e| e.used_ms);
    out
}

pub fn entry(root: &Path, key: &str) -> Option<Entry> {
    entries(root).into_iter().find(|e| e.key == key)
}

/// Records `e` and the stamps a Space checks, last the `.ok` stamp (a mirror
/// without it is not used).
pub fn publish(root: &Path, e: &Entry, fresh: bool) -> std::io::Result<()> {
    let key = &e.key;
    write_atomic(&side(root, key, "url"), e.url.as_bytes())?;
    write_atomic(
        &side(root, key, "json"),
        serde_json::to_string(e)
            .map_err(std::io::Error::other)?
            .as_bytes(),
    )?;
    let ok: String = e
        .packs
        .iter()
        .map(|p| format!("{} {}\n", p.size, p.name))
        .collect();
    write_atomic(&side(root, key, "ok"), ok.as_bytes())?;
    if fresh {
        stamp_fresh(root, key)?;
    }
    Ok(())
}

pub fn stamp_fresh(root: &Path, key: &str) -> std::io::Result<()> {
    write_atomic(
        &side(root, key, "fresh"),
        format!("{}\n", now_ms() / 1000).as_bytes(),
    )
}

/// Records that `key` was used now.
pub fn touch(root: &Path, key: &str) {
    if let Some(mut e) = entry(root, key) {
        e.used_ms = now_ms();
        let _ = write_atomic(
            &side(root, key, "json"),
            serde_json::to_string(&e).unwrap_or_default().as_bytes(),
        );
    }
}

/// Removes one mirror and everything beside it. The stamp goes first, so a
/// Space stops using it before its files go.
pub fn drop_entry(root: &Path, key: &str) {
    let _ = std::fs::remove_file(side(root, key, "ok"));
    let _ = std::fs::remove_file(side(root, key, "fresh"));
    let trash = root.join("trash").join(format!("{key}-{}", now_ms()));
    let moved = std::fs::create_dir_all(root.join("trash"))
        .and_then(|()| std::fs::rename(mirror_dir(root, key), &trash))
        .is_ok();
    let _ = std::fs::remove_file(side(root, key, "json"));
    let _ = std::fs::remove_file(side(root, key, "url"));
    if moved {
        let _ = std::fs::remove_dir_all(&trash);
    } else {
        let _ = std::fs::remove_dir_all(mirror_dir(root, key));
    }
}

/// Removes every mirror, unfinished fill and stamp. The daemon's counters
/// stay.
pub fn clear(root: &Path) {
    if let Ok(rd) = std::fs::read_dir(m_dir(root)) {
        // Stamps first: nothing is used while it goes.
        let all: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        for p in all
            .iter()
            .filter(|p| p.extension().is_some_and(|x| x == "ok"))
        {
            let _ = std::fs::remove_file(p);
        }
        for p in all {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    for d in ["tmp", "trash"] {
        let _ = std::fs::remove_dir_all(root.join(d));
    }
}

/// Removes what an interrupted fill or eviction left.
pub fn sweep_leftovers(root: &Path) {
    for d in ["tmp", "trash"] {
        let _ = std::fs::remove_dir_all(root.join(d));
    }
    // A record or stamp without its mirror, a mirror without a record.
    let Ok(rd) = std::fs::read_dir(m_dir(root)) else {
        return;
    };
    let names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    for n in &names {
        let Some((key, ext)) = n.split_once('.') else {
            continue;
        };
        let has_mirror = names.iter().any(|m| *m == format!("{key}.git"));
        let has_record = names.iter().any(|m| *m == format!("{key}.json"));
        let orphan = match ext {
            "git" => !has_record,
            _ => !has_mirror,
        };
        if orphan {
            let p = m_dir(root).join(n);
            let _ = if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
        }
    }
}

/// Free bytes on the disk `path` is on.
pub fn free_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `c` is a valid C string and `s` is written by statvfs.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0 {
            return Some(s.f_bavail as u64 * s.f_frsize as u64);
        }
    }
    let _ = path;
    None
}
