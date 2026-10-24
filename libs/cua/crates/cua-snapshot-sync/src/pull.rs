//! Fetching the files of a manifest this host does not have yet.
//!
//! Each missing file is asked of the sources in order (the hosts that have
//! it, then the fallback). A download goes to `partial/<sha256>` and
//! resumes from its length (a range request), so a copy cut off by a
//! network drop or a restart goes on where it stopped. Every file's SHA-256
//! and size are checked before it is kept; one that does not match is
//! thrown away and asked of the next source. A few files download at once.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use tokio::io::AsyncWriteExt as _;

use crate::store::Store;
use crate::{Error, Manifest, Progress, ProgressFn, Result};

/// Files downloading at once.
pub const PARALLEL_FILES: usize = 4;

/// Somewhere a file can come from.
#[async_trait::async_trait]
pub trait BlobSource: Send + Sync {
    /// For progress reports: a host's name, `s3` or `volume`.
    fn label(&self) -> String;
    /// Appends the bytes of blob `sha` (of `size` bytes) to `partial`
    /// after the bytes already there, calling `on_bytes` as they arrive.
    async fn fetch(
        &self,
        sha: &str,
        size: u64,
        partial: &Path,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<()>;
}

/// A host that has the files ([`crate::serve`]).
pub struct HttpSource {
    /// Base URL (`http://<host>:<port>/v1/snapshots`).
    pub url: String,
    /// The read grant.
    pub grant: String,
    /// The host's name.
    pub name: String,
    client: reqwest::Client,
}

impl HttpSource {
    /// A source at `url` with `grant`.
    pub fn new(url: &str, grant: &str, name: &str) -> Result<HttpSource> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // A stalled transfer fails (and the next source is asked)
            // instead of hanging.
            .read_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Unavailable(format!("http client: {e}")))?;
        Ok(HttpSource {
            url: url.trim_end_matches('/').to_string(),
            grant: grant.to_string(),
            name: name.to_string(),
            client,
        })
    }
}

/// Opens `partial` for appending, returning how many bytes it has (a file
/// longer than `size` is started over).
async fn open_partial(partial: &Path, size: u64) -> Result<(tokio::fs::File, u64)> {
    let have = tokio::fs::metadata(partial)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut o = tokio::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    o.mode(0o600);
    if have > size {
        tokio::fs::remove_file(partial).await?;
        return Ok((o.open(partial).await?, 0));
    }
    Ok((o.open(partial).await?, have))
}

#[async_trait::async_trait]
impl BlobSource for HttpSource {
    fn label(&self) -> String {
        self.name.clone()
    }

    async fn fetch(
        &self,
        sha: &str,
        size: u64,
        partial: &Path,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<()> {
        let (mut f, have) = open_partial(partial, size).await?;
        if have == size {
            return Ok(());
        }
        let resp = self
            .client
            .get(format!("{}/blobs/{sha}", self.url))
            .bearer_auth(&self.grant)
            .header(reqwest::header::RANGE, format!("bytes={have}-"))
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("{}: {e}", self.name)))?;
        let status = resp.status();
        match status.as_u16() {
            206 => {}
            // The whole file (no range support): start over.
            200 => {
                drop(f);
                tokio::fs::remove_file(partial).await?;
                f = open_partial(partial, size).await?.0;
            }
            401 | 403 => {
                let body = resp.text().await.unwrap_or_default();
                return Err(Error::Denied(format!("{}: {body}", self.name)));
            }
            _ => {
                let body = resp.text().await.unwrap_or_default();
                return Err(Error::Unavailable(format!(
                    "{}: HTTP {status}: {body}",
                    self.name
                )));
            }
        }
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::Unavailable(format!("{}: {e}", self.name)))?;
            f.write_all(&chunk).await?;
            on_bytes(chunk.len() as u64);
        }
        f.flush().await?;
        f.sync_all().await?;
        Ok(())
    }
}

/// The lock of blob `sha` in `store` (process-wide).
fn blob_lock(store: &Store, sha: &str) -> Arc<tokio::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static LOCKS: LazyLock<Mutex<HashMap<std::path::PathBuf, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Default::default);
    let mut m = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    // Forget locks nobody holds or waits for.
    m.retain(|_, l| Arc::strong_count(l) > 1);
    m.entry(store.blob_path(sha)).or_default().clone()
}

/// What a pull did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PullReport {
    /// Bytes downloaded.
    pub bytes_done: u64,
    /// Bytes this host had already.
    pub bytes_reused: u64,
    /// Where each downloaded blob came from (sha256 to source label).
    pub from: std::collections::BTreeMap<String, String>,
}

/// Fetches every file of `m` this host does not have, from `sources` in
/// order, verifying each. Reports progress at most every 250 ms (and at
/// the end, `done` and `verified`).
pub async fn pull(
    store: &Store,
    m: &Manifest,
    sources: Vec<Arc<dyn BlobSource>>,
    progress: Option<ProgressFn>,
) -> Result<PullReport> {
    m.validate()?;
    let digest = m.digest();
    let mut missing = Vec::new();
    let mut reused = 0;
    for (sha, size) in m.blobs() {
        if store.has_blob(&sha, size) {
            reused += size;
        } else {
            missing.push((sha, size));
        }
    }
    let total: u64 = missing.iter().map(|(_, s)| *s).sum();
    let done = Arc::new(AtomicU64::new(0));
    let last = Arc::new(std::sync::Mutex::new((Instant::now(), String::new())));
    let report = {
        let done = done.clone();
        let progress = progress.clone();
        let name = m.name.clone();
        let digest = digest.clone();
        let last = last.clone();
        Arc::new(move |from: &str, force: bool| {
            let Some(p) = &progress else { return };
            let mut l = last.lock().unwrap();
            if !force && l.0.elapsed() < Duration::from_millis(250) && l.1 == from {
                return;
            }
            *l = (Instant::now(), from.to_string());
            p(Progress {
                name: name.clone(),
                digest: digest.clone(),
                from: from.to_string(),
                bytes_done: done.load(Ordering::Relaxed),
                bytes_total: total,
                bytes_reused: reused,
                done: false,
                verified: false,
            });
        })
    };
    report("", true);
    let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL_FILES));
    // Files another pull here fetched while this one waited.
    let shared = Arc::new(AtomicU64::new(0));
    // Sources that could not be reached: not asked again for other files.
    let dead: Arc<std::sync::Mutex<std::collections::HashSet<usize>>> = Default::default();
    let mut tasks = Vec::new();
    for (sha, size) in missing {
        let sem = sem.clone();
        let sources = sources.clone();
        let store = store.clone();
        let done = done.clone();
        let report = report.clone();
        let shared = shared.clone();
        let dead = dead.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await;
            // One download of a file at a time on this machine: pulls of
            // checkpoints that share it (a base disk) wait for the first
            // and then find it here.
            let lock = blob_lock(&store, &sha);
            let _one = lock.lock().await;
            if store.has_blob(&sha, size) {
                shared.fetch_add(size, Ordering::Relaxed);
                return Ok::<_, Error>((sha, String::new()));
            }
            let partial = store.partial_path(&sha)?;
            let mut errors = Vec::new();
            for (si, src) in sources.iter().enumerate() {
                let label = src.label();
                if dead.lock().unwrap().contains(&si) {
                    errors.push(format!("{label}: unreachable"));
                    continue;
                }
                // Bytes a failed attempt counted are taken back: the next
                // source resumes from what is on disk.
                let before = tokio::fs::metadata(&partial).await.map(|m| m.len()).unwrap_or(0);
                let counted = AtomicU64::new(0);
                done.fetch_add(before, Ordering::Relaxed);
                let on = |n: u64| {
                    counted.fetch_add(n, Ordering::Relaxed);
                    done.fetch_add(n, Ordering::Relaxed);
                    report(&label, false);
                };
                let got = src.fetch(&sha, size, &partial, &on).await;
                let r = match got {
                    Ok(()) => store.finalize_partial(&sha, size).await,
                    Err(e) => Err(e),
                };
                match r {
                    Ok(()) => return Ok::<_, Error>((sha, label)),
                    Err(e) => {
                        // Nothing came from it at all: it is down or not
                        // reachable from here.
                        if matches!(e, Error::Unavailable(_)) && counted.load(Ordering::Relaxed) == 0 {
                            dead.lock().unwrap().insert(si);
                        }
                        done.fetch_sub(before + counted.load(Ordering::Relaxed), Ordering::Relaxed);
                        tracing::info!(sha, source = %label, "snapshot file not copied from this source: {e}");
                        errors.push(format!("{label}: {e}"));
                    }
                }
            }
            Err(Error::Unavailable(format!(
                "no source gave file {sha}: {}",
                if errors.is_empty() {
                    "there are no sources".to_string()
                } else {
                    errors.join("; ")
                }
            )))
        }));
    }
    let mut out = PullReport::default();
    let mut first_err = None;
    for t in tasks {
        match t.await {
            // Another pull here fetched it meanwhile.
            Ok(Ok((sha, from))) if from.is_empty() => {
                let _ = sha;
            }
            Ok(Ok((sha, from))) => {
                out.from.insert(sha, from);
            }
            Ok(Err(e)) => {
                first_err.get_or_insert(e);
            }
            Err(e) => {
                first_err.get_or_insert(Error::Unavailable(e.to_string()));
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    out.bytes_done = done.load(Ordering::Relaxed);
    out.bytes_reused = reused + shared.load(Ordering::Relaxed);
    if let Some(p) = &progress {
        let mut from: Vec<&str> = out.from.values().map(String::as_str).collect();
        from.sort();
        from.dedup();
        p(Progress {
            name: m.name.clone(),
            digest,
            from: from.join(","),
            bytes_done: out.bytes_done,
            bytes_total: total - shared.load(Ordering::Relaxed),
            bytes_reused: out.bytes_reused,
            done: true,
            verified: true,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileSpec, Manifest};
    use std::path::PathBuf;

    async fn exported(dir: &Path) -> (Store, Manifest, String, Vec<PathBuf>) {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mut paths = vec![];
        // 3 MiB and 1 MiB of distinct bytes, plus a duplicate of the first.
        for (i, n) in [(0u8, 3 << 20), (1, 1 << 20)] {
            let p = src.join(format!("file{i}"));
            let v: Vec<u8> = (0..n)
                .map(|j: usize| (j as u8).wrapping_mul(31).wrapping_add(i))
                .collect();
            std::fs::write(&p, v).unwrap();
            paths.push(p);
        }
        let dup = src.join("dup");
        std::fs::copy(&paths[0], &dup).unwrap();
        let store = Store::at(dir.join("a"));
        let files = vec![
            FileSpec {
                id: "f0".into(),
                path: paths[0].clone(),
                dir: None,
                qcow2_backing: None,
            },
            FileSpec {
                id: "f1".into(),
                path: paths[1].clone(),
                dir: None,
                qcow2_backing: None,
            },
            FileSpec {
                id: "f2".into(),
                path: dup,
                dir: None,
                qcow2_backing: None,
            },
        ];
        let (m, d) = store
            .export(
                "w0",
                "qemu",
                "abi",
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                files,
            )
            .await
            .unwrap();
        (store, m, d, paths)
    }

    async fn serve(dir: &Path, allow: crate::serve::AllowFn) -> String {
        let home = dir.to_path_buf();
        let app = crate::serve::router(Arc::new(move || Some(home.clone())), allow);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}{}", crate::HTTP_BASE)
    }

    fn allow_all() -> crate::serve::AllowFn {
        Arc::new(|_| Ok(()))
    }

    /// The export lives in `<home>/snapshots`: the server's home is the
    /// parent of the store's root.
    async fn setup() -> (tempfile::TempDir, Store, Manifest, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home-a");
        let (_, m, d, _) = exported(&dir.path().join("x")).await;
        // Export again into the served home (same files, same digest).
        let served = Store::of_home(&home);
        let src = dir.path().join("x/src");
        let (m2, d2) = served
            .export(
                "w0",
                "qemu",
                "abi",
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                vec![
                    FileSpec {
                        id: "f0".into(),
                        path: src.join("file0"),
                        dir: None,
                        qcow2_backing: None,
                    },
                    FileSpec {
                        id: "f1".into(),
                        path: src.join("file1"),
                        dir: None,
                        qcow2_backing: None,
                    },
                    FileSpec {
                        id: "f2".into(),
                        path: src.join("dup"),
                        dir: None,
                        qcow2_backing: None,
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!((m2.clone(), d2.clone()), (m, d));
        let url = serve(&home, allow_all()).await;
        let grant =
            crate::grant::issue(&served.grant_key().unwrap(), &d2, 600, crate::grant::now());
        (dir, served, m2, url, grant)
    }

    #[tokio::test]
    async fn pulls_verifies_and_dedupes() {
        let (dir, _served, m, url, grant) = setup().await;
        let b = Store::at(dir.path().join("b"));
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        let s2 = seen.clone();
        let src: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &grant, "mac-a").unwrap());
        let r = pull(
            &b,
            &m,
            vec![src.clone()],
            Some(Arc::new(move |p| s2.lock().unwrap().push(p))),
        )
        .await
        .unwrap();
        // The duplicate file is one blob: 4 MiB copied, not 7.
        assert_eq!(r.bytes_done, 4 << 20);
        assert_eq!(r.from.len(), 2);
        let last = seen.lock().unwrap().last().cloned().unwrap();
        assert!(last.done && last.verified);
        assert_eq!(last.bytes_total, 4 << 20);
        for f in &m.files {
            assert!(b.has_blob(&f.sha256, f.size));
        }
        // Again: nothing to copy.
        let r = pull(&b, &m, vec![src], None).await.unwrap();
        assert_eq!(r.bytes_done, 0);
        assert_eq!(r.bytes_reused, 4 << 20);
    }

    #[tokio::test]
    async fn a_partial_download_resumes_where_it_stopped() {
        let (dir, served, m, url, grant) = setup().await;
        let b = Store::at(dir.path().join("b"));
        let big = m.files.iter().find(|f| f.size == 3 << 20).unwrap();
        // 1 MiB of it arrived before the connection dropped.
        let idx = served
            .export_index(&served.digest_of("w0").unwrap())
            .unwrap();
        let full = std::fs::read(&idx.files[&big.sha256].path).unwrap();
        std::fs::write(b.partial_path(&big.sha256).unwrap(), &full[..1 << 20]).unwrap();
        let src: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &grant, "mac-a").unwrap());
        let r = pull(&b, &m, vec![src], None).await.unwrap();
        // 2 MiB of the big file plus the 1 MiB file came over the wire; the
        // resumed MiB counts as done too.
        assert_eq!(r.bytes_done, 4 << 20);
        assert!(b.has_blob(&big.sha256, big.size));
        assert_eq!(std::fs::read(b.blob_path(&big.sha256)).unwrap(), full);
    }

    #[tokio::test]
    async fn bad_grants_and_peers_are_refused() {
        let (dir, served, m, url, _grant) = setup().await;
        let b = Store::at(dir.path().join("b"));
        // A grant signed by another key.
        let forged = crate::grant::issue(&[9u8; 32], &m.digest(), 600, crate::grant::now());
        let src: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &forged, "mac-a").unwrap());
        let e = pull(&b, &m, vec![src], None).await.unwrap_err();
        assert!(e.to_string().contains("not valid on this host"), "{e}");
        // A grant for another manifest.
        let other = crate::grant::issue(
            &served.grant_key().unwrap(),
            &"0".repeat(64),
            600,
            crate::grant::now(),
        );
        let src: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &other, "mac-a").unwrap());
        assert!(pull(&b, &m, vec![src], None).await.is_err());
        // The peer rule.
        let home = served.root().parent().unwrap().to_path_buf();
        let deny = serve(&home, Arc::new(|_| Err("not from the LAN".into()))).await;
        let g = crate::grant::issue(
            &served.grant_key().unwrap(),
            &m.digest(),
            600,
            crate::grant::now(),
        );
        let src: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&deny, &g, "mac-a").unwrap());
        let e = pull(&b, &m, vec![src], None).await.unwrap_err();
        assert!(e.to_string().contains("not from the LAN"), "{e}");
        assert!(m.files.iter().all(|f| !b.has_blob(&f.sha256, f.size)));
    }

    /// Pulls of two checkpoints that share a file, at once: it is fetched
    /// once, both complete (they used to race on its partial file).
    #[tokio::test]
    async fn concurrent_pulls_share_a_file_safely() {
        let (dir, served, m, url, grant) = setup().await;
        // A second manifest with one of the same files plus its own.
        let src = dir.path().join("x/src");
        std::fs::write(src.join("own"), vec![7u8; 1 << 20]).unwrap();
        let (m2, d2) = served
            .export(
                "w1",
                "qemu",
                "abi",
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                vec![
                    FileSpec {
                        id: "f0".into(),
                        path: src.join("file0"),
                        dir: None,
                        qcow2_backing: None,
                    },
                    FileSpec {
                        id: "f1".into(),
                        path: src.join("own"),
                        dir: None,
                        qcow2_backing: None,
                    },
                ],
            )
            .await
            .unwrap();
        let g2 = crate::grant::issue(&served.grant_key().unwrap(), &d2, 600, crate::grant::now());
        let b = Store::at(dir.path().join("b"));
        let s1: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &grant, "mac-a").unwrap());
        let s2: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &g2, "mac-a").unwrap());
        let (r1, r2) = tokio::join!(pull(&b, &m, vec![s1], None), pull(&b, &m2, vec![s2], None));
        let (r1, r2) = (r1.unwrap(), r2.unwrap());
        // The shared 3 MiB file came over once.
        assert_eq!(r1.from.len() + r2.from.len(), 3, "{r1:?} {r2:?}");
        for f in m.files.iter().chain(&m2.files) {
            assert!(b.has_blob(&f.sha256, f.size));
        }
    }

    /// A source that serves wrong bytes: the file is discarded and taken
    /// from the next source.
    struct Liar;
    #[async_trait::async_trait]
    impl BlobSource for Liar {
        fn label(&self) -> String {
            "liar".into()
        }
        async fn fetch(
            &self,
            _: &str,
            size: u64,
            partial: &Path,
            on: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<()> {
            std::fs::write(partial, vec![0u8; size as usize])?;
            on(size);
            Ok(())
        }
    }

    /// A source that is down.
    struct Down;
    #[async_trait::async_trait]
    impl BlobSource for Down {
        fn label(&self) -> String {
            "down".into()
        }
        async fn fetch(
            &self,
            _: &str,
            _: u64,
            _: &Path,
            _: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<()> {
            Err(Error::Unavailable("connection refused".into()))
        }
    }

    #[tokio::test]
    async fn the_next_source_is_asked_after_a_bad_or_dead_one() {
        let (dir, _served, m, url, grant) = setup().await;
        let b = Store::at(dir.path().join("b"));
        let good: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &grant, "mac-a").unwrap());
        let r = pull(&b, &m, vec![Arc::new(Down), Arc::new(Liar), good], None)
            .await
            .unwrap();
        assert!(r.from.values().all(|f| f == "mac-a"), "{r:?}");
        assert_eq!(r.bytes_done, 4 << 20, "failed attempts are not counted");
        // No source works: the error names each.
        let c = Store::at(dir.path().join("c"));
        let e = pull(&c, &m, vec![Arc::new(Down), Arc::new(Liar)], None)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("down: ") && e.contains("liar:"), "{e}");
        // Nothing was half-kept.
        assert!(m.files.iter().all(|f| !c.has_blob(&f.sha256, f.size)));
    }

    /// A source that is down after a delay, counting the calls it gets.
    struct SlowDown(AtomicU64);
    #[async_trait::async_trait]
    impl BlobSource for SlowDown {
        fn label(&self) -> String {
            "slow-down".into()
        }
        async fn fetch(
            &self,
            _: &str,
            _: u64,
            _: &Path,
            _: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(100)).await;
            Err(Error::Unavailable("timed out".into()))
        }
    }

    /// A host that cannot be reached is not asked again for the other files
    /// (each would wait for its connect timeout): only the files already
    /// asking it when it failed did.
    #[tokio::test]
    async fn an_unreachable_source_is_not_asked_again() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mut files = vec![];
        for i in 0..(PARALLEL_FILES * 3) {
            let p = src.join(format!("f{i}"));
            std::fs::write(&p, format!("file {i}")).unwrap();
            files.push(FileSpec {
                id: format!("f{i}"),
                path: p,
                dir: None,
                qcow2_backing: None,
            });
        }
        let a = Store::at(dir.path().join("a"));
        let (m, _) = a
            .export(
                "w0",
                "qemu",
                "abi",
                serde_json::json!({}),
                serde_json::json!({}),
                vec![],
                files,
            )
            .await
            .unwrap();
        let down = Arc::new(SlowDown(AtomicU64::new(0)));
        let b = Store::at(dir.path().join("b"));
        assert!(pull(&b, &m, vec![down.clone()], None).await.is_err());
        let calls = down.0.load(Ordering::SeqCst);
        assert!(calls as usize <= PARALLEL_FILES, "{calls} calls");
    }

    /// A file that changed on the source after the export is refused there
    /// (409), not served as the old content.
    #[tokio::test]
    async fn a_changed_source_file_is_not_served() {
        let (dir, served, m, url, grant) = setup().await;
        let idx = served.export_index(&m.digest()).unwrap();
        let (_, e) = idx.files.iter().next().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut bytes = std::fs::read(&e.path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&e.path, bytes).unwrap();
        let b = Store::at(dir.path().join("b"));
        let src: Arc<dyn BlobSource> = Arc::new(HttpSource::new(&url, &grant, "mac-a").unwrap());
        let err = pull(&b, &m, vec![src], None).await.unwrap_err().to_string();
        assert!(
            err.contains("changed since the snapshot was exported"),
            "{err}"
        );
    }
}
