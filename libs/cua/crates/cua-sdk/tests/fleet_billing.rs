//! The account's billing through the exported SDK (what the Spaces apps'
//! Settings show), against a loopback account API; Cua Cloud calls and
//! cloud creates say it has closed.

use cua_daemon::{Runtime, RuntimeConfig};
use cua_sdk::{Cua, CuaError, SandboxCreateOptions};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A loopback account API answering every request with `status` and
/// `body`; returns its base URL and the request heads it saw.
async fn account_api(status: u16, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = s.read(&mut buf).await.unwrap_or(0);
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).to_string());
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = s.write_all(resp.as_bytes()).await;
        }
    });
    (url, seen)
}

fn cua(base_url: &str, dirs: &tempfile::TempDir) -> Arc<Cua> {
    let account = cua_auth::account::AccountApi::from_lookup(&|k| match k {
        "CUA_FLEET_BASE_URL" => Some(base_url.to_string()),
        "FLEETS_TOKEN" => Some("account-token".into()),
        _ => None,
    });
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        account,
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    Cua::from_runtime(runtime)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_carries_the_credit_and_the_billing_page() {
    let (url, seen) = account_api(
        200,
        r#"{"billing_enabled": true, "payment_method_present": false, "card": null,
            "plan": "none", "payg_available": true,
            "credit": {"balance_usd_cents": 742, "signup_grant_usd_cents": 100, "signup_grant_unused": false},
            "billing_url": "https://run.cua.ai/billing"}"#,
    )
    .await;
    let dirs = tempfile::tempdir().unwrap();
    let fleet = cua(&url, &dirs).fleet().unwrap();
    assert_eq!(fleet.base_url(), url);
    let s = fleet.billing_status().await.unwrap();
    assert_eq!(s.credit.as_ref().map(|c| c.balance_usd_cents), Some(742));
    assert_eq!(s.billing_url.as_deref(), Some("https://run.cua.ai/billing"));
    let head = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(head.starts_with("get /api/billing/status "), "{head}");
    assert!(
        head.contains("authorization: bearer account-token"),
        "{head}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_api_without_billing_means_billing_off() {
    let (url, _) = account_api(404, "{}").await;
    let dirs = tempfile::tempdir().unwrap();
    let s = cua(&url, &dirs)
        .fleet()
        .unwrap()
        .billing_status()
        .await
        .unwrap();
    assert!(!s.billing_enabled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cloud_calls_say_cua_cloud_has_closed() {
    let (url, seen) = account_api(200, "{}").await;
    let dirs = tempfile::tempdir().unwrap();
    let cua = cua(&url, &dirs);
    match cua.fleet().unwrap().get_pool("p".into()).await {
        Err(CuaError::Fleet(m)) => assert!(m.contains("Cua Cloud has closed"), "{m}"),
        other => panic!("expected the closure, got {other:?}"),
    }
    let mut o = SandboxCreateOptions::new("cloud", "ghcr.io/trycua/linux:24.04");
    o.kind = Some("vm".into());
    match cua.sandboxes().create(o).await {
        Err(CuaError::Fleet(m)) => assert!(m.contains("Cua Cloud has closed"), "{m}"),
        Err(e) => panic!("expected the closure, got {e:?}"),
        Ok(_) => panic!("expected the closure, got a sandbox"),
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing reached the account API"
    );
}
