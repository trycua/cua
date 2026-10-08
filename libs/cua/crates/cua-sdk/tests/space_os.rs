//! Connected relay metadata must come from the handshake without registering
//! the discovered machine locally. Only in-process fixtures and temporary homes.
use cua_daemon::{
    Runtime, RuntimeConfig, fixtures,
    server::{self, ServerConfig},
};
use cua_host::{StaticToken, relay::RegisterRequest, testing::FakeRelay};
use cua_sdk::Cua;
use cua_spaces::relay::RelayAccount;
use std::sync::Arc;

#[tokio::test]
async fn generic_space_preserves_registered_os_metadata() {
    let home = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(home.path().join("sandboxes")),
        spaces_home: Some(home.path().join("cua")),
        ..Default::default()
    })
    .unwrap();
    let id = "direct:127.0.0.1:9";
    runtime
        .spaces()
        .registry()
        .upsert(
            cua_proto::daemon::v1::Space {
                id: id.into(),
                name: "Generic service".into(),
                os: "macos".into(),
                os_name: "Darwin".into(),
                os_pretty_name: "macOS".into(),
                ..Default::default()
            },
            cua_spaces::registry::Credential {
                // Declared MCP services have no spacesd handshake. Constructing
                // the handle does not contact this endpoint.
                service_urls: [("service".into(), "http://127.0.0.1:9/mcp".into())].into(),
                ..Default::default()
            },
        )
        .unwrap();
    let cua = Cua::from_runtime(runtime);
    let space = cua.spaces().space(id.into()).await.unwrap();
    assert_eq!(space.info().os, "macos");
    assert_eq!(space.info().os_name, "Darwin");
    assert_eq!(space.info().os_pretty_name, "macOS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovered_relay_os_survives_embedded_and_daemon_connections() {
    for daemon in [false, true] {
        for registered in [false, true] {
            for (family, expected) in [
                (Some(2), "macos"),
                (Some(3), "windows"),
                (Some(1), "linux"),
                (None, ""),
                (Some(0), ""),
                (Some(999), ""),
            ] {
                let relay = FakeRelay::start().await;
                relay.legacy_enrollment(true);
                relay.add_account("owner-token", "owner", None);
                let id = "00112233445566778899aabbccddeeff";
                cua_host::RelayClient::new(&relay.url)
                    .unwrap()
                    .register(
                        "owner-token",
                        &RegisterRequest {
                            id: id.into(),
                            name: "Test host".into(),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                let env = fixtures::start_env(None, None).await;
                *env.mock.state.os.lock().unwrap() =
                    Some(family.map(|family| cua_proto::env::v1::OperatingSystem {
                        family,
                        name: "Live OS".into(),
                        pretty_name: "Live OS 1".into(),
                        ..Default::default()
                    }));
                relay.tunnel(id, &env.url);
                relay.set_online(id, true, "0.0.0-mock");
                let home = tempfile::tempdir().unwrap();
                let runtime = Runtime::new(RuntimeConfig {
                    state_dir: Some(home.path().join("sandboxes")),
                    spaces_home: Some(home.path().join("cua")),
                    ..Default::default()
                })
                .unwrap();
                runtime.spaces().set_relay(Some(RelayAccount::new(
                    &relay.url,
                    Arc::new(StaticToken("owner-token".into())),
                )));
                let registry_home = home.path().join("cua");
                if registered {
                    runtime
                        .spaces()
                        .registry()
                        .upsert(
                            cua_proto::daemon::v1::Space {
                                id: format!("relay:{id}"),
                                name: "Test host".into(),
                                os: "stale-family".into(),
                                os_name: "Stale OS".into(),
                                os_pretty_name: "Stale OS 0".into(),
                                ..Default::default()
                            },
                            Default::default(),
                        )
                        .unwrap();
                }

                let (cua, server) = if daemon {
                    let h = server::start(
                        runtime,
                        ServerConfig {
                            socket_path: Some(home.path().join("cua.sock")),
                            loopback: Some("127.0.0.1:0".parse().unwrap()),
                            token: "daemon-token".into(),
                            discovery_path: Some(home.path().join("daemon.json")),
                            bridge_ticket_ttl: std::time::Duration::from_secs(30),
                        },
                    )
                    .await
                    .unwrap();
                    (
                        Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap(),
                        Some(h),
                    )
                } else {
                    (Cua::from_runtime(runtime), None)
                };
                let spaces = cua.spaces();
                let rows = spaces.list().await.unwrap();
                let key = format!("relay:{id}");
                assert!(
                    rows.iter()
                        .any(|r| r.id == key && (registered || r.os.is_empty()))
                );
                let connected = spaces.space(key.clone()).await.unwrap();
                assert_eq!(
                    connected.info().os,
                    expected,
                    "daemon={daemon}, family={family:?}"
                );
                assert_eq!(
                    connected.info().os_name,
                    if expected.is_empty() { "" } else { "Live OS" }
                );
                assert_eq!(
                    connected.info().os_pretty_name,
                    if expected.is_empty() { "" } else { "Live OS 1" }
                );
                assert!(
                    registered || !registry_home.join("spaces.json").exists(),
                    "discovery must not register a machine"
                );
                assert_eq!(connected.info().id, key);
                assert_eq!(connected.info().name, "Test host");
                if !registered {
                    // Connected info does not backfill discovery/resolve or
                    // make those APIs probe a now-offline host.
                    relay.set_online(id, false, "0.0.0-mock");
                    let listed = spaces
                        .list()
                        .await
                        .unwrap()
                        .into_iter()
                        .find(|r| r.id == key)
                        .unwrap();
                    assert!(listed.os.is_empty());
                    assert!(spaces.resolve(key.clone()).await.unwrap().os.is_empty());
                }
                drop(connected);
                drop(cua);
                if let Some(server) = server {
                    server.shutdown();
                    server.wait().await;
                }
            }
        }
    }
}

/// A relay id resolves and connects with no prior list, including a machine
/// that joins after the first read, in both topologies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_id_resolves_before_any_list_including_a_late_join() {
    for daemon in [false, true] {
        let relay = FakeRelay::start().await;
        relay.legacy_enrollment(true);
        relay.add_account("owner-token", "owner", None);
        let id = "00112233445566778899aabbccddeeff";
        let joined = "ffeeddccbbaa99887766554433221100";
        let register = async |machine: &str, name: &str| {
            cua_host::RelayClient::new(&relay.url)
                .unwrap()
                .register(
                    "owner-token",
                    &RegisterRequest {
                        id: machine.into(),
                        name: name.into(),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        };
        register(id, "Test host").await;
        let env = fixtures::start_env(None, None).await;
        relay.tunnel(id, &env.url);
        relay.set_online(id, true, "0.0.0-mock");
        let home = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(RuntimeConfig {
            state_dir: Some(home.path().join("sandboxes")),
            spaces_home: Some(home.path().join("cua")),
            ..Default::default()
        })
        .unwrap();
        runtime.spaces().set_relay(Some(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("owner-token".into())),
        )));
        let (cua, server) = if daemon {
            let h = server::start(
                runtime,
                ServerConfig {
                    socket_path: Some(home.path().join("cua.sock")),
                    loopback: Some("127.0.0.1:0".parse().unwrap()),
                    token: "daemon-token".into(),
                    discovery_path: Some(home.path().join("daemon.json")),
                    bridge_ticket_ttl: std::time::Duration::from_secs(30),
                },
            )
            .await
            .unwrap();
            (
                Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap(),
                Some(h),
            )
        } else {
            (Cua::from_runtime(runtime), None)
        };
        let spaces = cua.spaces();
        let key = format!("relay:{id}");
        let resolved = spaces.resolve(key.clone()).await.unwrap();
        assert_eq!(resolved.name, "Test host", "daemon={daemon}");
        let connected = spaces.space(key).await.unwrap();
        assert_eq!(connected.info().name, "Test host");
        register(joined, "Late host").await;
        let late = spaces.resolve(format!("relay:{joined}")).await.unwrap();
        assert_eq!(late.name, "Late host", "daemon={daemon}");
        let missing = spaces
            .resolve("direct:127.0.0.1:9".into())
            .await
            .unwrap_err();
        assert!(
            matches!(missing, cua_sdk::CuaError::NotFound(_)),
            "{missing}"
        );
        assert!(!home.path().join("cua/spaces.json").exists());
        drop(connected);
        drop(cua);
        if let Some(server) = server {
            server.shutdown();
            server.wait().await;
        }
    }
}
