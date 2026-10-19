//! Pinned install archives, fetched once on this machine and sent to a
//! local Space instead of every Space downloading them.
//!
//! A teleported app's archive (VS Code's is ~220 MB) used to be downloaded
//! inside each fresh Space, through the sandbox's network stack (gVisor's is
//! several times slower than the host's) and without byte progress. Now the
//! SDK keeps a host cache (`$CUA_HOME/cache/installables/<sha256>.<ext>`,
//! each file verified against the manifest's sha256 as it downloads) and
//! uploads the file over the Space's existing spacesd channel to
//! `~/.cua/tools/.incoming/`, where the install script picks it up and
//! verifies it again. A later Space gets it from the cache at loopback
//! speed. `cua cache ls|du|prune` account for the cache (category
//! `installables`) and evict it least recently used first.
//!
//! Only for local Spaces (a loopback spacesd): a cloud Space is closer to
//! the publisher's CDN than to this machine. `CUA_INSTALL_HOST_CACHE=1`
//! forces it on, `0` off. Any failure here is not fatal: the Space
//! downloads the archive itself, as before.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cua_agents::installables::StagedArchive;
use cua_spacesd_client::{UploadOptions, pb};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::{Error, Result};
use crate::space::Space;

/// Upper bound for one archive (the largest pinned app is a few hundred MB).
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The host cache directory.
pub fn cache_dir() -> PathBuf {
    crate::registry::cua_home()
        .join("cache")
        .join("installables")
}

/// Whether to stage through the host for a spacesd at `url`:
/// `CUA_INSTALL_HOST_CACHE` (`1`/`0`), else only for a loopback spacesd.
pub fn wanted(url: &url::Url) -> bool {
    match std::env::var("CUA_INSTALL_HOST_CACHE").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => is_loopback(url),
    }
}

fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `a` in the host cache under `dir`, downloaded (and sha256-verified while
/// it streams) when missing. `progress(done, total)`.
pub async fn fetch(
    dir: &Path,
    a: &StagedArchive,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<PathBuf> {
    let dest = dir.join(a.file_name());
    if tokio::fs::metadata(&dest).await.is_ok() {
        // A hit is a use: `cua cache prune` and the automatic cleanup evict
        // least recently used first and keep what was used within their
        // grace period (the upload that follows).
        touch(&dest);
        return Ok(dest);
    }
    tokio::fs::create_dir_all(dir).await?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| Error::Transfer(e.to_string()))?;
    let mut last = String::from("no mirror");
    for url in &a.urls {
        let part = dir.join(format!(
            "{}.part-{:08x}",
            a.file_name(),
            rand::random::<u32>()
        ));
        match download(&http, url, &part, &a.sha256, progress).await {
            Ok(()) => {
                tokio::fs::rename(&part, &dest).await?;
                return Ok(dest);
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                last = format!("{url}: {e}");
            }
        }
    }
    Err(Error::Transfer(format!("{} {}: {last}", a.id, a.version)))
}

/// Marks `path` used now (its mtime, which the cache accounting reads).
fn touch(path: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

async fn download(
    http: &reqwest::Client,
    url: &str,
    part: &Path,
    sha256: &str,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> Result<()> {
    let mut resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Transfer(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(Error::Transfer(format!("HTTP {}", resp.status())));
    }
    let total = resp.content_length().unwrap_or(0);
    if total > MAX_ARCHIVE_BYTES {
        return Err(Error::Transfer("archive too large".into()));
    }
    let mut file = tokio::fs::File::create(part).await?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut reported = 0u64;
    // Bounded by the size cap: every chunk adds bytes.
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| Error::Transfer(e.to_string()))?
    {
        done += chunk.len() as u64;
        if done > MAX_ARCHIVE_BYTES {
            return Err(Error::Transfer("archive too large".into()));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        if done - reported >= 4 << 20 {
            reported = done;
            progress(done, total);
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    progress(done, total.max(done));
    let got = hex::encode(hasher.finalize());
    if got != sha256 {
        return Err(Error::Transfer(format!(
            "sha256 mismatch: got {got}, pinned {sha256}"
        )));
    }
    Ok(())
}

/// One staging report line, with bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageEvent {
    pub detail: String,
    pub done: u64,
    pub total: u64,
}

impl Space {
    /// Sends the archives `ids` would download (for `arch`) from the host
    /// cache to the Space's `~/.cua/tools/.incoming`, skipping what the
    /// Space already has installed. Returns the ids staged.
    pub async fn stage_install_archives(
        &self,
        ids: &[&str],
        arch: &str,
        home: &str,
        events: tokio::sync::mpsc::UnboundedSender<StageEvent>,
    ) -> Result<Vec<String>> {
        let send = |detail: String, done: u64, total: u64| {
            let _ = events.send(StageEvent {
                detail,
                done,
                total,
            });
        };
        let archives = cua_agents::installables::staged_archives(ids, arch)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let dir = cache_dir();
        let mut staged = vec![];
        for a in archives {
            let [m1, m2] = a.markers(home);
            let have = self
                .bash(
                    &format!(
                        "test -f {} || test -f {}",
                        cua_agents::quote(&m1),
                        cua_agents::quote(&m2)
                    ),
                    Duration::from_secs(20),
                )
                .await?;
            if have.success() {
                continue;
            }
            let cached = tokio::fs::metadata(dir.join(a.file_name())).await.is_ok();
            let what = format!("{} {}", a.id, a.version);
            if !cached {
                send(format!("{what}: downloading to the host cache"), 0, 0);
            }
            let path = fetch(&dir, &a, &mut |d, t| {
                send(format!("{what}: downloading to the host cache"), d, t)
            })
            .await?;
            let size = tokio::fs::metadata(&path).await?.len();
            send(format!("{what}: sending to the Space"), 0, size);
            let counter = Arc::new(AtomicU64::new(0));
            let guest_path = a.guest_path(home);
            let upload = self.spacesd()?.upload(
                &guest_path,
                path.clone(),
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    create_parents: true,
                    permissions: 0o600,
                    progress: Some(counter.clone()),
                    ..Default::default()
                },
            );
            tokio::pin!(upload);
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            // Bounded: the upload future completes or fails.
            let result = loop {
                tokio::select! {
                    r = &mut upload => break r,
                    _ = tick.tick() => send(
                        format!("{what}: sending to the Space"),
                        counter.load(Ordering::Relaxed),
                        size,
                    ),
                }
            };
            let result = result?;
            if result.sha256 != a.sha256 {
                // A corrupt cache entry: drop it; the Space downloads.
                let _ = tokio::fs::remove_file(&path).await;
                return Err(Error::Transfer(format!(
                    "{what}: the host cache copy does not match the pin"
                )));
            }
            send(format!("{what}: sent"), size, size);
            staged.push(a.id.clone());
        }
        Ok(staged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_spaces_stage_through_the_host_by_default() {
        // SAFETY of the env read: this test does not set the variable.
        if std::env::var_os("CUA_INSTALL_HOST_CACHE").is_some() {
            return;
        }
        for (u, want) in [
            ("http://127.0.0.1:3211", true),
            ("http://localhost:3211", true),
            ("http://[::1]:3211", true),
            ("https://space-1.fleet.cua.ai", false),
            ("http://10.0.0.5:3211", false),
        ] {
            assert_eq!(wanted(&url::Url::parse(u).unwrap()), want, "{u}");
        }
    }

    /// A cached file is used as is (and marked used); a fetch with no reachable mirror fails
    /// without leaving partial files (hermetic: the mirror is a closed
    /// loopback port).
    #[tokio::test]
    async fn the_cache_is_reused_and_a_failed_fetch_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let a = StagedArchive {
            id: "x".into(),
            version: "1".into(),
            urls: vec!["http://127.0.0.1:9/x.deb".into()],
            sha256: "00".repeat(32),
            format: "deb".into(),
        };
        let err = fetch(dir.path(), &a, &mut |_, _| {}).await.unwrap_err();
        assert!(err.to_string().contains("x 1"), "{err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        let cached = dir.path().join(a.file_name());
        std::fs::write(&cached, b"cached").unwrap();
        // Last used long ago: a hit marks it used now, so the cache's LRU
        // cleanup (`cua cache prune`) keeps what Spaces still fetch.
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(30 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&cached)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let p = fetch(dir.path(), &a, &mut |_, _| {}).await.unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"cached");
        let used = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert!(used > long_ago + Duration::from_secs(86_400));
    }

    /// A download is verified against the pin while it streams: a
    /// mismatch keeps nothing (served by a one-shot loopback server).
    #[tokio::test]
    async fn a_download_that_does_not_match_the_pin_is_discarded() {
        let body = b"not the pinned bytes".to_vec();
        let good = hex::encode(Sha256::digest(&body));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = body.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for _ in 0..2 {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    served.len()
                );
                s.write_all(head.as_bytes()).await.unwrap();
                s.write_all(&served).await.unwrap();
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let mut a = StagedArchive {
            id: "x".into(),
            version: "1".into(),
            urls: vec![format!("http://127.0.0.1:{port}/x.deb")],
            sha256: "11".repeat(32),
            format: "deb".into(),
        };
        let err = fetch(dir.path(), &a, &mut |_, _| {}).await.unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        a.sha256 = good;
        let mut seen = vec![];
        let p = fetch(dir.path(), &a, &mut |d, t| seen.push((d, t)))
            .await
            .unwrap();
        assert_eq!(std::fs::read(p).unwrap(), body);
        assert_eq!(seen.last(), Some(&(body.len() as u64, body.len() as u64)));
    }
}
