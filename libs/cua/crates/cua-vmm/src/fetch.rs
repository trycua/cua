//! Resumable, retrying downloads of the pinned files cua fetches itself (the
//! built-in Linux runtime's tools and disk, Apple's `container` package).
//!
//! These are hundreds of megabytes fetched on a first run, often over a
//! network that drops. A download therefore:
//!
//! * writes to `<file>.partial` and **resumes** from it with an HTTP `Range`
//!   request, in the same run after a lost connection and in the next one
//!   after a quit (the bytes already there are hashed again, so the file is
//!   still checked end to end against its SHA-256);
//! * **retries** a lost connection, a timeout and a 5xx or 429 answer with a
//!   growing wait, instead of failing the create;
//! * starts over, once, when the server ignores `Range` or the finished file
//!   does not match its checksum (a bad resume must never be a bad install);
//! * tells the caller about a 401 or 403 ([`FetchError::Unauthorized`]), so a
//!   registry token that ran out can be asked for again and the download
//!   carries on from the same place.
//!
//! `on_bytes` hears every byte once: those already in the partial file when
//! the download starts, then each new chunk. A retry does not count a byte
//! twice.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::error::VmmError;

/// What to fetch.
pub struct Fetch<'a> {
    pub url: &'a str,
    /// A bearer token (a registry blob), when it needs one.
    pub bearer: Option<&'a str>,
    /// The size it must have, when known.
    pub size: Option<u64>,
    /// The SHA-256 (lowercase hex) it must have.
    pub sha256: &'a str,
}

/// How hard to try.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Tries in all (the first included).
    pub attempts: u32,
    /// The wait before the second try; it doubles up to `max_wait`.
    pub first_wait: Duration,
    pub max_wait: Duration,
    pub connect_timeout: Duration,
    /// Longest silence on an open connection before it counts as lost.
    pub read_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            attempts: 8,
            first_wait: Duration::from_secs(2),
            max_wait: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(20),
            read_timeout: Duration::from_secs(30),
        }
    }
}

/// Why a fetch ended.
#[derive(Debug)]
pub enum FetchError {
    /// The server refused the token (401 or 403): ask for a new one and call again.
    Unauthorized,
    /// Anything else, in words.
    Failed(VmmError),
}

impl From<FetchError> for VmmError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::Unauthorized => VmmError::other("the registry refused the download token"),
            FetchError::Failed(e) => e,
        }
    }
}

impl From<std::io::Error> for FetchError {
    fn from(e: std::io::Error) -> Self {
        FetchError::Failed(e.into())
    }
}

/// Where the partial bytes of `to` are kept.
pub fn partial_path(to: &Path) -> PathBuf {
    let mut p = to.as_os_str().to_owned();
    p.push(".partial");
    PathBuf::from(p)
}

enum Once {
    Done,
    /// Lost connection, timeout, 5xx or 429: try again from the partial file.
    Retry(String),
    /// The partial file is not usable: delete it and start over.
    Restart(String),
    Unauthorized,
    Fatal(VmmError),
}

fn sha256_of_prefix(path: &Path) -> std::io::Result<Sha256> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(h);
        }
        h.update(&buf[..n]);
    }
}

fn client(p: &Policy) -> Result<reqwest::Client, FetchError> {
    reqwest::Client::builder()
        .connect_timeout(p.connect_timeout)
        .read_timeout(p.read_timeout)
        .build()
        .map_err(|e| FetchError::Failed(VmmError::other(format!("download: {e}"))))
}

async fn once(
    f: &Fetch<'_>,
    client: &reqwest::Client,
    partial: &Path,
    to: &Path,
    credited: &mut u64,
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Once {
    let have = std::fs::metadata(partial).map(|m| m.len()).unwrap_or(0);
    if f.size.is_some_and(|s| have > s) {
        return Once::Restart("the partial file is larger than the file".into());
    }
    let mut req = client.get(f.url);
    if let Some(t) = f.bearer {
        req = req.bearer_auth(t);
    }
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let res = match req.send().await {
        Ok(r) => r,
        Err(e) => return Once::Retry(e.to_string()),
    };
    let status = res.status();
    let resumed = match status.as_u16() {
        206 => true,
        200 => false,
        401 | 403 => return Once::Unauthorized,
        // The partial file is complete or past the end: look at it afresh.
        416 => return Once::Restart("the server could not resume the download".into()),
        408 | 429 | 500..=599 => return Once::Retry(format!("the server answered {status}")),
        _ => return Once::Fatal(VmmError::other(format!("download {}: {status}", f.url))),
    };
    if resumed && have > 0 {
        // Content-Range must start where the partial file ends.
        let ok = res
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with(&format!("bytes {have}-")));
        if !ok {
            return Once::Restart("the server resumed from somewhere else".into());
        }
    }
    let (mut file, mut hash, mut got) = if resumed && have > 0 {
        let hash = match sha256_of_prefix(partial) {
            Ok(h) => h,
            Err(e) => return Once::Fatal(e.into()),
        };
        let file = match tokio::fs::OpenOptions::new()
            .append(true)
            .open(partial)
            .await
        {
            Ok(f) => f,
            Err(e) => return Once::Fatal(e.into()),
        };
        (file, hash, have)
    } else {
        // A server that ignores Range sends the whole file again.
        let file = match tokio::fs::File::create(partial).await {
            Ok(f) => f,
            Err(e) => return Once::Fatal(e.into()),
        };
        (file, Sha256::new(), 0)
    };
    // Bytes already in the partial file count once, on the first look at them.
    if got > *credited {
        on_bytes(got - *credited);
        *credited = got;
    }
    let mut res = res;
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                got += chunk.len() as u64;
                if f.size.is_some_and(|s| got > s) {
                    return Once::Restart("the download is larger than the file".into());
                }
                hash.update(&chunk);
                if let Err(e) = file.write_all(&chunk).await {
                    return Once::Fatal(e.into());
                }
                if got > *credited {
                    on_bytes(got - *credited);
                    *credited = got;
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = file.flush().await;
                return Once::Retry(e.to_string());
            }
        }
    }
    if let Err(e) = file.flush().await {
        return Once::Fatal(e.into());
    }
    drop(file);
    if f.size.is_some_and(|s| got < s) {
        // The connection ended early without an error: keep what came, ask for the rest.
        return Once::Retry("the connection closed before the end of the file".into());
    }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != f.sha256 {
        return Once::Restart(format!(
            "the file did not match its checksum (got {digest})"
        ));
    }
    match tokio::fs::rename(partial, to).await {
        Ok(()) => Once::Done,
        Err(e) => Once::Fatal(e.into()),
    }
}

/// Downloads `f` to `to` (through `to` + `.partial`), resuming and retrying
/// as the module says. `on_bytes` hears each byte once.
pub async fn fetch(
    f: &Fetch<'_>,
    to: &Path,
    policy: &Policy,
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Result<(), FetchError> {
    let partial = partial_path(to);
    let client = client(policy)?;
    let mut credited = 0u64;
    let mut wait = policy.first_wait;
    let mut restarts = 0u32;
    let mut last = String::new();
    for attempt in 1..=policy.attempts.max(1) {
        match once(f, &client, &partial, to, &mut credited, on_bytes).await {
            Once::Done => return Ok(()),
            Once::Unauthorized => return Err(FetchError::Unauthorized),
            Once::Fatal(e) => return Err(FetchError::Failed(e)),
            Once::Restart(why) => {
                // Once: a second bad resume is a real failure, not something to loop on.
                restarts += 1;
                last = why;
                let _ = tokio::fs::remove_file(&partial).await;
                credited = 0;
                if restarts > 1 {
                    break;
                }
            }
            Once::Retry(why) => {
                last = why;
                if attempt < policy.attempts {
                    tracing::warn!(url = f.url, attempt, reason = %last, "download interrupted, resuming");
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(policy.max_wait);
                }
            }
        }
    }
    let kept = if partial.exists() {
        " (what was downloaded is kept)"
    } else {
        ""
    };
    Err(FetchError::Failed(VmmError::other(format!(
        "download {}: {last}; check the connection and try again{kept}",
        f.url
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt as _;
    use tokio::net::TcpListener;

    /// A tiny HTTP server: serves `blob` with Range, and for each of its
    /// first requests drops the connection after sending `cut[i]` bytes of
    /// the body (usize::MAX: no cut). Records each request's Range header.
    struct Server {
        url: String,
        ranges: Arc<Mutex<Vec<Option<String>>>>,
    }

    async fn serve(blob: Vec<u8>, cut: Vec<usize>, honour_range: bool) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/blob", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges.clone();
        tokio::spawn(async move {
            let mut n = 0usize;
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 4096];
                let k = s.read(&mut buf).await.unwrap();
                let head = String::from_utf8_lossy(&buf[..k]).to_string();
                let range = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .map(|l| l.split_once(':').unwrap().1.trim().to_string());
                seen.lock().unwrap().push(range.clone());
                let start = match (&range, honour_range) {
                    (Some(r), true) => r
                        .trim_start_matches("bytes=")
                        .trim_end_matches('-')
                        .parse::<usize>()
                        .unwrap_or(0),
                    _ => 0,
                };
                let body = &blob[start..];
                let (status, extra) = if start > 0 {
                    (
                        "206 Partial Content",
                        format!(
                            "Content-Range: bytes {start}-{}/{}\r\n",
                            blob.len() - 1,
                            blob.len()
                        ),
                    )
                } else {
                    ("200 OK", String::new())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                    body.len()
                );
                let limit = cut.get(n).copied().unwrap_or(usize::MAX);
                n += 1;
                let _ = s.write_all(head.as_bytes()).await;
                let send = body.len().min(limit);
                let _ = s.write_all(&body[..send]).await;
                let _ = s.flush().await;
                // Dropping the socket mid-body is a lost connection.
                drop(s);
            }
        });
        Server { url, ranges }
    }

    fn blob(n: usize) -> (Vec<u8>, String) {
        let b: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
        let h = Sha256::digest(&b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect::<String>();
        (b, h)
    }

    fn fast() -> Policy {
        Policy {
            attempts: 6,
            first_wait: Duration::from_millis(5),
            max_wait: Duration::from_millis(20),
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn a_connection_dropped_twice_resumes_from_what_arrived() {
        let (b, sha) = blob(3_000_000);
        let srv = serve(b.clone(), vec![1_000_000, 900_000], true).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("f.bin");
        let mut counted = 0u64;
        fetch(
            &Fetch {
                url: &srv.url,
                bearer: None,
                size: Some(b.len() as u64),
                sha256: &sha,
            },
            &to,
            &fast(),
            &mut |n| counted += n,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b);
        assert!(!partial_path(&to).exists());
        assert_eq!(counted, b.len() as u64, "every byte is counted once");
        let r = srv.ranges.lock().unwrap().clone();
        assert_eq!(r.len(), 3, "two drops, one finish: {r:?}");
        assert_eq!(r[0], None);
        assert!(
            r[1].as_deref().unwrap().starts_with("bytes=") && r[1].as_deref() != Some("bytes=0-")
        );
        assert!(r[2].is_some(), "the third request resumed too");
    }

    #[tokio::test]
    async fn a_partial_file_from_an_earlier_run_is_resumed_and_counted() {
        let (b, sha) = blob(2_000_000);
        let srv = serve(b.clone(), vec![], true).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("f.bin");
        std::fs::write(partial_path(&to), &b[..700_000]).unwrap();
        let mut counted = 0u64;
        fetch(
            &Fetch {
                url: &srv.url,
                bearer: None,
                size: Some(b.len() as u64),
                sha256: &sha,
            },
            &to,
            &fast(),
            &mut |n| counted += n,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b);
        assert_eq!(counted, b.len() as u64);
        let r = srv.ranges.lock().unwrap().clone();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].as_deref(), Some("bytes=700000-"));
    }

    #[tokio::test]
    async fn a_server_that_ignores_range_restarts_cleanly() {
        let (b, sha) = blob(1_500_000);
        // First request is cut; the server never honours Range.
        let srv = serve(b.clone(), vec![500_000], false).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("f.bin");
        fetch(
            &Fetch {
                url: &srv.url,
                bearer: None,
                size: Some(b.len() as u64),
                sha256: &sha,
            },
            &to,
            &fast(),
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(&to).unwrap(),
            b,
            "the whole file, not the cut one glued on"
        );
    }

    #[tokio::test]
    async fn a_corrupt_partial_file_is_never_installed() {
        let (b, sha) = blob(1_000_000);
        let srv = serve(b.clone(), vec![], true).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("f.bin");
        // Garbage that is not a prefix of the file: the resume fails its checksum, starts over once.
        std::fs::write(partial_path(&to), vec![7u8; 300_000]).unwrap();
        fetch(
            &Fetch {
                url: &srv.url,
                bearer: None,
                size: Some(b.len() as u64),
                sha256: &sha,
            },
            &to,
            &fast(),
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b);
    }

    #[tokio::test]
    async fn the_wrong_checksum_is_an_error_not_an_install() {
        let (b, _sha) = blob(100_000);
        let srv = serve(b.clone(), vec![], true).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("f.bin");
        let e = fetch(
            &Fetch {
                url: &srv.url,
                bearer: None,
                size: Some(b.len() as u64),
                sha256: &"0".repeat(64),
            },
            &to,
            &fast(),
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(e, FetchError::Failed(_)));
        assert!(!to.exists());
        assert!(!partial_path(&to).exists());
    }

    #[tokio::test]
    async fn a_server_that_is_gone_fails_in_words_and_keeps_the_partial_file() {
        let (b, sha) = blob(400_000);
        // Every request is cut after 100 KB and the budget is two tries.
        let srv = serve(b.clone(), vec![100_000; 10], true).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("f.bin");
        let mut p = fast();
        p.attempts = 2;
        let e = fetch(
            &Fetch {
                url: &srv.url,
                bearer: None,
                size: Some(b.len() as u64),
                sha256: &sha,
            },
            &to,
            &p,
            &mut |_| {},
        )
        .await
        .unwrap_err();
        let FetchError::Failed(e) = e else { panic!() };
        assert!(e.to_string().contains("what was downloaded is kept"), "{e}");
        assert_eq!(std::fs::metadata(partial_path(&to)).unwrap().len(), 200_000);
        assert!(!to.exists());
    }
}
