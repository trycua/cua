// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Await-token-file mode: a non-loopback driver with no token serves only
//! GetCapabilities/Health until its token file holds a token; the file then
//! installs, rotates (revoking open sessions) and revokes (back to awaiting)
//! the token. Init never accepts a token over the network in this mode.
//!
//! Every wait below is bounded (see `eventually`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use cua_proto::env::v1::*;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::token_file::write_token_atomically;
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use tonic::Code;

const TOKEN_A: &str = "tokenA-0123456789abcdef0123456789";
const TOKEN_B: &str = "tokenB-0123456789abcdef0123456789";
const TRANSPORTS: [TransportPreference; 2] =
    [TransportPreference::Native, TransportPreference::GrpcWeb];

struct Driver {
    url: String,
    token_file: PathBuf,
    ctx: ServerContext,
    _dirs: Vec<tempfile::TempDir>,
}

/// A driver configured for a non-loopback bind in await-token-file mode
/// (served on loopback for the test). `initial` is written before start.
async fn start(initial: Option<&str>) -> Driver {
    let data = tempfile::tempdir().unwrap();
    let run = tempfile::tempdir().unwrap();
    let token_file = run.path().join("env-token");
    write_token_atomically(&token_file, initial, None).unwrap();
    let config = ServerConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        await_token_file: Some(token_file.clone()),
        token_poll_interval: Duration::from_millis(50),
        data_dir: data.path().to_path_buf(),
        shutdown_grace: Duration::from_secs(1),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, None);
    let server = ServerBuilder::new(ctx.clone()).build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    Driver {
        url: format!("http://{addr}"),
        token_file,
        ctx,
        _dirs: vec![data, run],
    }
}

async fn client(url: &str, token: Option<&str>, transport: TransportPreference) -> SpacesdClient {
    let mut o = ConnectOptions::parse(url)
        .unwrap()
        .transport(transport)
        .probe(false);
    if let Some(t) = token {
        o = o.token(t);
    }
    SpacesdClient::connect(o).await.unwrap()
}

fn reason(status: &tonic::Status) -> i32 {
    cua_spacesd_server::error::error_info(status)
        .map(|i| i.reason)
        .unwrap_or_default()
}

fn stat_root() -> StatRequest {
    StatRequest {
        path: "/".into(),
        ..Default::default()
    }
}

/// Polls `check` every 25 ms for at most 5 s.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn wait_token(d: &Driver, expected: Option<&str>) {
    let ctx = d.ctx.clone();
    let expected = expected.map(str::to_owned);
    eventually("the driver token to match the file", || {
        let ctx = ctx.clone();
        let expected = expected.clone();
        async move { ctx.auth().token().as_deref() == expected.as_deref() }
    })
    .await;
}

fn write(path: &Path, token: Option<&str>) {
    write_token_atomically(path, token, None).unwrap();
}

async fn assert_awaiting(url: &str, transport: TransportPreference) {
    let anon = client(url, None, transport).await;
    let caps = anon
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(!caps.initialized, "{transport:?}");
    let health = anon
        .system()
        .health(HealthRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(health.status, HealthStatus::Serving as i32);
    let auth = health.components.iter().find(|c| c.name == "auth").unwrap();
    assert!(auth.detail.starts_with("awaiting"), "{}", auth.detail);
    let refused = anon.filesystem().stat(stat_root()).await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition, "{transport:?}");
    assert_eq!(reason(&refused), ErrorReason::NotInitialized as i32);
    assert!(
        refused.message().contains("awaiting token"),
        "{}",
        refused.message()
    );
    // Init is not reachable, so a token can never arrive over the network.
    let init = anon
        .system()
        .init(InitRequest {
            token: "attacker-0123456789abcdef".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(init.code(), Code::FailedPrecondition, "{transport:?}");
    // A token-bearing caller is refused the same way (there is no token).
    let guess = client(url, Some(TOKEN_A), transport).await;
    let refused = guess.filesystem().stat(stat_root()).await.unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
}

#[tokio::test]
async fn awaiting_then_install_rotate_revoke() {
    for transport in TRANSPORTS {
        let d = start(None).await;
        assert!(d.ctx.awaiting_token());
        assert_awaiting(&d.url, transport).await;

        // Install.
        write(&d.token_file, Some(&format!("  {TOKEN_A}\n")));
        wait_token(&d, Some(TOKEN_A)).await;
        let a = client(&d.url, Some(TOKEN_A), transport).await;
        a.filesystem().stat(stat_root()).await.unwrap();
        let caps = a
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner();
        assert!(caps.initialized);
        let anon = client(&d.url, None, transport).await;
        assert_eq!(
            anon.filesystem()
                .stat(stat_root())
                .await
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );
        // Init with the current token is a no-op; another token is refused.
        let same = a
            .system()
            .init(InitRequest {
                token: TOKEN_A.into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        assert!(!same.token_changed);
        let other = a
            .system()
            .init(InitRequest {
                token: TOKEN_B.into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(other.code(), Code::PermissionDenied, "{transport:?}");
        assert_eq!(d.ctx.auth().token().as_deref(), Some(TOKEN_A));
        // Init without a token still sets defaults.
        a.system()
            .init(InitRequest {
                labels: [("k".to_owned(), "v".to_owned())].into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            d.ctx.init_state().labels.get("k").map(String::as_str),
            Some("v")
        );

        // Rotate: the old token is refused, the new one works.
        write(&d.token_file, Some(TOKEN_B));
        wait_token(&d, Some(TOKEN_B)).await;
        let stale = client(&d.url, Some(TOKEN_A), transport).await;
        assert_eq!(
            stale
                .filesystem()
                .stat(stat_root())
                .await
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );
        let b = client(&d.url, Some(TOKEN_B), transport).await;
        b.filesystem().stat(stat_root()).await.unwrap();
        // Rotation keeps the Init defaults (same claim).
        assert!(d.ctx.init_state().labels.contains_key("k"));

        // Revoke: back to awaiting, Init defaults reset.
        write(&d.token_file, None);
        wait_token(&d, None).await;
        assert!(d.ctx.awaiting_token());
        assert!(d.ctx.init_state().labels.is_empty());
        let b = client(&d.url, Some(TOKEN_B), transport).await;
        let refused = b.filesystem().stat(stat_root()).await.unwrap_err();
        assert_eq!(refused.code(), Code::FailedPrecondition);
        assert_eq!(reason(&refused), ErrorReason::NotInitialized as i32);
        assert_awaiting(&d.url, transport).await;

        // A removed file also means "no token"; a new claim installs again.
        write(&d.token_file, Some(TOKEN_A));
        wait_token(&d, Some(TOKEN_A)).await;
        std::fs::remove_file(&d.token_file).unwrap();
        wait_token(&d, None).await;
        write(&d.token_file, Some(TOKEN_B));
        wait_token(&d, Some(TOKEN_B)).await;
        d.ctx.request_shutdown(false);
    }
}

#[tokio::test]
async fn token_present_at_start_is_live_immediately() {
    let d = start(Some(TOKEN_A)).await;
    assert!(!d.ctx.awaiting_token());
    for transport in TRANSPORTS {
        client(&d.url, Some(TOKEN_A), transport)
            .await
            .filesystem()
            .stat(stat_root())
            .await
            .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn insecure_or_malformed_files_are_refused() {
    use std::os::unix::fs::PermissionsExt as _;
    let d = start(None).await;
    // World-readable: refused (not running as root in a container here).
    let tmp = d.token_file.with_extension("tmp");
    std::fs::write(&tmp, TOKEN_A).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::rename(&tmp, &d.token_file).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    if !d.ctx.config().token_file_allow_world_readable {
        assert!(d.ctx.awaiting_token(), "0644 must not be accepted");
    }
    // Fixing the mode installs it.
    std::fs::set_permissions(&d.token_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    wait_token(&d, Some(TOKEN_A)).await;
    // A malformed token revokes (fail closed) rather than keeping the old one.
    write(&d.token_file, Some("bad token with spaces"));
    wait_token(&d, None).await;
    write(&d.token_file, Some("short"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(d.ctx.awaiting_token());
}

#[tokio::test]
async fn rotation_ends_streams_and_forwards_opened_with_the_old_token() {
    for transport in TRANSPORTS {
        let d = start(Some(TOKEN_A)).await;
        let a = client(&d.url, Some(TOKEN_A), transport).await;
        let dir = tempfile::tempdir().unwrap();
        let mut stream = a
            .filesystem()
            .watch_dir(WatchDirRequest {
                path: dir.path().to_string_lossy().into_owned(),
                recursive: false,
                keepalive_interval: None,
            })
            .await
            .unwrap()
            .into_inner();
        stream.message().await.unwrap().unwrap();
        // A forward to a loopback listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        a.tunnel()
            .forward(ForwardRequest {
                port: port as u32,
                ..Default::default()
            })
            .await
            .unwrap();

        write(&d.token_file, Some(TOKEN_B));
        wait_token(&d, Some(TOKEN_B)).await;

        // The stream ends (error or EOF) within the bound.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..1000 {
                match stream.message().await {
                    Ok(Some(_)) => continue,
                    _ => return true,
                }
            }
            false
        })
        .await;
        assert_eq!(
            ended,
            Ok(true),
            "{transport:?}: stream outlived the rotation"
        );
        let b = client(&d.url, Some(TOKEN_B), transport).await;
        let forwards = b
            .tunnel()
            .list_forwards(ListForwardsRequest {})
            .await
            .unwrap()
            .into_inner()
            .forwards;
        assert!(forwards.is_empty(), "{transport:?}: {forwards:?}");
        d.ctx.request_shutdown(false);
    }
}

#[tokio::test]
async fn concurrent_callers_during_rotation_see_only_valid_outcomes() {
    let d = start(Some(TOKEN_A)).await;
    let tokens: Vec<String> = (0..6).map(|i| format!("{TOKEN_A}-{i}")).collect();
    let mut tasks = Vec::new();
    for (i, transport) in TRANSPORTS.iter().cycle().take(8).enumerate() {
        let url = d.url.clone();
        let transport = *transport;
        let tokens = tokens.clone();
        tasks.push(tokio::spawn(async move {
            let mut ok = 0u32;
            for n in 0..60 {
                let token = &tokens[(i + n) % tokens.len()];
                let c = client(&url, Some(token), transport).await;
                match c.filesystem().stat(stat_root()).await {
                    Ok(_) => ok += 1,
                    Err(e) => assert!(
                        matches!(
                            e.code(),
                            Code::Unauthenticated
                                | Code::FailedPrecondition
                                | Code::Unavailable
                                | Code::Unknown
                                | Code::Internal
                                | Code::Cancelled
                        ),
                        "unexpected {e:?}"
                    ),
                }
            }
            ok
        }));
    }
    for (n, token) in tokens.iter().enumerate() {
        if n % 3 == 2 {
            write(&d.token_file, None);
            wait_token(&d, None).await;
        }
        write(&d.token_file, Some(token));
        wait_token(&d, Some(token)).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(60), task)
            .await
            .expect("caller task bounded")
            .unwrap();
    }
    // Final state is the last token, and only it works.
    let last = tokens.last().unwrap();
    assert_eq!(d.ctx.auth().token().as_deref(), Some(last.as_str()));
    client(&d.url, Some(last), TransportPreference::Native)
        .await
        .filesystem()
        .stat(stat_root())
        .await
        .unwrap();
}
