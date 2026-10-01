//! `relay:<id>`: the account's machines from the relay directory
//! (`cua_host::testing::FakeRelay`) and connections to `<relay>/m/<id>`
//! carrying the account token (`cua_spacesd_client::testing::MockServer` emulating the
//! relay path prefix and checking the bearer).

use cua_host::testing::FakeRelay;
use cua_spaces::relay::{AccountTokens, StaticToken};
use cua_spaces::{Provider, RelayAccount, SpaceId, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const MACHINE: &str = "0123abcd4567ef89";

struct Counting {
    token: String,
    calls: AtomicU32,
}

#[async_trait::async_trait]
impl AccountTokens for Counting {
    async fn access_token(&self) -> cua_host::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.token.clone())
    }
}

#[tokio::test]
async fn lists_owned_and_shared_relay_machines_as_spaces() {
    let relay = FakeRelay::start().await;
    relay.add_account("owner-token", "user-1", Some("ada@example.com"));
    relay.add_account("friend-token", "user-2", Some("friend@example.com"));
    // Register a machine the way `cua host setup` does.
    let reg = cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "owner-token",
            &cua_host::relay::RegisterRequest {
                id: MACHINE.into(),
                name: "studio-mac".into(),
                allow: vec!["friend@example.com".into()],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_online(&reg.machine.id, true, "0.1.0");

    for (token, role) in [("owner-token", "owner"), ("friend-token", "shared")] {
        let home = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder()
            .home(home.path())
            .relay(RelayAccount::new(
                &relay.url,
                Arc::new(StaticToken(token.into())),
            ))
            .build();
        assert!(spaces.list().unwrap().is_empty(), "nothing cached yet");
        let machines = spaces.relay_machines().await.unwrap();
        assert_eq!(machines.len(), 1);
        assert_eq!(machines[0].role, role);
        assert!(machines[0].online);
        let all = spaces.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, format!("relay:{MACHINE}"));
        assert_eq!(all[0].provider, Provider::Relay);
        assert_eq!(all[0].name, "studio-mac");
        assert_eq!(all[0].spacesd_version, "0.1.0");
        // Resolvable by display name.
        assert_eq!(
            spaces.resolve("studio-mac").unwrap(),
            SpaceId::Relay {
                machine_id: MACHINE.into()
            }
        );
    }

    // A stranger sees nothing; a bad token is an authentication error.
    relay.add_account("stranger-token", "user-3", None);
    let home = tempfile::tempdir().unwrap();
    let stranger = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("stranger-token".into())),
        ))
        .build();
    assert!(stranger.list_all().await.unwrap().is_empty());
    let bad = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("nope".into())),
        ))
        .build();
    let err = bad.relay_machines().await.unwrap_err();
    assert_eq!(err.tag(), "unauthenticated", "{err}");
    // list_all tolerates an unreachable directory.
    assert!(bad.list_all().await.unwrap().is_empty());
}

#[tokio::test]
async fn relay_spaces_need_an_account() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let err = spaces.relay_machines().await.unwrap_err();
    assert_eq!(err.tag(), "host_capability_missing", "{err}");
    let err = spaces.space(&format!("relay:{MACHINE}")).await.unwrap_err();
    assert_eq!(err.tag(), "host_capability_missing", "{err}");
}

#[tokio::test]
async fn connects_through_the_relay_path_with_the_account_token_per_call() {
    // The mock plays the relay + machine: it only answers under
    // /m/<machine-id> and only with the account token as bearer.
    let srv = MockServer::start(MockAuth {
        token: Some("account-token".into()),
        gateway: None,
        prefix: Some(format!("/m/{MACHINE}")),
    })
    .await;
    let tokens = Arc::new(Counting {
        token: "account-token".into(),
        calls: AtomicU32::new(0),
    });
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .probe_timeout(Duration::from_secs(10))
        .relay(RelayAccount::new(srv.url(), tokens.clone()))
        .build();
    let space = spaces.space(&format!("relay:{MACHINE}")).await.unwrap();
    assert_eq!(space.provider(), Provider::Relay);
    let before = tokens.calls.load(Ordering::SeqCst);
    assert!(before >= 1);
    let out = space
        .bash("echo relayed", Duration::from_secs(10))
        .await
        .unwrap();
    assert!(out.render().contains("relayed"), "{}", out.render());
    assert!(
        tokens.calls.load(Ordering::SeqCst) > before,
        "the account token is fetched per call, so refreshes take effect"
    );
    // Relay Spaces are not registry entries; releasing just disconnects.
    let released = spaces.delete(&format!("relay:{MACHINE}")).await.unwrap();
    assert!(
        released.contains("stays in your relay directory"),
        "{released}"
    );

    // A wrong account token is refused by the (emulated) relay.
    let home = tempfile::tempdir().unwrap();
    let wrong = Spaces::builder()
        .home(home.path())
        .probe_timeout(Duration::from_secs(5))
        .relay(RelayAccount::new(
            srv.url(),
            Arc::new(StaticToken("other".into())),
        ))
        .build();
    assert!(wrong.space(&format!("relay:{MACHINE}")).await.is_err());
}

/// The relay requires an enrolled client device: the account token alone
/// lists nothing, and with the device every directory call and every
/// connection to `relay:<id>` carries the device's session.
#[tokio::test]
async fn relay_spaces_carry_the_enrolled_device_session() {
    let relay = FakeRelay::start().await;
    relay.add_account("owner-token", "user-1", Some("ada@example.com"));
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "owner-token",
            &cua_host::relay::RegisterRequest {
                id: MACHINE.into(),
                name: "studio-mac".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_online(MACHINE, true, "0.1.0");
    relay.require_devices(true);
    let tokens: Arc<dyn AccountTokens> = Arc::new(StaticToken("owner-token".into()));

    let home = tempfile::tempdir().unwrap();
    let bare = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(&relay.url, tokens.clone()))
        .build();
    let err = bare.relay_machines().await.unwrap_err();
    assert_eq!(err.tag(), "permission_denied", "{err}");

    relay.fresh_sign_in("user-1");
    let device = Arc::new(
        cua_host::DeviceAuth::new(
            &relay.url,
            tokens.clone(),
            Arc::new(cua_host::MemoryKeySlot::default()),
            "laptop",
        )
        .unwrap(),
    );
    assert_eq!(
        device.enroll().await.unwrap().device.state,
        cua_host::DeviceState::Enrolled
    );
    let spaces = Spaces::builder()
        .home(home.path())
        .probe_timeout(Duration::from_secs(3))
        .relay(RelayAccount::new(&relay.url, tokens).with_device(device.clone()))
        .build();
    let machines = spaces.relay_machines().await.unwrap();
    assert_eq!(machines.len(), 1);
    // The fake relay does not tunnel; it records what reached /m/<id>.
    assert!(spaces.space(&format!("relay:{MACHINE}")).await.is_err());
    let session = device.session().await.unwrap();
    let proxied = relay.proxied();
    assert!(!proxied.is_empty());
    assert!(
        proxied
            .iter()
            .all(|(path, s)| path.starts_with(&format!("/m/{MACHINE}"))
                && s.as_deref() == Some(session.as_str())),
        "{proxied:?}"
    );
}
