//! Content-addressed local image cache (`~/.cua/images`, or `$CUA_HOME/images`).
//!
//! ```text
//! ~/.cua/images/
//!   blobs/sha256/<hex>                 verified registry blobs (layers, configs)
//!   disks/<manifest-hex>/disk.qcow2    extracted containerDisk disks
//!   rootfs/<manifest-hex>/             unpacked rootfs trees
//!   refs/<registry>/<repo>/<tag>.json  last resolved digest per tag (informational)
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::digest::hex_of;
use crate::error::{ImageError, Result};

#[derive(Clone, Debug)]
pub struct ImageCache {
    root: PathBuf,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new(cua_vmm::host::cua_home().join("images"))
    }
}

impl ImageCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn hex(digest: &str) -> Result<&str> {
        hex_of(digest).ok_or_else(|| ImageError::Registry(format!("unsupported digest '{digest}'")))
    }

    pub fn blob_path(&self, digest: &str) -> Result<PathBuf> {
        Ok(self
            .root
            .join("blobs")
            .join("sha256")
            .join(Self::hex(digest)?))
    }

    pub fn has_blob(&self, digest: &str) -> bool {
        self.blob_path(digest).is_ok_and(|p| p.exists())
    }

    pub fn disk_dir(&self, manifest_digest: &str) -> Result<PathBuf> {
        Ok(self.root.join("disks").join(Self::hex(manifest_digest)?))
    }

    pub fn rootfs_dir(&self, manifest_digest: &str) -> Result<PathBuf> {
        Ok(self.root.join("rootfs").join(Self::hex(manifest_digest)?))
    }

    /// Record `reference → digest` for inspection (`cua image ls`).
    pub fn record_ref(&self, reference: &str, digest: &str) -> Result<()> {
        let safe: String = reference
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let dir = self.root.join("refs");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join(format!("{safe}.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({ "reference": reference, "digest": digest }),
            )?,
        )?;
        Ok(())
    }

    /// All recorded references.
    pub fn refs(&self) -> Vec<(String, String)> {
        let Ok(rd) = std::fs::read_dir(self.root.join("refs")) else {
            return vec![];
        };
        rd.flatten()
            .filter_map(|e| {
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
                Some((
                    v.get("reference")?.as_str()?.to_string(),
                    v.get("digest")?.as_str()?.to_string(),
                ))
            })
            .collect()
    }
}

/// An exclusive cross-process lock file (`O_EXCL`), removed on drop.
pub struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    /// Acquire `path`, waiting while another process holds it. Returns `None`
    /// when `done()` becomes true while waiting (the other process finished
    /// the work). Stale locks older than `stale` are broken.
    pub async fn acquire(
        path: &Path,
        done: impl Fn() -> bool,
        stale: Duration,
    ) -> Result<Option<Self>> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(_) => {
                    return Ok(Some(Self {
                        path: path.to_path_buf(),
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if done() {
                        return Ok(None);
                    }
                    let age = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .unwrap_or_default();
                    if age > stale {
                        tracing::warn!(lock = %path.display(), "breaking stale cache lock");
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn paths_are_content_addressed() {
        let c = ImageCache::new("/c");
        assert_eq!(
            c.blob_path(D).unwrap(),
            Path::new(&format!("/c/blobs/sha256/{}", &D[7..]))
        );
        assert!(c.blob_path("md5:abc").is_err());
        assert!(c.disk_dir(D).unwrap().starts_with("/c/disks"));
    }

    #[test]
    fn refs_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let c = ImageCache::new(d.path());
        c.record_ref("ghcr.io/trycua/x:1", D).unwrap();
        assert_eq!(
            c.refs(),
            vec![("ghcr.io/trycua/x:1".to_string(), D.to_string())]
        );
    }

    #[tokio::test]
    async fn lock_is_exclusive_and_released() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.lock");
        let l = CacheLock::acquire(&p, || false, Duration::from_secs(60))
            .await
            .unwrap()
            .unwrap();
        // A second waiter sees the work finished and returns None.
        assert!(
            CacheLock::acquire(&p, || true, Duration::from_secs(60))
                .await
                .unwrap()
                .is_none()
        );
        drop(l);
        assert!(!p.exists());
        // Stale locks are broken.
        std::fs::write(&p, b"").unwrap();
        assert!(
            CacheLock::acquire(&p, || false, Duration::ZERO)
                .await
                .unwrap()
                .is_some()
        );
    }
}
