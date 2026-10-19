// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Gateway bootstrap state machine: a non-loopback driver with no token
//! serves only GetCapabilities/Health/Init; the first Init carrying a token
//! installs it (and persists it 0600); later Inits need that token.

use std::time::Duration;

use cua_proto::env::v1::*;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use tonic::Code;

struct Bootstrapped {
    url: String,
    token_file: std::path::PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// A driver whose config says "non-loopback, no token, bootstrap allowed"
/// (so the context is in bootstrap mode) but that is served on loopback.
async fn start() -> Bootstrapped {
    let data = tempfile::tempdir().unwrap();
    let run = tempfile::tempdir().unwrap();
    let token_file = run.path().join("env-token");
    let config = ServerConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        insecure_bootstrap: true,
        bootstrap_token_file: Some(token_file.clone()),
        data_dir: data.path().to_path_buf(),
        shutdown_grace: Duration::from_secs(1),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, None);
    let server = ServerBuilder::new(ctx).build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    Bootstrapped {
        url: format!("http://{addr}"),
        token_file,
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

#[tokio::test]
async fn bootstrap_first_init_wins_and_persists() {
    for transport in [TransportPreference::Native, TransportPreference::GrpcWeb] {
        let d = start().await;
        let anon = client(&d.url, None, transport).await;

        // Uninitialized: capabilities/health answer, everything else is
        // FAILED_PRECONDITION / NOT_INITIALIZED.
        let caps = anon
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap()
            .into_inner();
        assert!(!caps.initialized, "{transport:?}");
        anon.system().health(HealthRequest {}).await.unwrap();
        let refused = anon
            .filesystem()
            .stat(StatRequest {
                path: "/".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(refused.code(), Code::FailedPrecondition, "{transport:?}");
        assert_eq!(reason(&refused), ErrorReason::NotInitialized as i32);

        // An Init without a token does not end bootstrap.
        let empty = anon
            .system()
            .init(InitRequest::default())
            .await
            .unwrap_err();
        assert_eq!(empty.code(), Code::FailedPrecondition);
        assert!(!d.token_file.exists());

        // The first Init with a token installs it and persists it 0600.
        let r = anon
            .system()
            .init(InitRequest {
                token: "first-token".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        assert!(r.token_changed);
        assert_eq!(
            std::fs::read_to_string(&d.token_file).unwrap().trim(),
            "first-token"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&d.token_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        // A second unauthenticated Init (even with another token) is refused.
        let second = anon
            .system()
            .init(InitRequest {
                token: "attacker".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(second.code(), Code::Unauthenticated, "{transport:?}");
        // So is any other anonymous call now.
        let anon_stat = anon
            .filesystem()
            .stat(StatRequest {
                path: "/".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(anon_stat.code(), Code::Unauthenticated);

        // The token holder is in: calls work and Init can rotate.
        let owner = client(&d.url, Some("first-token"), transport).await;
        owner
            .filesystem()
            .stat(StatRequest {
                path: "/".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            owner
                .system()
                .get_capabilities(GetCapabilitiesRequest {})
                .await
                .unwrap()
                .into_inner()
                .initialized
        );
        let again = owner
            .system()
            .init(InitRequest {
                token: "first-token".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        assert!(!again.token_changed);
        let wrong = client(&d.url, Some("attacker"), transport).await;
        assert_eq!(
            wrong
                .system()
                .init(InitRequest {
                    token: "x".into(),
                    ..Default::default()
                })
                .await
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );
    }
}

#[tokio::test]
async fn concurrent_bootstrap_inits_have_exactly_one_winner() {
    let d = start().await;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let url = d.url.clone();
        tasks.push(tokio::spawn(async move {
            let c = client(&url, None, TransportPreference::Native).await;
            c.system()
                .init(InitRequest {
                    token: format!("token-{i}"),
                    ..Default::default()
                })
                .await
                .map(|_| i)
        }));
    }
    let mut winners = Vec::new();
    for t in tasks {
        if let Ok(i) = t.await.unwrap() {
            winners.push(i);
        }
    }
    assert_eq!(winners.len(), 1, "{winners:?}");
    let persisted = std::fs::read_to_string(&d.token_file).unwrap();
    assert_eq!(persisted.trim(), format!("token-{}", winners[0]));
}
