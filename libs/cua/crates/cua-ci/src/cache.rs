//! The host's cache of `actions/runner` tarballs:
//! `$CUA_HOME/ci/cache/actions-runner-<platform>-<version>.tar.gz`.
//!
//! The latest release is looked up at most once a day (`release.json`);
//! each tarball is downloaded once, checked against the SHA-256 GitHub
//! publishes for it (the asset digest, or the release notes for older
//! releases), and then copied into every Space that needs it. A tarball
//! without a published hash, or one that does not match, is refused.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::github::{GitHubClient, RunnerRelease};
use crate::{Error, Result};

/// The latest release is asked for again after this long.
const RELEASE_TTL_MS: u64 = 24 * 3600 * 1000;

/// A verified tarball on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tarball {
    /// Its path.
    pub path: PathBuf,
    /// Runner version.
    pub version: String,
    /// SHA-256 (hex).
    pub sha256: String,
}

/// Where the pool gets a runner tarball for a platform.
#[async_trait]
pub trait TarballSource: Send + Sync {
    /// A verified tarball for `platform` (`osx-arm64`, `linux-x64`, ...).
    async fn tarball(&self, platform: &str) -> Result<Tarball>;
}

/// The release, as fetched, with when.
#[derive(Serialize, Deserialize)]
struct CachedRelease {
    fetched_ms: u64,
    release: RunnerRelease,
}

/// Where releases come from (GitHub; a fake in tests).
#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// The latest `actions/runner` release.
    async fn latest(&self) -> Result<RunnerRelease>;
    /// Downloads `url` to `dest`; the SHA-256 (hex) of what was written.
    async fn download(&self, url: &str, dest: &Path) -> Result<String>;
}

#[async_trait]
impl ReleaseSource for GitHubClient {
    async fn latest(&self) -> Result<RunnerRelease> {
        self.latest_runner_release().await
    }

    async fn download(&self, url: &str, dest: &Path) -> Result<String> {
        GitHubClient::download(self, url, dest).await
    }
}

/// The tarball cache.
pub struct TarballCache {
    dir: PathBuf,
    source: Arc<dyn ReleaseSource>,
    lock: tokio::sync::Mutex<()>,
}

impl TarballCache {
    /// Under `dir`, filled from `source`.
    pub fn new(dir: PathBuf, source: Arc<dyn ReleaseSource>) -> Self {
        Self {
            dir,
            source,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    async fn release(&self, now_ms: u64) -> Result<RunnerRelease> {
        let path = self.dir.join("release.json");
        let cached: Option<CachedRelease> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(c) = &cached
            && now_ms.saturating_sub(c.fetched_ms) < RELEASE_TTL_MS
        {
            return Ok(c.release.clone());
        }
        match self.source.latest().await {
            Ok(release) => {
                cua_home::guard_write(&path)?;
                std::fs::create_dir_all(&self.dir)?;
                let body = serde_json::to_vec_pretty(&CachedRelease {
                    fetched_ms: now_ms,
                    release: release.clone(),
                })
                .map_err(|e| Error::Io(std::io::Error::other(e)))?;
                std::fs::write(&path, body)?;
                Ok(release)
            }
            // GitHub unreachable: the release we know still works.
            Err(e) => match cached {
                Some(c) => {
                    tracing::warn!(
                        "actions/runner release lookup failed ({e}); using {}",
                        c.release.version
                    );
                    Ok(c.release)
                }
                None => Err(e),
            },
        }
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
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

#[async_trait]
impl TarballSource for TarballCache {
    async fn tarball(&self, platform: &str) -> Result<Tarball> {
        let _one = self.lock.lock().await;
        let release = self.release(crate::unix_ms()).await?;
        let asset = release.asset(platform).ok_or_else(|| {
            Error::GitHub(format!(
                "actions/runner {} has no {platform} tarball",
                release.version
            ))
        })?;
        let want = asset.sha256.clone().ok_or_else(|| {
            Error::GitHub(format!(
                "actions/runner {} publishes no SHA-256 for {}; refusing an unverified runner",
                release.version, asset.name
            ))
        })?;
        let path = self.dir.join(&asset.name);
        if path.is_file() {
            let have = tokio::task::spawn_blocking({
                let p = path.clone();
                move || sha256_file(&p)
            })
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))??;
            if have == want {
                return Ok(Tarball {
                    path,
                    version: release.version.clone(),
                    sha256: want,
                });
            }
            tracing::warn!(
                "{} does not match its SHA-256; downloading it again",
                path.display()
            );
        }
        cua_home::guard_write(&path)?;
        std::fs::create_dir_all(&self.dir)?;
        let part = self.dir.join(format!("{}.part", asset.name));
        let got = self.source.download(&asset.url, &part).await;
        let got = match got {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        if got != want {
            let _ = std::fs::remove_file(&part);
            return Err(Error::GitHub(format!(
                "{} has SHA-256 {got}, but GitHub publishes {want}; refusing it",
                asset.name
            )));
        }
        std::fs::rename(&part, &path)?;
        // Older versions of the same platform are no longer needed.
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&format!("actions-runner-{platform}-")) && n != asset.name {
                let _ = std::fs::remove_file(e.path());
            }
        }
        Ok(Tarball {
            path,
            version: release.version.clone(),
            sha256: want,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::ReleaseAsset;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Fake {
        body: Vec<u8>,
        sha: Option<String>,
        latest_calls: AtomicU32,
        downloads: AtomicU32,
        fail_latest: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl ReleaseSource for Fake {
        async fn latest(&self) -> Result<RunnerRelease> {
            self.latest_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_latest.load(Ordering::SeqCst) {
                return Err(Error::GitHub("offline".into()));
            }
            Ok(RunnerRelease {
                version: "2.338.2".into(),
                assets: vec![ReleaseAsset {
                    name: "actions-runner-osx-arm64-2.338.2.tar.gz".into(),
                    url: "https://example.test/osx".into(),
                    platform: "osx-arm64".into(),
                    sha256: self.sha.clone(),
                }],
            })
        }

        async fn download(&self, _url: &str, dest: &Path) -> Result<String> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            std::fs::write(dest, &self.body)?;
            Ok(sha256_file(dest)?)
        }
    }

    fn fake(body: &[u8], sha: Option<String>) -> Arc<Fake> {
        Arc::new(Fake {
            body: body.to_vec(),
            sha,
            latest_calls: AtomicU32::new(0),
            downloads: AtomicU32::new(0),
            fail_latest: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn sha(b: &[u8]) -> String {
        use sha2::Digest as _;
        hex::encode(sha2::Sha256::digest(b))
    }

    #[tokio::test]
    async fn downloads_once_verified_then_reuses() {
        let d = tempfile::tempdir().unwrap();
        let f = fake(b"tarball", Some(sha(b"tarball")));
        let c = TarballCache::new(d.path().to_path_buf(), f.clone());
        let t = c.tarball("osx-arm64").await.unwrap();
        assert_eq!(t.version, "2.338.2");
        assert_eq!(std::fs::read(&t.path).unwrap(), b"tarball");
        let again = c.tarball("osx-arm64").await.unwrap();
        assert_eq!(again, t);
        assert_eq!(f.downloads.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.latest_calls.load(Ordering::SeqCst),
            1,
            "release.json is fresh"
        );
        // GitHub unreachable later: the known release still serves.
        let stale = CachedRelease {
            fetched_ms: 0,
            release: f.latest().await.unwrap(),
        };
        std::fs::write(
            d.path().join("release.json"),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        f.fail_latest.store(true, Ordering::SeqCst);
        assert_eq!(c.tarball("osx-arm64").await.unwrap(), t);
        // A tarball edited on disk is downloaded again.
        std::fs::write(&t.path, b"tampered").unwrap();
        c.tarball("osx-arm64").await.unwrap();
        assert_eq!(f.downloads.load(Ordering::SeqCst), 2);
        assert!(
            c.tarball("linux-x64")
                .await
                .unwrap_err()
                .to_string()
                .contains("no linux-x64")
        );
    }

    #[tokio::test]
    async fn a_mismatch_or_a_missing_hash_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let c = TarballCache::new(d.path().to_path_buf(), fake(b"evil", Some(sha(b"good"))));
        let e = c.tarball("osx-arm64").await.unwrap_err().to_string();
        assert!(e.contains("refusing it"), "{e}");
        assert!(
            !d.path()
                .join("actions-runner-osx-arm64-2.338.2.tar.gz")
                .exists()
        );
        assert!(
            !d.path()
                .join("actions-runner-osx-arm64-2.338.2.tar.gz.part")
                .exists()
        );
        let d = tempfile::tempdir().unwrap();
        let c = TarballCache::new(d.path().to_path_buf(), fake(b"x", None));
        let e = c.tarball("osx-arm64").await.unwrap_err().to_string();
        assert!(e.contains("unverified"), "{e}");
    }
}
