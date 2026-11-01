//! `Spaces::rename` through both hosts the SDK has: embedded and a real
//! daemon on loopback (`RenameSpace`). A Space added by address (a mock
//! cua-spacesd) keeps its id and lists its new name; bad names are refused
//! before any host is asked. Nothing starts a container or VM, and nothing
//! touches `~/.cua`.

use async_trait::async_trait;
use cua_daemon::{
    Runtime, RuntimeConfig, fixtures,
    server::{self, ServerConfig},
};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, RuntimeResult,
};
use cua_sdk::{Cua, CuaConfig, CuaError};
use std::sync::Arc;
use std::time::Duration;

/// A local runtime with nothing in it (no host engine is probed).
struct Empty;

#[async_trait]
impl LocalRuntime for Empty {
    fn backend(&self) -> String {
        "qemu".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        Err(RuntimeError::NotFound(name.into()))
    }
}

async fn renames(cua: &Cua, url: &str) {
    let spaces = cua.spaces();
    let studio = spaces
        .add(url.into(), None, Some("studio".into()))
        .await
        .unwrap();

    let renamed = spaces
        .rename(studio.id.clone(), "design-review".into())
        .await
        .unwrap();
    assert_eq!(renamed.id, studio.id);
    assert_eq!(renamed.display_name, "design-review");
    let listed = spaces
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == studio.id)
        .unwrap();
    assert_eq!(listed.display_name, "design-review");
    // Found by its new name.
    assert_eq!(
        spaces.resolve("design-review".into()).await.unwrap().id,
        studio.id
    );

    // Refused before any host is asked, in plain words.
    match spaces
        .rename(studio.id.clone(), "Design Review".into())
        .await
    {
        Err(CuaError::InvalidArgument(m)) => assert!(m.contains("lowercase letters"), "{m}"),
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
    match spaces.rename("local:nope".into(), "x".into()).await {
        Err(CuaError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_renames_a_space() {
    let env = fixtures::start_env(None, None).await;
    let dir = tempfile::tempdir().unwrap();
    let cua = Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        account_from_session: false,
        ..Default::default()
    })
    .unwrap();
    tokio::time::timeout(Duration::from_secs(60), renames(&cua, &env.url))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_renames_a_space() {
    let env = fixtures::start_env(None, None).await;
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        local: Some(Arc::new(Empty)),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    let h = server::start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: "daemon-token".into(),
            discovery_path: Some(dirs.path().join("daemon.json")),
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
    tokio::time::timeout(Duration::from_secs(60), renames(&cua, &env.url))
        .await
        .unwrap();
}

/// A daemon on loopback signed in to `relay` with `token`, and an SDK
/// connected to it.
async fn signed_in_daemon(
    dirs: &tempfile::TempDir,
    relay: &cua_host::testing::FakeRelay,
    token: &str,
) -> (server::DaemonHandle, Arc<Cua>) {
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        local: Some(Arc::new(Empty)),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    runtime
        .spaces()
        .set_relay(Some(cua_spaces::RelayAccount::new(
            &relay.url,
            Arc::new(cua_spaces::relay::StaticToken(token.into())),
        )));
    let h = server::start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some("127.0.0.1:0".parse().unwrap()),
            token: "daemon-token".into(),
            discovery_path: Some(dirs.path().join("daemon.json")),
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
    (h, cua)
}

/// Through the daemon the relay's two refusals keep their kind: a Space
/// shared with you is PermissionDenied (in the relay's plain words), a
/// sign-in the relay no longer takes is Unauthenticated (sign in again).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemon_says_permission_denied_for_a_space_shared_with_you_and_unauthenticated_for_an_expired_sign_in()
 {
    let relay = cua_host::testing::FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("bob-token", "bob", Some("bob@example.com"));
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: "space-00000000000b".into(),
                name: "studio".into(),
                allow: vec!["bob@example.com".into()],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        let dirs = tempfile::tempdir().unwrap();
        let (_h, bob) = signed_in_daemon(&dirs, &relay, "bob-token").await;
        match bob
            .spaces()
            .rename("relay:space-00000000000b".into(), "mine-now".into())
            .await
        {
            Err(CuaError::PermissionDenied(m)) => {
                assert_eq!(
                    m,
                    "studio is shared with you. Only its owner can rename it."
                )
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        assert_eq!(relay.machine("space-00000000000b").unwrap().name, "studio");

        let dirs = tempfile::tempdir().unwrap();
        let (_h, expired) = signed_in_daemon(&dirs, &relay, "expired-token").await;
        match expired
            .spaces()
            .rename("relay:space-00000000000b".into(), "mine-now".into())
            .await
        {
            Err(CuaError::Unauthenticated(m)) => assert!(m.contains("Sign in again"), "{m}"),
            other => panic!("expected Unauthenticated, got {other:?}"),
        }
    })
    .await
    .unwrap();
}
