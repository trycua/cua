//! The daemon lists the Spaces running on this computer on the signed-in
//! account's relay by itself: a Space created through the runtime shows on
//! the relay without any call, leaves it when it stops or is deleted, and
//! stays off when its owner opted out. The relay is the local fake relay, the
//! Space's cua-spacesd a mock, the home a temp dir (never `~/.cua`).

use async_trait::async_trait;
use cua_daemon::server::{ServerConfig, start};
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_host::testing::FakeRelay;
use cua_sandbox_core::placement::On;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::{RelayAccount, SpaceCreate};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Runtime {
    port: u16,
    stopped: Mutex<Vec<String>>,
}

impl Runtime {
    fn instance(&self, name: &str, status: InstanceStatus) -> LocalInstance {
        LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ports: [(3211u16, self.port)].into(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        Ok(self.instance(&spec.name, InstanceStatus::Running))
    }
    fn power_control(&self, _: &str) -> Option<cua_sandbox_core::PowerControl> {
        Some(cua_sandbox_core::PowerControl::Stop)
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.stopped.lock().unwrap().push(name.into());
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(self.instance(name, InstanceStatus::Running))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, _: &str) -> Result<InstanceStatus, RuntimeError> {
        Ok(InstanceStatus::Running)
    }
    async fn delete(&self, _: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn endpoints(&self, name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(self.instance(name, InstanceStatus::Running).endpoints)
    }
}

async fn until<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn the_daemon_lists_and_unlists_spaces_on_the_account() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: set before the runtime or any thread reads it.
    unsafe { std::env::set_var("CUA_HOME", home.path()) };
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let driver = MockServer::start(MockAuth::default()).await;
    driver.state.advertise(&["relay_attach"]);
    let port: u16 = driver.url().rsplit(':').next().unwrap().parse().unwrap();
    // The mock driver "dials the relay" when asked to attach.
    {
        let state = driver.state.clone();
        let relay = relay.clone();
        tokio::spawn(async move {
            let mut last: Option<String> = None;
            loop {
                let now = state.relay_attached.lock().unwrap().clone();
                match (now, &last) {
                    (Some(a), _) => {
                        relay.set_online(&a.machine_id, true, "0.1.0");
                        last = Some(a.machine_id);
                    }
                    (None, Some(m)) => {
                        relay.set_online(m, false, "0.1.0");
                        last = None;
                    }
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
    }
    let rt = Arc::new(Runtime {
        port,
        stopped: Mutex::default(),
    });
    let runtime = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(home.path().join("sandboxes")),
        spaces_home: Some(home.path().to_path_buf()),
        local: Some(rt.clone()),
        ..Default::default()
    })
    .unwrap();
    let spaces = runtime.spaces().clone();
    spaces.set_relay(Some(RelayAccount::new(
        &relay.url,
        Arc::new(cua_host::StaticToken("ada-token".into())),
    )));
    let handle = start(
        runtime,
        ServerConfig {
            socket_path: None,
            loopback: Some(([127, 0, 0, 1], 0).into()),
            token: "owner".into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();

    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    let rows = || async {
        client
            .machines("ada-token")
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.id.starts_with("space-"))
            .collect::<Vec<_>>()
    };
    let make = |name: &str, relay: Option<bool>| {
        let spaces = spaces.clone();
        let opts = SpaceCreate {
            on: Some(On::Local),
            image: Some("linux".into()),
            name: Some(name.into()),
            timeout: Some(Duration::from_secs(30)),
            relay,
            ..Default::default()
        };
        async move { spaces.create(opts).await.unwrap().ready().unwrap().id }
    };

    // Created: on the account within moments, with nobody calling anything.
    let id = make("phone-visible", None).await;
    until("the Space is on the relay", async || {
        (rows().await.len() == 1).then_some(())
    })
    .await;
    assert!(rows().await[0].online);

    // Opted out: never listed.
    make("private", Some(false)).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(rows().await.len(), 1);

    // Stopped: it leaves the account.
    spaces.stop(&id).await.unwrap();
    until("the stopped Space left the relay", async || {
        rows().await.is_empty().then_some(())
    })
    .await;

    // Started again: back.
    spaces.start(&id).await.unwrap();
    until("the started Space is back", async || {
        (rows().await.len() == 1).then_some(())
    })
    .await;

    // Deleted: gone.
    spaces.delete(&id).await.unwrap();
    until("the deleted Space left the relay", async || {
        rows().await.is_empty().then_some(())
    })
    .await;
    handle.shutdown();
}
