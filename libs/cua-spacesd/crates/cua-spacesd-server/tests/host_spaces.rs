// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A host that provides Spaces, in process: the real relay (account mode,
//! a fake OIDC issuer), the driver joined to it with a host policy and the
//! `HostSpacesService` provider, and a fake cua daemon on a Unix socket.
//!
//! - The owner reaches `HostSpacesService` through the relay; the driver
//!   forwards to the daemon with the relay-verified caller (never one a
//!   client sent), and the daemon's errors come back as they are.
//! - With the host setting `share_desktop` off, a relayed caller reaches
//!   nothing else: no processes, no files, no MCP (the host's own desktop
//!   and files stay private). The local token holder is unaffected.
//! - A view-only share and an account the host is not shared with are
//!   refused; `provide_spaces` off refuses with how to turn it on.

#![cfg(unix)]

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_proto::env::v1::host_spaces_service_server::{HostSpacesService, HostSpacesServiceServer};
use cua_proto::env::v1::*;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::host_spaces::{HostSpacesProvider, CALLER_METADATA};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};

use common::*;

const ISSUER: &str = "https://auth.test/realms/cua";
const HOST_TOKEN: &str = "host-env-token";

/// The fake daemon: records who each forwarded call was for.
#[derive(Clone, Default)]
struct FakeDaemon {
    callers: Arc<Mutex<Vec<String>>>,
}

#[tonic::async_trait]
impl HostSpacesService for FakeDaemon {
    async fn get_host_spaces(
        &self,
        req: tonic::Request<GetHostSpacesRequest>,
    ) -> Result<tonic::Response<GetHostSpacesResponse>, tonic::Status> {
        let who = req
            .metadata()
            .get(CALLER_METADATA)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        self.callers.lock().unwrap().push(who);
        Ok(tonic::Response::new(GetHostSpacesResponse {
            name: "Mac mini (spare)".into(),
            ..Default::default()
        }))
    }

    async fn create_host_space(
        &self,
        _: tonic::Request<CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<CreateHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::resource_exhausted(
            "Mac mini (spare) already runs 2 macOS VMs",
        ))
    }

    async fn delete_cloud_space(
        &self,
        req: tonic::Request<DeleteCloudSpaceRequest>,
    ) -> Result<tonic::Response<DeleteCloudSpaceResponse>, tonic::Status> {
        let who = req
            .metadata()
            .get(CALLER_METADATA)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        self.callers.lock().unwrap().push(who);
        Ok(tonic::Response::new(DeleteCloudSpaceResponse {
            message: format!("Deleted relay:{}", req.into_inner().space),
        }))
    }

    async fn delete_host_space(
        &self,
        _: tonic::Request<DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<DeleteHostSpaceResponse>, tonic::Status> {
        Ok(tonic::Response::new(DeleteHostSpaceResponse {
            message: "deleted".into(),
        }))
    }

    async fn set_host_space_power(
        &self,
        _: tonic::Request<SetHostSpacePowerRequest>,
    ) -> Result<tonic::Response<SetHostSpacePowerResponse>, tonic::Status> {
        Ok(tonic::Response::new(SetHostSpacePowerResponse::default()))
    }

    async fn cancel_host_space(
        &self,
        req: tonic::Request<CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<CancelHostSpaceResponse>, tonic::Status> {
        let who = req
            .metadata()
            .get(CALLER_METADATA)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        self.callers.lock().unwrap().push(who);
        Ok(tonic::Response::new(CancelHostSpaceResponse {
            message: format!("Cancelled {}", req.into_inner().space),
        }))
    }
}

async fn serve_daemon(socket: &Path, daemon: FakeDaemon) {
    let listener = tokio::net::UnixListener::bind(socket).unwrap();
    let router =
        tonic::service::Routes::new(HostSpacesServiceServer::new(daemon)).into_axum_router();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
}

fn write_policy(path: &Path, owner: &str, desktop: bool, provide: bool, socket: &Path) {
    std::fs::write(
        path,
        serde_json::json!({
            "owner": owner,
            "owner_email": format!("{owner}@example.com"),
            "allow": [],
            "trust_relay_allowlist": true,
            "sharing": true,
            "share_desktop": desktop,
            "provide_spaces": provide,
            "spaces_daemon": {"socket": socket, "cua_home": ""},
        })
        .to_string(),
    )
    .unwrap();
}

async fn api(addr: SocketAddr, method: &str, path: &str, token: &str, body: serde_json::Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(full(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = http_client().request(request).await.unwrap();
    assert!(
        response.status().is_success(),
        "{method} {path}: {}",
        response.status()
    );
}

async fn client(url: &str, token: &str) -> SpacesdClient {
    SpacesdClient::connect(
        ConnectOptions::parse(url)
            .unwrap()
            .transport(TransportPreference::Native)
            .probe(false)
            .token(token.to_string()),
    )
    .await
    .unwrap()
}

async fn code_of<T>(r: Result<tonic::Response<T>, tonic::Status>) -> (tonic::Code, String) {
    match r {
        Ok(_) => (tonic::Code::Ok, String::new()),
        Err(s) => (s.code(), s.message().to_string()),
    }
}

#[tokio::test]
async fn a_host_provides_spaces_through_the_relay_and_keeps_its_desktop_private() {
    let dir = tempfile::tempdir().unwrap();
    let socket: PathBuf = dir.path().join("cua.sock");
    let daemon = FakeDaemon::default();
    serve_daemon(&socket, daemon.clone()).await;

    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        // This test is about host-provided Spaces, not device enrollment
        // (see cua-relay's own tests/device_enrollment.rs for that); the
        // migration grace period ending by default (S3) would otherwise
        // require an enrolled device here for no reason relevant to what
        // this test checks.
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);

    // The owner registers the host (what `cua host setup` does).
    let machine_id = format!("host{}", uuid::Uuid::new_v4().simple());
    let reg: serde_json::Value = {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("http://{relay_addr}/v1/machines"))
            .header("authorization", format!("Bearer {ada}"))
            .header("content-type", "application/json")
            .body(full(
                serde_json::to_vec(
                    &serde_json::json!({"id": machine_id, "name": "Mac mini (spare)"}),
                )
                .unwrap(),
            ))
            .unwrap();
        let response = http_client().request(request).await.unwrap();
        serde_json::from_slice(&body_bytes(response, 1 << 20).await).unwrap()
    };
    let machine_token = reg["machine_token"].as_str().unwrap().to_string();

    // The host's driver: joined, with its policy (desktop off, providing
    // Spaces) and the HostSpacesService provider.
    let policy = dir.path().join("host.json");
    write_policy(&policy, "ada", false, true, &socket);
    let data = tempfile::tempdir().unwrap();
    let ctx = ServerContext::new(
        ServerConfig {
            data_dir: data.path().to_path_buf(),
            shutdown_grace: Duration::from_secs(1),
            ..ServerConfig::default()
        },
        Some(HOST_TOKEN.into()),
    );
    let server = ServerBuilder::new(ctx.clone())
        .provider(Arc::new(HostSpacesProvider::new(&policy)))
        .build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    let join = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        machine_token,
        machine_id.clone(),
        addr,
    );
    ctx.auth().set_external(Arc::new(
        cua_spacesd_server::relay_account::RelayAssertionAuth::new(
            machine_id.clone(),
            join.account.clone(),
            Some(policy.clone()),
        ),
    ));
    ctx.mark_joined_at_start();
    let shutdown = tokio_util::sync::CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, shutdown.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&machine_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(relay.machine_ids().contains(&machine_id), "joined");
    let url = format!("http://{relay_addr}/m/{machine_id}");

    // The owner, through the relay: capabilities and HostSpacesService.
    let owner = client(&url, &ada).await;
    let caps = owner
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(caps
        .features
        .iter()
        .any(|f| f.name == "host_spaces" && f.supported));
    let got = owner
        .host_spaces()
        .get_host_spaces(GetHostSpacesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(got.name, "Mac mini (spare)");
    let who: serde_json::Value = serde_json::from_str(&daemon.callers.lock().unwrap()[0]).unwrap();
    assert_eq!(who["account"], "ada");
    assert_eq!(who["role"], "owner");
    assert_eq!(who["via"], "relay");
    assert_eq!(who["email"], "ada@example.com");
    // The daemon's own errors come back as they are.
    let (code, message) = code_of(
        owner
            .host_spaces()
            .create_host_space(CreateHostSpaceRequest::default())
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::ResourceExhausted);
    assert!(message.contains("2 macOS VMs"), "{message}");
    // A relayed caller cancels its own create on a host that does not
    // share its desktop, with who it is attached.
    let cancelled = owner
        .host_spaces()
        .cancel_host_space(CancelHostSpaceRequest {
            space: "space-ada00001".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(cancelled.message, "Cancelled space-ada00001");
    let who: serde_json::Value =
        serde_json::from_str(daemon.callers.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(who["account"], "ada");

    // The desktop is not shared: no processes, files or MCP, even for the
    // owner, and the refusal says why.
    let (code, message) = code_of(
        owner
            .process()
            .start_process(StartProcessRequest {
                config: Some(ProcessConfig {
                    command: "/bin/sh".into(),
                    args: vec!["-c".into(), "id".into()],
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::PermissionDenied);
    assert!(message.contains("does not share its desktop"), "{message}");
    let (code, _) = code_of(owner.filesystem().stat(StatRequest::default()).await).await;
    assert_eq!(code, tonic::Code::PermissionDenied);
    let mcp = http::Request::builder()
        .method("POST")
        .uri(format!("{url}/mcp"))
        .header("authorization", format!("Bearer {ada}"))
        .header("content-type", "application/json")
        .body(full(b"{}".to_vec()))
        .unwrap();
    let status = http_client().request(mcp).await.unwrap().status();
    assert!(
        status == http::StatusCode::UNAUTHORIZED
            || status == http::StatusCode::FORBIDDEN
            || status == http::StatusCode::NOT_IMPLEMENTED,
        "/mcp through the relay: {status}"
    );
    // The host's own token holder still reaches its shell.
    let local = client(&format!("http://{addr}"), HOST_TOKEN).await;
    let (code, _) = code_of(
        local
            .filesystem()
            .stat(StatRequest {
                path: data.path().display().to_string(),
                ..Default::default()
            })
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::Ok);
    local
        .host_spaces()
        .get_host_spaces(GetHostSpacesRequest {})
        .await
        .unwrap();
    let who: serde_json::Value =
        serde_json::from_str(daemon.callers.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(
        (who["account"].as_str(), who["via"].as_str()),
        (Some("local"), Some("token"))
    );

    // Bob: not shared at all (the relay refuses), then shared to watch
    // (the driver refuses HostSpacesService: view-only).
    let stranger = client(&url, &bob).await;
    let (code, _) = code_of(
        stranger
            .host_spaces()
            .get_host_spaces(GetHostSpacesRequest {})
            .await,
    )
    .await;
    assert_ne!(code, tonic::Code::Ok, "not shared");
    api(
        relay_addr,
        "PATCH",
        &format!("/v1/machines/{machine_id}"),
        &ada,
        serde_json::json!({"viewers": ["bob@example.com"]}),
    )
    .await;
    let viewer = client(&url, &bob).await;
    let (code, message) = code_of(
        viewer
            .host_spaces()
            .get_host_spaces(GetHostSpacesRequest {})
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::PermissionDenied, "{message}");
    assert!(message.contains("view-only"), "{message}");
    let calls = daemon.callers.lock().unwrap().len();
    // Ada's get, then her cancel.
    assert_eq!(calls, 3, "nothing forwarded for bob");

    // Not providing Spaces: refused with how to turn it on.
    write_policy(&policy, "ada", false, false, &socket);
    let (code, message) = code_of(
        owner
            .host_spaces()
            .get_host_spaces(GetHostSpacesRequest {})
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::FailedPrecondition, "{message}");
    assert!(message.contains("--provide-spaces on"), "{message}");
    // A Space in the owner's own cloud that this machine created is
    // deleted here whether or not it provides Spaces, with the verified
    // caller (the daemon checks it is the owner).
    let deleted = owner
        .host_spaces()
        .delete_cloud_space(DeleteCloudSpaceRequest {
            space: "cloud-00000000000000aa".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(deleted.message, "Deleted relay:cloud-00000000000000aa");
    let who: serde_json::Value =
        serde_json::from_str(daemon.callers.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(
        (who["account"].as_str(), who["role"].as_str()),
        (Some("ada"), Some("owner"))
    );

    // Sharing the desktop again lets the owner in (the policy reloads).
    write_policy(&policy, "ada", true, true, &socket);
    let (code, message) = code_of(
        owner
            .filesystem()
            .stat(StatRequest {
                path: data.path().display().to_string(),
                ..Default::default()
            })
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::Ok, "{message}");
    shutdown.cancel();
}

/// A host in direct mode (`cua host setup --direct ... --provide-spaces`):
/// no relay. The env token holder reaches `HostSpacesService` on the direct
/// listener (over loopback, a private peer) as the local owner; a caller
/// without the token is refused before anything reaches the daemon.
#[tokio::test]
async fn a_direct_host_serves_host_spaces_to_its_token_holder() {
    let dir = tempfile::tempdir().unwrap();
    let socket: PathBuf = dir.path().join("cua.sock");
    let daemon = FakeDaemon::default();
    serve_daemon(&socket, daemon.clone()).await;
    let policy = dir.path().join("host.json");
    std::fs::write(
        &policy,
        serde_json::json!({
            "owner": "local",
            "trust_relay_allowlist": false,
            "sharing": true,
            "share_desktop": false,
            "provide_spaces": true,
            "spaces_daemon": {"socket": socket, "cua_home": ""},
            "direct": {"listen": "127.0.0.1:0", "allow_any_address": false},
        })
        .to_string(),
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let ctx = ServerContext::new(
        ServerConfig {
            data_dir: data.path().to_path_buf(),
            shutdown_grace: Duration::from_secs(1),
            ..ServerConfig::default()
        },
        Some(HOST_TOKEN.into()),
    );
    let server = ServerBuilder::new(ctx)
        .provider(Arc::new(HostSpacesProvider::new(&policy)))
        .build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    let url = format!("http://{addr}");

    let local = client(&url, HOST_TOKEN).await;
    let caps = local
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(caps
        .features
        .iter()
        .any(|f| f.name == "host_spaces" && f.supported));
    let got = local
        .host_spaces()
        .get_host_spaces(GetHostSpacesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(got.name, "Mac mini (spare)");
    let who: serde_json::Value =
        serde_json::from_str(daemon.callers.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(
        (
            who["account"].as_str(),
            who["role"].as_str(),
            who["via"].as_str()
        ),
        (Some("local"), Some("owner"), Some("token"))
    );

    let stranger = client(&url, "not-the-token").await;
    let (code, _) = code_of(
        stranger
            .host_spaces()
            .get_host_spaces(GetHostSpacesRequest {})
            .await,
    )
    .await;
    assert_eq!(code, tonic::Code::Unauthenticated);
    assert_eq!(daemon.callers.lock().unwrap().len(), 1);
}
