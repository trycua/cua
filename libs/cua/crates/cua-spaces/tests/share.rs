//! Sharing a Space (owner side), hermetic: the fake relay directory
//! (`cua_host::testing::FakeRelay`) and a mock spacesd. The first share of
//! a Space that is not a host registers a machine for it and attaches the
//! Space's driver (`SystemService.AttachRelay`); roles move between the
//! relay's two lists; removing everyone detaches it; every step is in the
//! hash-chained owner audit. The driver-side enforcement of roles is tested
//! in cua-spacesd-server (`tests/share_space.rs`).

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::share::{ShareConsent, ShareRole};
use cua_spaces::{RelayAccount, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::sync::{Arc, Mutex};

/// Answers every confirmation with `answer`, recording the reasons.
#[derive(Default)]
struct Consent {
    deny: bool,
    asked: Mutex<Vec<String>>,
}

impl ShareConsent for Consent {
    fn confirm(&self, reason: &str) -> Result<(), String> {
        self.asked.lock().unwrap().push(reason.to_string());
        if self.deny {
            Err("the user declined".into())
        } else {
            Ok(())
        }
    }
}

fn yes() -> Arc<Consent> {
    Arc::new(Consent::default())
}

#[tokio::test]
async fn sharing_a_space_attaches_it_and_moves_people_between_roles() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let env = MockServer::start(MockAuth::default()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ))
        .share_consent(yes())
        .build();
    let info = spaces
        .add(&env.url(), None, Some("studio".into()))
        .await
        .unwrap();

    // A driver without relay_attach cannot be shared, and says why.
    let e = spaces
        .share_space(&info.id, "bob@example.com", ShareRole::Viewer)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "capability_missing", "{e}");
    assert!(env.state.relay_attached.lock().unwrap().is_none());

    env.state.advertise(&["relay_attach"]);
    spaces.forget_connection(&info.id).await.unwrap();
    // The machine comes online once the driver dialed out; the fake relay
    // does not tunnel, so flip it when the attach arrives.
    let state = env.state.clone();
    let r = relay.url.clone();
    let online = tokio::spawn({
        let relay_handle = relay.clone();
        async move {
            for _ in 0..400 {
                if let Some(a) = state.relay_attached.lock().unwrap().clone() {
                    relay_handle.set_online(&a.machine_id, true, "0.1.0");
                    return a;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("never attached ({r})");
        }
    });
    let shares = spaces
        .share_space(&info.id, " Bob@Example.com", ShareRole::Viewer)
        .await
        .unwrap();
    let attached = online.await.unwrap();
    assert_eq!(attached.owner, "ada");
    assert_eq!(attached.owner_email, "ada@example.com");
    assert_eq!(attached.relay_url, relay.url);
    assert!(attached.machine_token.starts_with("cmt_"));
    assert_eq!(shares.machine, attached.machine_id);
    assert_eq!(
        shares.invitee_space,
        format!("relay:{}", attached.machine_id)
    );
    assert!(shares.online);
    assert_eq!(shares.shares.len(), 1);
    assert_eq!(shares.shares[0].who, "bob@example.com");
    assert_eq!(shares.shares[0].role, ShareRole::Viewer);
    let m = relay.machine(&attached.machine_id).unwrap();
    assert_eq!(m.viewers, ["bob@example.com"]);
    assert!(m.allow.is_empty());

    // Upgrading moves Bob to the allowlist (never on both lists).
    let shares = spaces
        .share_space(&info.id, "bob@example.com", ShareRole::Editor)
        .await
        .unwrap();
    assert_eq!(shares.shares[0].role, ShareRole::Editor);
    let m = relay.machine(&attached.machine_id).unwrap();
    assert_eq!(m.allow, ["bob@example.com"]);
    assert!(m.viewers.is_empty());
    // A second share reuses the attachment (no new machine).
    spaces
        .share_space(&info.id, "carol@example.com", ShareRole::Viewer)
        .await
        .unwrap();
    assert_eq!(
        env.state
            .relay_attached
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .machine_id,
        attached.machine_id
    );
    let listed = spaces.space_shares(&info.id).await.unwrap();
    assert_eq!(listed.shares.len(), 2);

    // Removing one person; then everyone (the Space leaves the relay).
    let shares = spaces
        .unshare_space(&info.id, Some("bob@example.com"))
        .await
        .unwrap();
    assert_eq!(shares.shares.len(), 1);
    assert!(
        relay
            .machine(&attached.machine_id)
            .unwrap()
            .allow
            .is_empty()
    );
    let gone = spaces.unshare_space(&info.id, None).await.unwrap();
    assert!(gone.machine.is_empty() && gone.shares.is_empty());
    assert!(
        env.state.relay_attached.lock().unwrap().is_none(),
        "detached"
    );
    assert!(
        relay.machine(&attached.machine_id).is_none(),
        "machine removed"
    );

    // Teams are not relay principals.
    let e = spaces
        .share_space(&info.id, "team:design", ShareRole::Viewer)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument");

    // The owner audit has every step, newest first, and verifies.
    let audit = spaces.share_audit(Some(&info.id), 20).unwrap();
    let actions: Vec<&str> = audit.iter().rev().map(|a| a.action.as_str()).collect();
    assert_eq!(
        actions,
        [
            "attach", "share", "share", "share", "unshare", "unshare", "detach"
        ]
    );
    assert_eq!(
        audit.last().unwrap().detail,
        format!("machine={}", attached.machine_id)
    );
}

#[tokio::test]
async fn only_the_owner_can_share_a_host() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("bob-token", "bob", Some("bob@example.com"));
    let reg = cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio".into(),
                allow: vec!["bob@example.com".into()],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    let as_ = |token: &str| {
        let home = tempfile::tempdir().unwrap();
        let spaces = Spaces::builder()
            .home(home.path())
            .relay(RelayAccount::new(
                &relay.url,
                Arc::new(StaticToken(token.into())),
            ))
            .share_consent(yes())
            .share_consent(yes())
            .build();
        (spaces, home)
    };
    let id = format!("relay:{}", reg.machine.id);
    let (bob, _h) = as_("bob-token");
    bob.relay_machines().await.unwrap();
    let e = bob
        .share_space(&id, "carol@example.com", ShareRole::Viewer)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    let (ada, _h2) = as_("ada-token");
    ada.relay_machines().await.unwrap();
    let shares = ada
        .share_space(&id, "carol@example.com", ShareRole::Viewer)
        .await
        .unwrap();
    assert_eq!(shares.invitee_space, id);
    let roles: Vec<(String, ShareRole)> = shares
        .shares
        .iter()
        .map(|s| (s.who.clone(), s.role))
        .collect();
    assert_eq!(
        roles,
        [
            ("bob@example.com".to_string(), ShareRole::Editor),
            ("carol@example.com".to_string(), ShareRole::Viewer)
        ]
    );
}

/// The MCP tools reach the same calls, and a bad role is refused by name.
#[tokio::test]
async fn the_sharing_tools_share_list_and_unshare() {
    use cua_spaces::mcp::McpServer;
    use serde_json::json;
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let reg = cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_online(&reg.machine.id, true, "0.1.0");
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ))
        .share_consent(yes())
        .build();
    spaces.relay_machines().await.unwrap();
    let server = McpServer::new(spaces);
    let id = format!("relay:{}", reg.machine.id);
    let call = |name: &str, args: serde_json::Value| {
        let server = server.clone();
        let name = name.to_string();
        async move {
            let out = server.call(&name, args).await;
            let text = out.first_text().unwrap_or_default().to_string();
            (out.is_error, text)
        }
    };
    let (err, text) = call(
        "share_space",
        json!({"space": id, "who": "bob@example.com", "role": "owner"}),
    )
    .await;
    assert!(err && text.contains("use viewer or editor"), "{text}");
    let (err, text) = call(
        "share_space",
        json!({"space": id, "who": "bob@example.com"}),
    )
    .await;
    assert!(!err, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["shares"][0]["role"], "viewer", "viewer is the default");
    assert_eq!(v["invitee_space"], id);
    let (_, text) = call("space_shares", json!({"space": id, "audit": 5})).await;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["shares"].as_array().unwrap().len(), 1);
    assert_eq!(v["audit"][0]["action"], "share");
    let (err, text) = call(
        "unshare_space",
        json!({"space": id, "who": "bob@example.com"}),
    )
    .await;
    assert!(!err, "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["shares"].as_array().unwrap().is_empty());
}

/// Sharing hands a desktop to another account: without someone to confirm
/// it is refused, and a declined confirmation shares nothing.
#[tokio::test]
async fn sharing_needs_the_users_confirmation() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let reg = cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: "0123abcd4567ef89".into(),
                name: "studio".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ))
        .build();
    spaces.relay_machines().await.unwrap();
    let id = format!("relay:{}", reg.machine.id);
    let e = spaces
        .share_space(&id, "bob@example.com", ShareRole::Viewer)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "host_capability_missing", "{e}");
    let no = Arc::new(Consent {
        deny: true,
        ..Default::default()
    });
    spaces.set_share_consent(Some(no.clone()));
    let e = spaces
        .share_space(&id, "bob@example.com", ShareRole::Editor)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    assert_eq!(
        no.asked.lock().unwrap().as_slice(),
        [format!("Let bob@example.com use {id} (see and control it)")]
    );
    assert!(relay.machine(&reg.machine.id).unwrap().allow.is_empty());
    assert!(spaces.share_audit(None, 10).unwrap().is_empty());
}

/// Publishing a Space on the relay for the account's other devices: the
/// driver is attached, the machine lists as the owner's (and nobody
/// else's), registering again is a no-op, and unregistering detaches and
/// removes it. A host cannot be unregistered from here.
#[tokio::test]
async fn relay_register_publishes_a_space_for_the_accounts_devices() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("eve-token", "eve", Some("eve@example.com"));
    let env = MockServer::start(MockAuth::default()).await;
    env.state.advertise(&["relay_attach"]);
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ))
        .build();
    let info = spaces
        .add(&env.url(), None, Some("bot".into()))
        .await
        .unwrap();
    let state = env.state.clone();
    let handle = relay.clone();
    tokio::spawn(async move {
        for _ in 0..400 {
            if let Some(a) = state.relay_attached.lock().unwrap().clone() {
                handle.set_online(&a.machine_id, true, "0.1.0");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let r = spaces.relay_register(&info.id).await.unwrap();
    assert!(r.online);
    assert_eq!(r.relay_space, format!("relay:{}", r.machine));
    // The owner's other devices see it; another account does not.
    let ada = cua_host::RelayClient::new(&relay.url).unwrap();
    let mine = ada.machines("ada-token").await.unwrap();
    assert!(mine.iter().any(|m| m.id == r.machine && m.role == "owner"));
    assert!(ada.machines("eve-token").await.unwrap().is_empty());
    // Idempotent: the same machine, no second attach.
    let again = spaces.relay_register(&info.id).await.unwrap();
    assert_eq!(again.machine, r.machine);
    assert!(spaces.relay_unregister(&info.id).await.unwrap());
    assert!(env.state.relay_attached.lock().unwrap().is_none());
    assert!(relay.machine(&r.machine).is_none());
    assert!(!spaces.relay_unregister(&info.id).await.unwrap());
    let e = spaces
        .relay_unregister(&format!("relay:{}", "0123abcd4567ef89"))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("cua host remove"), "{e}");
}
