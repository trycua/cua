//! SHA-256 of files, remembered by path, size and modification time so a
//! checkpoint exported again (or a base disk shared by many checkpoints) is
//! not read again.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;

/// What identifies a file's content without reading it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// Size in bytes.
    pub size: u64,
    /// Modification time, nanoseconds since the epoch.
    pub mtime_ns: u128,
}

impl Stamp {
    /// The stamp of `path` now.
    pub fn of(path: &Path) -> std::io::Result<Stamp> {
        let m = std::fs::metadata(path)?;
        let mtime_ns = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Ok(Stamp {
            size: m.len(),
            mtime_ns,
        })
    }
}

/// SHA-256 of `path` (hex), reading it once.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    files: BTreeMap<PathBuf, (Stamp, String)>,
}

/// Hashes `paths` (on blocking threads, a few at once), using and updating
/// the cache file `cache`. Returns `(sha256, stamp)` per path, in order.
pub async fn hash_files(cache: &Path, paths: &[PathBuf]) -> Result<Vec<(String, Stamp)>> {
    let mut c: Cache = std::fs::read(cache)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(paths.len());
    let mut todo = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let stamp = Stamp::of(p)?;
        match c.files.get(p) {
            Some((s, sha)) if *s == stamp => out.push(Some((sha.clone(), stamp))),
            _ => {
                out.push(None);
                todo.push((i, p.clone(), stamp));
            }
        }
    }
    // A few files at once: each is a sequential read, and disks (and
    // SHA-256 on one core) are faster with some parallelism.
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut tasks = Vec::new();
    for (i, p, stamp) in todo {
        let sem = sem.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            let p2 = p.clone();
            let sha = tokio::task::spawn_blocking(move || sha256_file(&p2))
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))??;
            // A file that changed while it was read is hashed again next
            // time (its stamp is not trusted).
            let after = Stamp::of(&p)?;
            Ok::<_, std::io::Error>((i, p, sha, stamp, after))
        }));
    }
    for t in tasks {
        let (i, p, sha, stamp, after) = t
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))??;
        if after == stamp {
            c.files.insert(p, (stamp.clone(), sha.clone()));
        }
        out[i] = Some((sha, stamp));
    }
    // Forget files that are gone.
    c.files.retain(|p, _| p.exists());
    if let Some(d) = cache.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = cache.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(&c)?)?;
    std::fs::rename(&tmp, cache)?;
    Ok(out
        .into_iter()
        .map(|o| o.expect("every file hashed"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_are_cached_until_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        std::fs::write(&a, b"hello").unwrap();
        let cache = dir.path().join("cache.json");
        let h = hash_files(&cache, std::slice::from_ref(&a)).await.unwrap();
        assert_eq!(
            h[0].0,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(
            std::fs::read_to_string(&cache)
                .unwrap()
                .contains("2cf24dba")
        );
        // Same stamp: the cached hash, even if the content were different
        // (here the cache entry is edited to prove it is used).
        let raw = std::fs::read_to_string(&cache)
            .unwrap()
            .replace("2cf24dba", "00000000");
        std::fs::write(&cache, raw).unwrap();
        let h2 = hash_files(&cache, std::slice::from_ref(&a)).await.unwrap();
        assert!(h2[0].0.starts_with("00000000"));
        // A changed size changes the stamp: hashed again.
        std::fs::write(&a, b"hello!").unwrap();
        let h3 = hash_files(&cache, std::slice::from_ref(&a)).await.unwrap();
        assert_eq!(h3[0].0, sha256_file(&a).unwrap());
        assert_eq!(h3[0].1.size, 6);
    }
}
