// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A host deletes the Spaces its owner deleted while it was removed from
//! the account (`DELETE /v1/machines/{id}?spaces=delete` on the relay), the
//! next time it dials the relay: the real relay in process (a fake OIDC
//! issuer) and a fake cua daemon on a Unix socket.
//!
//! - Only the listed Spaces are deleted; each is acked; a failed delete is
//!   retried on the next sweep; a Space already gone is acked.
//! - End to end: an offline computer is removed with delete, comes back,
//!   and its Spaces are deleted and acked; one not on the list stays.
//! - Added again with the same id (a new token): the deletions still run
//!   first, and the deleted Space ids cannot be registered meanwhile.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_proto::env::v1::host_spaces_service_server::{HostSpacesService, HostSpacesServiceServer};
use cua_proto::env::v1::*;
use cua_relay::devices::DevicePolicy;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_spacesd_server::host_spaces::CALLER_METADATA;
use cua_spacesd_server::pending_deletions::{
    run, sweep, DaemonDeleter, PendingDeletionsConfig, SpaceDeleter,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

struct Env {
    base: String,
    ada: String,
    _issuer: FakeIssuer,
    _dir: tempfile::TempDir,
}

impl Env {
    async fn start() -> Env {
        let issuer = FakeIssuer::new(ISSUER);
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(RelayConfig {
            oidc: Some(Arc::new(OidcValidator::with_jwks(
                OidcConfig::new(ISSUER),
                issuer.jwks(),
            ))),
            device_policy: DevicePolicy {
                grace_secs: 14 * 86_400,
                ..DevicePolicy::default()
            },
            state_file: Some(dir.path().join("machines.json")),
            ..RelayConfig::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = relay.serve(listener, std::future::pending()).await;
        });
        let ada = issuer.token("ada-sub", Some("ada@example.com"), "cua-relay", 3000);
        Env {
            base,
            ada,
            _issuer: issuer,
            _dir: dir,
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (reqwest::StatusCode, Value) {
        let mut r = reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let s = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            s,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn try_register(&self, id: &str, host: Option<&str>) -> (reqwest::StatusCode, Value) {
        self.call(
            reqwest::Method::POST,
            "/v1/machines",
            &self.ada,
            Some(json!({"id": id, "name": id, "host": host})),
        )
        .await
    }

    async fn register(&self, id: &str, host: Option<&str>) -> String {
        let (s, v) = self.try_register(id, host).await;
        assert!(s.is_success(), "{s} {v}");
        v["machine_token"].as_str().unwrap().to_owned()
    }

    async fn remove_with_delete(&self, computer: &str) {
        let (s, v) = self
            .call(
                reqwest::Method::DELETE,
                &format!("/v1/machines/{computer}?spaces=delete"),
                &self.ada,
                None,
            )
            .await;
        assert_eq!(s, reqwest::StatusCode::NO_CONTENT, "{v}");
    }

    /// The Space ids still pending for `computer` (`None`: the record is
    /// gone and the token means nothing to the relay).
    async fn pending(&self, computer: &str, token: &str) -> Option<BTreeSet<String>> {
        let (s, v) = self
            .call(
                reqwest::Method::GET,
                &format!("/v1/machines/{computer}/pending-deletions"),
                token,
                None,
            )
            .await;
        if s == reqwest::StatusCode::UNAUTHORIZED {
            return None;
        }
        assert_eq!(s, reqwest::StatusCode::OK, "{v}");
        Some(
            v["spaces"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["id"].as_str().unwrap().to_owned())
                .collect(),
        )
    }

    async fn listed(&self) -> Vec<String> {
        let (_, v) = self
            .call(reqwest::Method::GET, "/v1/machines", &self.ada, None)
            .await;
        v["machines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    }
}

fn new_id(tag: &str) -> String {
    format!("{tag}{}", uuid::Uuid::new_v4().simple())
}

/// Local Spaces by relay id; the ones in `fail_once` fail their first
/// delete.
#[derive(Default)]
struct FakeSpaces {
    local: Mutex<BTreeSet<String>>,
    fail_once: Mutex<BTreeSet<String>>,
    asked: Mutex<Vec<String>>,
}

impl FakeSpaces {
    fn with(local: &[&str]) -> Arc<Self> {
        let s = Self::default();
        s.local
            .lock()
            .unwrap()
            .extend(local.iter().map(|s| s.to_string()));
        Arc::new(s)
    }

    fn local(&self) -> BTreeSet<String> {
        self.local.lock().unwrap().clone()
    }

    fn delete(&self, space: &str) -> Result<bool, String> {
        self.asked.lock().unwrap().push(space.to_owned());
        if self.fail_once.lock().unwrap().remove(space) {
            return Err("the VM is busy".into());
        }
        Ok(self.local.lock().unwrap().remove(space))
    }
}

#[tonic::async_trait]
impl SpaceDeleter for FakeSpaces {
    async fn delete(&self, space: &str) -> Result<bool, String> {
        FakeSpaces::delete(self, space)
    }
}

#[tokio::test]
async fn a_sweep_deletes_only_the_listed_spaces_and_retries_a_failure() {
    let env = Env::start().await;
    let host = new_id("m");
    let token = env.register(&host, None).await;
    let (a, b, gone) = (new_id("space-"), new_id("space-"), new_id("space-"));
    for kid in [&a, &b, &gone] {
        env.register(kid, Some(&host)).await;
    }
    let spaces = FakeSpaces::with(&[&a, &b, "space-not-on-the-list"]);
    spaces.fail_once.lock().unwrap().insert(b.clone());

    // Nothing pending yet: nothing is touched.
    let r = sweep(&env.base, &host, &token, &*spaces).await.unwrap();
    assert_eq!(r, Default::default());

    env.remove_with_delete(&host).await;
    let r = sweep(&env.base, &host, &token, &*spaces).await.unwrap();
    assert_eq!(r.deleted, vec![a.clone()]);
    assert_eq!(r.absent, vec![gone.clone()], "already gone: acked");
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].0, b);
    assert_eq!(
        env.pending(&host, &token).await,
        Some(BTreeSet::from([b.clone()])),
        "the failed one stays pending"
    );

    let r = sweep(&env.base, &host, &token, &*spaces).await.unwrap();
    assert_eq!((r.deleted, r.failed), (vec![b.clone()], vec![]));
    assert_eq!(env.pending(&host, &token).await, None, "all acked: cleared");
    assert_eq!(
        spaces.local(),
        BTreeSet::from(["space-not-on-the-list".to_owned()])
    );
    assert!(!spaces
        .asked
        .lock()
        .unwrap()
        .iter()
        .any(|s| s == "space-not-on-the-list"));
}

/// The host's cua daemon: deletes the Spaces it has, by relay id, and
/// records whether each call came as this machine's own (no caller).
#[derive(Clone)]
struct FakeDaemon {
    spaces: Arc<FakeSpaces>,
    forwarded: Arc<Mutex<Vec<String>>>,
}

#[tonic::async_trait]
impl HostSpacesService for FakeDaemon {
    async fn get_host_spaces(
        &self,
        _: tonic::Request<GetHostSpacesRequest>,
    ) -> Result<tonic::Response<GetHostSpacesResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("get_host_spaces"))
    }

    async fn create_host_space(
        &self,
        _: tonic::Request<CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<CreateHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("create_host_space"))
    }

    async fn cancel_host_space(
        &self,
        _: tonic::Request<CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<CancelHostSpaceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("cancel_host_space"))
    }

    async fn set_host_space_power(
        &self,
        _: tonic::Request<SetHostSpacePowerRequest>,
    ) -> Result<tonic::Response<SetHostSpacePowerResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("set_host_space_power"))
    }

    async fn set_host_warm(
        &self,
        _: tonic::Request<SetHostWarmRequest>,
    ) -> Result<tonic::Response<SetHostWarmResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("set_host_warm"))
    }

    async fn set_host_space_egress(
        &self,
        _: tonic::Request<SetHostSpaceEgressRequest>,
    ) -> Result<tonic::Response<SetHostSpaceEgressResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("set_host_space_egress"))
    }

    async fn diagnose_host(
        &self,
        _: tonic::Request<DiagnoseHostRequest>,
    ) -> Result<tonic::Response<DiagnoseHostResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("diagnose_host"))
    }

    async fn delete_host_space(
        &self,
        req: tonic::Request<DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<DeleteHostSpaceResponse>, tonic::Status> {
        if let Some(who) = req.metadata().get(CALLER_METADATA) {
            self.forwarded
                .lock()
                .unwrap()
                .push(who.to_str().unwrap_or("").to_owned());
        }
        let space = req.into_inner().space;
        match self.spaces.delete(&space) {
            Ok(true) => Ok(tonic::Response::new(DeleteHostSpaceResponse {
                message: format!("Deleted relay:{space}"),
            })),
            Ok(false) => Err(tonic::Status::not_found(format!(
                "{space} is not a Space this host provides"
            ))),
            Err(e) => Err(tonic::Status::unavailable(e)),
        }
    }

    async fn delete_cloud_space(
        &self,
        req: tonic::Request<DeleteCloudSpaceRequest>,
    ) -> Result<tonic::Response<DeleteCloudSpaceResponse>, tonic::Status> {
        Err(tonic::Status::not_found(format!(
            "relay:{} is not a Space in your cloud that this computer created",
            req.into_inner().space
        )))
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

/// A host: its policy names the fake daemon.
struct Host {
    dir: tempfile::TempDir,
    spaces: Arc<FakeSpaces>,
    forwarded: Arc<Mutex<Vec<String>>>,
}

impl Host {
    async fn start(local: &[&str]) -> Host {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        let spaces = FakeSpaces::with(local);
        let forwarded = Arc::new(Mutex::new(Vec::new()));
        serve_daemon(
            &socket,
            FakeDaemon {
                spaces: spaces.clone(),
                forwarded: forwarded.clone(),
            },
        )
        .await;
        std::fs::write(
            dir.path().join("host.json"),
            json!({
                "owner": "ada-sub",
                "provide_spaces": true,
                "spaces_daemon": {"socket": socket, "cua_home": ""},
            })
            .to_string(),
        )
        .unwrap();
        Host {
            dir,
            spaces,
            forwarded,
        }
    }

    /// The computer comes online: it dials the relay (refused or not) and
    /// sweeps on each dial, as `cua-spacesd join` with a host policy does.
    fn come_online(&self, env: &Env, id: &str, token_file: &Path) -> CancellationToken {
        let stop = CancellationToken::new();
        let dialing = Arc::new(tokio::sync::Notify::new());
        let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let local = unused.local_addr().unwrap();
        drop(unused);
        let mut join = cua_relay::client::JoinConfig::new(
            env.base.replace("http://", "ws://"),
            String::new(),
            id.to_owned(),
            local,
        );
        join.relay_token_file = Some(token_file.to_owned());
        join.max_backoff = Duration::from_millis(200);
        join.dialing = Some(dialing.clone());
        let mut cfg = PendingDeletionsConfig::new(
            join.relay_url.clone(),
            id.to_owned(),
            String::new(),
            dialing,
        );
        cfg.relay_token_file = Some(token_file.to_owned());
        cfg.min_interval = Duration::from_millis(100);
        cfg.retry_interval = Duration::from_millis(100);
        tokio::spawn(cua_relay::client::run(join, stop.clone()));
        tokio::spawn(run(
            cfg,
            Arc::new(DaemonDeleter::new(self.dir.path().join("host.json"))),
            stop.clone(),
        ));
        stop
    }
}

/// Waits (bounded) until `ok` holds.
macro_rules! eventually {
    ($what:expr, $ok:expr) => {{
        let mut done = false;
        for _ in 0..200 {
            if $ok {
                done = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(done, "timed out waiting for: {}", $what);
    }};
}

#[tokio::test]
async fn an_offline_computer_removed_with_delete_deletes_its_spaces_when_it_comes_back() {
    let env = Env::start().await;
    let id = new_id("m");
    let token = env.register(&id, None).await;
    let (a, b) = (new_id("space-"), new_id("space-"));
    env.register(&a, Some(&id)).await;
    env.register(&b, Some(&id)).await;
    let keep = new_id("space-");
    let host = Host::start(&[&a, &b, &keep]).await;
    host.spaces.fail_once.lock().unwrap().insert(b.clone());

    // Offline (never dialed), removed with its Spaces deleted: they leave
    // the account at once.
    env.remove_with_delete(&id).await;
    assert!(env.listed().await.is_empty());
    assert_eq!(
        env.pending(&id, &token).await,
        Some(BTreeSet::from([a.clone(), b.clone()]))
    );

    // It comes back.
    let token_file = host.dir.path().join("relay-token");
    std::fs::write(&token_file, &token).unwrap();
    let _stop = host.come_online(&env, &id, &token_file);
    eventually!(
        "both Spaces deleted and acked",
        env.pending(&id, &token).await.is_none()
    );
    assert_eq!(host.spaces.local(), BTreeSet::from([keep.clone()]));
    assert!(!host.spaces.asked.lock().unwrap().contains(&keep));
    assert!(
        host.forwarded.lock().unwrap().is_empty(),
        "the deletes are this machine's own calls"
    );
}

#[tokio::test]
async fn a_computer_added_again_still_deletes_its_deleted_spaces() {
    let env = Env::start().await;
    let id = new_id("m");
    let old_token = env.register(&id, None).await;
    let kid = new_id("space-");
    env.register(&kid, Some(&id)).await;
    let host = Host::start(&[&kid]).await;
    env.remove_with_delete(&id).await;

    // `cua host setup` adds it again under the same id with a new token
    // before it ever dialed: deleted means deleted.
    let new_token = env.register(&id, None).await;
    assert_ne!(old_token, new_token);
    let (s, _) = env.try_register(&kid, Some(&id)).await;
    assert_eq!(
        s,
        reqwest::StatusCode::GONE,
        "not registered again meanwhile"
    );

    let token_file = host.dir.path().join("relay-token");
    std::fs::write(&token_file, &new_token).unwrap();
    let _stop = host.come_online(&env, &id, &token_file);
    eventually!(
        "the deleted Space is deleted and acked",
        env.pending(&id, &new_token).await == Some(BTreeSet::new())
    );
    assert!(host.spaces.local().is_empty());
    assert!(!env.listed().await.contains(&kid));
    // Acked: the id is free again.
    env.register(&kid, Some(&id)).await;
}
