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
async fn space_shares_distinguishes_missing_from_unshared_spaces() {
    use cua_spaces::mcp::McpServer;
    use serde_json::json;

    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let server = McpServer::new(spaces.clone());

    let missing = server
        .call("space_shares", json!({"space": "local:nonexistent"}))
        .await;
    assert!(missing.is_error);
    assert_eq!(missing.structured.unwrap()["error"]["kind"], "not_found");
    let missing = server
        .call("unshare_space", json!({"space": "local:nonexistent"}))
        .await;
    assert!(missing.is_error);
    assert_eq!(missing.structured.unwrap()["error"]["kind"], "not_found");

    let unknown_name = server
        .call("space_shares", json!({"space": "nonexistent"}))
        .await;
    assert!(unknown_name.is_error);
    assert_eq!(
        unknown_name.structured.unwrap()["error"]["kind"],
        "not_found"
    );

    let env = MockServer::start(MockAuth::default()).await;
    let info = spaces
        .add(&env.url(), None, Some("studio".into()))
        .await
        .unwrap();
    let unshared = server.call("space_shares", json!({"space": info.id})).await;
    assert!(!unshared.is_error, "{:?}", unshared.first_text());
    let shares: serde_json::Value = serde_json::from_str(unshared.first_text().unwrap()).unwrap();
    assert_eq!(shares["shares"], json!([]));
    let unshared = server
        .call("unshare_space", json!({"space": info.id}))
        .await;
    assert!(!unshared.is_error, "{:?}", unshared.first_text());
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
    // The mock driver's version predates the feature: the error names the
    // release to update to.
    assert!(e.to_string().contains("cua-spacesd 0.2.2 or newer"), "{e}");
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
    // A machine the relay does not list is not on the relay.
    assert!(
        !spaces
            .relay_unregister(&format!("relay:{}", "0123abcd4567ef89"))
            .await
            .unwrap()
    );
}

/// The hermetic world of the tests below: a fake relay with two accounts
/// (`ada` owns, `bob` is someone she shares with) and Spaces for each.
async fn two_accounts() -> (FakeRelay, Spaces, Spaces, Vec<tempfile::TempDir>) {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("bob-token", "bob", Some("bob@example.com"));
    let homes = vec![tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let spaces = |home: &std::path::Path, token: &str| {
        Spaces::builder()
            .home(home)
            .relay(RelayAccount::new(
                &relay.url,
                Arc::new(StaticToken(token.into())),
            ))
            .build()
    };
    let ada = spaces(homes[0].path(), "ada-token");
    let bob = spaces(homes[1].path(), "bob-token");
    (relay, ada, bob, homes)
}

/// Registers `id` as one of ada's machines, as a Space's relay_register
/// (no meta) or a host's setup (`host_meta`) does.
async fn register(relay: &FakeRelay, id: &str, host_meta: bool) {
    let meta = if host_meta {
        [(cua_host::META_PROVIDES_SPACES.to_string(), "on".to_string())].into()
    } else {
        Default::default()
    };
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: id.into(),
                name: "vps-desktop".into(),
                allow: vec![],
                host: None,
                meta,
            },
        )
        .await
        .unwrap();
}

/// Deleting a Space that registered itself on the relay takes its machine
/// out of the relay directory (#4486): deleting the Space by its own id
/// detaches its driver and removes the machine, and deleting its
/// `relay:<machine>` after the Space is gone removes the stale record.
#[tokio::test]
async fn deleting_a_relay_registered_space_removes_its_machine() {
    let (relay, ada, _bob, _homes) = two_accounts().await;
    let env = MockServer::start(MockAuth::default()).await;
    env.state.advertise(&["relay_attach"]);
    let info = ada.add(&env.url(), None, Some("bot".into())).await.unwrap();
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
    let r = ada.relay_register(&info.id).await.unwrap();
    assert!(relay.machine(&r.machine).is_some());
    // Deleting the Space itself: its driver leaves and its machine goes.
    let message = ada.delete(&info.id).await.unwrap();
    assert!(!message.contains("Note"), "{message}");
    assert!(env.state.relay_attached.lock().unwrap().is_none());
    assert!(relay.machine(&r.machine).is_none(), "machine removed");

    // The issue's case: the Space is gone (its sandbox deleted elsewhere),
    // only its record is left, listed as offline, not ready.
    let stale = "space-00000000000000aa";
    register(&relay, stale, false).await;
    let id = format!("relay:{stale}");
    let row = ada
        .list_all()
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .expect("listed");
    assert_eq!(row.status, "offline");
    let listed = cua_spaces::mcp::McpServer::new(ada.clone())
        .call("list_spaces", serde_json::json!({}))
        .await;
    let rows: serde_json::Value = serde_json::from_str(listed.first_text().expect("rows")).unwrap();
    let phase = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .map(|r| r["phase"].clone());
    assert_eq!(phase, Some(serde_json::json!("offline")), "{rows}");
    let message = ada.delete(&id).await.unwrap();
    assert!(
        message.contains("from your relay directory")
            && !message.contains("stays in your relay directory"),
        "{message}"
    );
    assert!(relay.machine(stale).is_none(), "stale record removed");
    assert!(!ada.list_all().await.unwrap().iter().any(|i| i.id == id));
}

/// Removing a stale record: `rm` of a gone Space's `relay:<machine>` and
/// `relay_unregister` of a machine that is no longer connected remove it
/// from the relay directory; a host that is connected now is refused with
/// the remedy that works (`cua host remove` on it), and a live Space
/// registration is untouched by `rm`.
#[tokio::test]
async fn stale_relay_records_can_be_removed() {
    let (relay, ada, _bob, _homes) = two_accounts().await;
    let gone = "space-00000000000000bb";
    register(&relay, gone, false).await;
    ada.relay_machines().await.unwrap();
    ada.remove(&format!("relay:{gone}")).await.unwrap();
    assert!(relay.machine(gone).is_none(), "rm removed the stale record");

    // A live Space registration: `rm` only drops it here.
    let live = "space-00000000000000cc";
    register(&relay, live, false).await;
    relay.set_online(live, true, "0.1.0");
    ada.remove(&format!("relay:{live}")).await.unwrap();
    assert!(
        relay.machine(live).is_some(),
        "a live Space keeps its record"
    );
    assert!(
        ada.relay_unregister(&format!("relay:{live}"))
            .await
            .unwrap()
    );
    assert!(relay.machine(live).is_none());

    // A host: connected, it is refused with a remedy that can be carried
    // out; once that machine is gone, its record is removed.
    let host = "0123abcd4567ef890123abcd4567ef89";
    register(&relay, host, true).await;
    relay.set_online(host, true, "0.1.0");
    let e = ada
        .relay_unregister(&format!("relay:{host}"))
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument", "{e}");
    assert!(e.to_string().contains("cua host remove"), "{e}");
    assert!(relay.machine(host).is_some());
    // `rm` and `delete` never remove a host's record.
    ada.remove(&format!("relay:{host}")).await.unwrap();
    relay.set_online(host, false, "");
    ada.remove(&format!("relay:{host}")).await.unwrap();
    let message = ada.delete(&format!("relay:{host}")).await.unwrap();
    assert!(message.contains("relay-unregister"), "{message}");
    assert!(relay.machine(host).is_some());
    assert!(
        ada.relay_unregister(&format!("relay:{host}"))
            .await
            .unwrap()
    );
    assert!(
        relay.machine(host).is_none(),
        "the gone host's record removed"
    );
    let audit = ada.share_audit(None, 10).unwrap();
    assert!(audit.iter().any(|a| a.action == "forget"), "{audit:?}");
}

/// Only a machine's owner can remove its record: someone it is shared
/// with (editor or viewer) is refused by every path, here and by the
/// relay itself, and the record stays.
#[tokio::test]
async fn only_the_owner_removes_a_relay_record() {
    let (relay, ada, bob, _homes) = two_accounts().await;
    let gone = "space-00000000000000dd";
    register(&relay, gone, false).await;
    let ada_client = cua_host::RelayClient::new(&relay.url).unwrap();
    for patch in [
        cua_host::MachinePatch {
            allow: Some(vec!["bob@example.com".into()]),
            ..Default::default()
        },
        cua_host::MachinePatch {
            allow: Some(vec![]),
            viewers: Some(vec!["bob@example.com".into()]),
            ..Default::default()
        },
    ] {
        ada_client.patch("ada-token", gone, &patch).await.unwrap();
        let id = format!("relay:{gone}");
        bob.relay_machines().await.unwrap();
        let e = bob.relay_unregister(&id).await.unwrap_err();
        assert_eq!(e.tag(), "permission_denied", "{e}");
        assert!(e.to_string().contains("only its owner"), "{e}");
        let message = bob.delete(&id).await.unwrap();
        assert!(message.contains("only its"), "{message}");
        bob.relay_machines().await.unwrap();
        bob.remove(&id).await.unwrap();
        // The relay refuses anyone but the owner, whatever the client does.
        let e = cua_host::RelayClient::new(&relay.url)
            .unwrap()
            .delete("bob-token", gone)
            .await
            .unwrap_err();
        assert!(matches!(e, cua_host::Error::PermissionDenied(_)), "{e:?}");
        assert!(relay.machine(gone).is_some(), "the record stays");
    }
    // The owner can.
    ada.relay_machines().await.unwrap();
    assert!(
        ada.relay_unregister(&format!("relay:{gone}"))
            .await
            .unwrap()
    );
    assert!(relay.machine(gone).is_none());
}
