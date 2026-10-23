//! share-a-link (Rust). Scenario definition: ../python/test_share.py; the
//! assertions here are the same.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use cua_sdk_e2e::cua::ShareAccess;
use cua_sdk_e2e::*;

/// A tiny app on a loopback port: answers `app says <path>`.
fn app() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = format!("app says {path}");
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    port
}

#[tokio::test]
async fn files_and_ports_become_links_a_daemon_owns() {
    e2e(
        "share-a-link",
        "hermetic",
        "files and ports become links a daemon owns",
        Duration::from_secs(120),
        None,
        || async {
            let port = app();
            let dir = tmp_dir("share");
            let report = dir.join("report.html");
            std::fs::write(&report, "<h1>report</h1>")?;
            let d = ShareDaemon::start()?;
            let c = d.client();

            let f = c.share(report.display().to_string(), None, None, None).await?;
            assert_eq!(f.access, ShareAccess::Private);
            assert_eq!(f.kind, "file");
            assert!(f.url.starts_with(&format!("{}/m/{}/share/", d.relay_url, SHARE_MACHINE)), "{}", f.url);
            assert_eq!(d.get(&f.url, Some("owner"))?, (200, "<h1>report</h1>".to_string()));
            assert_eq!(d.get(&f.url, None)?.0, 401);
            assert_eq!(d.get(&f.url, Some("other"))?.0, 403);
            let check = c.check_share(f.id.clone(), None).await?;
            assert!(check.ok && check.status == 200 && check.via == "link", "{check:?}");

            let p = c
                .share(format!("localhost:{port}"), None, None, Some("dash".into()))
                .await?;
            assert_eq!(
                d.get(&format!("{}x?y=1", p.url), Some("owner"))?,
                (200, "app says /x?y=1".to_string())
            );
            assert_eq!(c.shares().await?.len(), 2);
            assert_eq!(c.stop_share("dash".into()).await?, vec![p.id.clone()]);
            assert_eq!(d.get(&p.url, Some("owner"))?.0, 404);

            let public = c
                .share(dir.display().to_string(), Some(ShareAccess::Public), Some(60), None)
                .await?;
            assert!(public.url.contains("/share/pub/") && public.expires_at_unix.is_some());
            let (code, body) = d.get(&public.url, None)?;
            assert!(code == 200 && body.contains("report.html"), "{body}");

            assert_eq!(d.client().stop_all_shares().await?.len(), 2);
            assert_eq!(d.get(&public.url, None)?.0, 404);
            assert!(c.shares().await?.is_empty());

            for bad in [
                c.share(dir.join("missing").display().to_string(), None, None, None).await,
                c.share("0".into(), None, None, None).await,
                c.share(report.display().to_string(), Some(ShareAccess::Public), Some(5), None).await,
            ] {
                assert!(matches!(bad, Err(CuaError::InvalidArgument(_))), "{bad:?}");
            }
            assert!(matches!(c.stop_share("share-nope".into()).await, Err(CuaError::NotFound(_))));

            let dead = c.share("1".into(), None, None, None).await?;
            let bad = c.check_share(dead.id, None).await?;
            assert!(!bad.ok && bad.status == 502, "{bad:?}");
            c.stop_all_shares().await?;
            Ok(())
        },
    )
    .await;
}
