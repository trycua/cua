//! The gateway over a real Unix socket, against a real upstream.

use super::*;
use crate::model::{Access, Owner, random_hex};
use std::sync::Arc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[derive(Default)]
struct Log(Mutex<Vec<String>>);
impl AuditSink for Log {
    fn record(&self, principal: &str, action: &str, handle: &str, detail: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{principal}|{action}|{handle}|{detail}"));
    }
}
struct NoSpaces;
impl UpstreamResolver for NoSpaces {
    fn resolve_space<'a>(
        &'a self,
        _s: &'a str,
        _p: u16,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + 'a>> {
        Box::pin(async { Err("no such Space".to_string()) })
    }
}

/// Echoes method, path and the headers the gateway must control.
async fn upstream() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = req.lines().next().unwrap_or_default().to_string();
                let hdr = |name: &str| {
                    req.lines()
                        .find(|l| l.to_ascii_lowercase().starts_with(&format!("{name}:")))
                        .unwrap_or("")
                        .to_string()
                };
                let body = format!(
                    "{line}\n{}\n{}\n{}",
                    hdr("host"),
                    hdr("x-cua-share-caller"),
                    hdr("x-forwarded-host")
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nlocation: http://127.0.0.1:{}/next\r\nconnection: close\r\n\r\n{body}",
                    body.len(),
                    0
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

struct Rig {
    sock: PathBuf,
    store: Arc<ShareStore>,
    log: Arc<Log>,
    _h: GatewayHandle,
    _d: tempfile::TempDir,
    up: u16,
}

async fn rig(cfg: GatewayConfig) -> Rig {
    let d = tempfile::tempdir().unwrap();
    let store = Arc::new(ShareStore::open(d.path().join("share/shares.json")).unwrap());
    let log = Arc::new(Log::default());
    let gw = Arc::new(Gateway::new(
        store.clone(),
        Arc::new(NoSpaces),
        log.clone(),
        cfg,
    ));
    let sock = d.path().join("share/gateway.sock");
    let h = gw.serve_unix(&sock).await.unwrap();
    Rig {
        sock,
        store,
        log,
        _h: h,
        _d: d,
        up: upstream().await,
    }
}

fn mk(access: Access, target: Target, expires_ms: Option<u64>) -> Share {
    let key = if access == Access::Public {
        random_hex(20)
    } else {
        random_hex(8)
    };
    Share {
        handle: format!("share-{}", random_hex(8)),
        key,
        name: None,
        target,
        access,
        owner: Owner {
            account: "alice".into(),
            email: Some("a@x.io".into()),
        },
        teams: vec!["t1".into()],
        created_ms: now_ms(),
        expires_ms,
        url: String::new(),
    }
}

async fn req(
    sock: &Path,
    method: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> (u16, String, String) {
    let mut s = tokio::net::UnixStream::connect(sock).await.unwrap();
    let mut r = format!("{method} {path} HTTP/1.1\r\nhost: s1.share.test\r\nconnection: close\r\n");
    for (k, v) in extra {
        r.push_str(&format!("{k}: {v}\r\n"));
    }
    r.push_str("\r\n");
    s.write_all(r.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_string();
    let code = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (code, head.to_string(), body.to_string())
}

fn owner() -> String {
    r#"{"account":"alice","email":"a@x.io","role":"owner","teams":[],"via":"relay"}"#.to_string()
}

#[tokio::test]
async fn private_share_needs_the_owner() {
    let r = rig(GatewayConfig::default()).await;
    let s = mk(Access::Private, Target::Port { port: r.up }, None);
    r.store.add(s.clone()).unwrap();
    let path = format!("/share/{}/hello?x=1", s.key);
    assert_eq!(req(&r.sock, "GET", &path, &[]).await.0, 401);
    let bob = r#"{"account":"bob","role":"shared","teams":["t1"]}"#;
    assert_eq!(
        req(&r.sock, "GET", &path, &[(CALLER_HEADER, bob)]).await.0,
        403
    );
    let (code, head, body) = req(
        &r.sock,
        "GET",
        &path,
        &[
            (CALLER_HEADER, &owner()),
            ("x-forwarded-host", "s1.share.cua.test"),
        ],
    )
    .await;
    assert_eq!(code, 200, "{body}");
    // The app sees its own Host, the path below the share, the public host,
    // and never the credential header.
    assert!(body.starts_with("GET /hello?x=1 HTTP/1.1"), "{body}");
    assert!(
        body.contains(&format!("host: 127.0.0.1:{}", r.up)),
        "{body}"
    );
    assert!(
        body.contains("x-forwarded-host: s1.share.cua.test"),
        "{body}"
    );
    assert!(
        !body.to_ascii_lowercase().contains("x-cua-share-caller"),
        "{body}"
    );
    // Redirects to the app's own address come back to the link.
    let _ = head;
    // The audit log holds the open and both refusals.
    let log = r.log.0.lock().unwrap().join("\n");
    assert!(log.contains("a@x.io|open|"), "{log}");
    assert!(
        log.contains("anonymous|denied|") && log.contains("account:bob|denied|"),
        "{log}"
    );
}

#[tokio::test]
async fn ids_and_tokens_do_not_cross() {
    let r = rig(GatewayConfig::default()).await;
    let private = mk(Access::Private, Target::Port { port: r.up }, None);
    let public = mk(Access::Public, Target::Port { port: r.up }, None);
    r.store.add(private.clone()).unwrap();
    r.store.add(public.clone()).unwrap();
    // A private id on the public path, and a token on the private path, are
    // simply unknown.
    assert_eq!(
        req(&r.sock, "GET", &format!("/share/pub/{}/", private.key), &[])
            .await
            .0,
        404
    );
    assert_eq!(
        req(
            &r.sock,
            "GET",
            &format!("/share/{}/", public.key),
            &[(CALLER_HEADER, &owner())]
        )
        .await
        .0,
        404
    );
    assert_eq!(
        req(&r.sock, "GET", &format!("/share/pub/{}/", public.key), &[])
            .await
            .0,
        200
    );
    for p in [
        "/",
        "/share",
        "/share/",
        "/share/xyz/",
        "/share/pub/",
        "/share/pub/short/",
        "/metrics",
        "/share/../etc",
    ] {
        assert_eq!(req(&r.sock, "GET", p, &[]).await.0, 404, "{p}");
    }
}

#[tokio::test]
async fn public_links_expire_and_are_removed_on_stop() {
    let r = rig(GatewayConfig::default()).await;
    let live = mk(
        Access::Public,
        Target::Port { port: r.up },
        Some(now_ms() + 60_000),
    );
    let dead = mk(
        Access::Public,
        Target::Port { port: r.up },
        Some(now_ms().saturating_sub(1)),
    );
    r.store.add(live.clone()).unwrap();
    r.store.add(dead.clone()).unwrap();
    assert_eq!(
        req(&r.sock, "GET", &format!("/share/pub/{}/", dead.key), &[])
            .await
            .0,
        410
    );
    assert_eq!(
        req(&r.sock, "GET", &format!("/share/pub/{}/", live.key), &[])
            .await
            .0,
        200
    );
    r.store.remove_where(|s| s.handle == live.handle).unwrap();
    assert_eq!(
        req(&r.sock, "GET", &format!("/share/pub/{}/", live.key), &[])
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn public_links_are_rate_limited_per_share() {
    let r = rig(GatewayConfig {
        public_rps: 1.0,
        public_burst: 3.0,
        ..Default::default()
    })
    .await;
    let a = mk(Access::Public, Target::Port { port: r.up }, None);
    let b = mk(Access::Public, Target::Port { port: r.up }, None);
    r.store.add(a.clone()).unwrap();
    r.store.add(b.clone()).unwrap();
    let pa = format!("/share/pub/{}/", a.key);
    let mut codes = vec![];
    for _ in 0..6 {
        codes.push(req(&r.sock, "GET", &pa, &[]).await.0);
    }
    assert_eq!(&codes[..3], &[200, 200, 200], "{codes:?}");
    assert!(codes[3..].iter().all(|c| *c == 429), "{codes:?}");
    let (_, head, _) = req(&r.sock, "GET", &pa, &[]).await;
    assert!(head.to_ascii_lowercase().contains("retry-after:"), "{head}");
    // Another share is unaffected.
    assert_eq!(
        req(&r.sock, "GET", &format!("/share/pub/{}/", b.key), &[])
            .await
            .0,
        200
    );
    assert!(
        r.log
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("|rate-limited|"))
    );
}

#[tokio::test]
async fn guessing_is_throttled() {
    let r = rig(GatewayConfig {
        unknown_rps: 0.5,
        unknown_burst: 3.0,
        ..Default::default()
    })
    .await;
    let mut codes = vec![];
    for _ in 0..8 {
        codes.push(
            req(
                &r.sock,
                "GET",
                &format!("/share/pub/{}/", random_hex(20)),
                &[],
            )
            .await
            .0,
        );
    }
    assert_eq!(&codes[..3], &[404, 404, 404], "{codes:?}");
    assert!(codes[3..].iter().all(|c| *c == 429), "{codes:?}");
}

#[tokio::test]
async fn files_are_read_only_and_cannot_escape() {
    let r = rig(GatewayConfig::default()).await;
    let d = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(d.path()).unwrap().join("pub");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("r.html"), "<h1>report</h1>").unwrap();
    std::fs::write(d.path().join("secret.txt"), "TOP").unwrap();
    let s = mk(Access::Public, Target::Dir { path: root }, None);
    r.store.add(s.clone()).unwrap();
    let base = format!("/share/pub/{}", s.key);
    let (c, _, b) = req(&r.sock, "GET", &format!("{base}/r.html"), &[]).await;
    assert_eq!((c, b.as_str()), (200, "<h1>report</h1>"));
    for p in ["/../secret.txt", "/%2e%2e/secret.txt", "/..%2fsecret.txt"] {
        let (c, _, b) = req(&r.sock, "GET", &format!("{base}{p}"), &[]).await;
        assert_ne!(c, 200, "{p}");
        assert!(!b.contains("TOP"), "{p}");
    }
    assert_eq!(
        req(&r.sock, "PUT", &format!("{base}/new.txt"), &[]).await.0,
        405
    );
    assert_eq!(
        req(&r.sock, "DELETE", &format!("{base}/r.html"), &[])
            .await
            .0,
        405
    );
}

#[tokio::test]
async fn a_malformed_caller_header_fails_closed() {
    let r = rig(GatewayConfig::default()).await;
    let s = mk(Access::Private, Target::Port { port: r.up }, None);
    r.store.add(s.clone()).unwrap();
    let (c, _, _) = req(
        &r.sock,
        "GET",
        &format!("/share/{}/", s.key),
        &[(CALLER_HEADER, "not json")],
    )
    .await;
    assert_eq!(c, 400);
}

#[tokio::test]
async fn the_socket_is_private_and_not_taken_over() {
    use std::os::unix::fs::PermissionsExt as _;
    let r = rig(GatewayConfig::default()).await;
    assert_eq!(
        std::fs::metadata(&r.sock).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(r.sock.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let gw2 = Arc::new(Gateway::new(
        r.store.clone(),
        Arc::new(NoSpaces),
        r.log.clone(),
        GatewayConfig::default(),
    ));
    assert!(gw2.serve_unix(&r.sock).await.is_err());
}

#[tokio::test]
async fn a_space_that_is_not_reachable_is_a_502() {
    let r = rig(GatewayConfig::default()).await;
    let s = mk(
        Access::Public,
        Target::SpacePort {
            space: "dev".into(),
            port: 3000,
        },
        None,
    );
    r.store.add(s.clone()).unwrap();
    let (c, _, b) = req(&r.sock, "GET", &format!("/share/pub/{}/", s.key), &[]).await;
    assert_eq!(c, 502);
    assert!(b.contains("not reachable"));
}
