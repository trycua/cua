//! Sharing a real Space with a second account, end to end. Opt-in: run
//! through `tests/e2e/run-share-space-e2e.sh`, which starts a fake OIDC
//! issuer, the real `cua-relay` in account mode and a Linux Space (the
//! freshly built cua-spacesd over the image's), all in one network
//! namespace so the host and the Space name the relay by the same URL.
//! Without `CUA_SHARE_E2E_RELAY` every test prints why and passes.
//!
//! Env: `CUA_SHARE_E2E_RELAY` (relay URL), `CUA_SHARE_E2E_ISSUER` (the
//! fake issuer: `GET /mint?sub=&email=` mints account tokens),
//! `CUA_SHARE_E2E_SPACE` / `CUA_SHARE_E2E_TOKEN` (the Space's spacesd and
//! its env token), `CUA_SHARE_E2E_EVIDENCE` (directory for the transcript).
//!
//! The owner (ada) publishes the Space on the relay (`relay_register`):
//! her second device reaches it only through the relay; bob (another
//! account) cannot until it is shared. Shared as a viewer, bob joins
//! presence with his own cursor and every other call is refused by the
//! Space's driver; as an editor his input lands, as a human; unshared, he
//! is refused at once. The owner audit and the Space's access log show
//! each step.

use cua_spaces::presence::{Cursor, Identity, PresenceEvent};
use cua_spaces::relay::StaticToken;
use cua_spaces::share::{ShareConsent, ShareRole};
use cua_spaces::{RelayAccount, Spaces};
use serde_json::{Map, Value, json};
use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

struct Yes;
impl ShareConsent for Yes {
    fn confirm(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct Log(std::fs::File, Instant);
impl Log {
    fn line(&mut self, s: &str) {
        let t = self.1.elapsed().as_millis();
        println!("[{t:>6} ms] {s}");
        let _ = writeln!(self.0, "[{t:>6} ms] {s}");
    }
}

async fn token(issuer: &str, sub: &str) -> String {
    let v: Value = reqwest::get(format!(
        "{issuer}/mint?sub={sub}&email={sub}@example.com&ttl=3600"
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    v["access_token"].as_str().unwrap().to_string()
}

fn account(relay: &str, token: String, home: &std::path::Path) -> Spaces {
    Spaces::builder()
        .home(home)
        .download_dir(home.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .relay(RelayAccount::new(relay, Arc::new(StaticToken(token))))
        .share_consent(Arc::new(Yes))
        .build()
}

async fn bash_err(s: &cua_spaces::Space, cmd: &str) -> String {
    match s.bash(cmd, Duration::from_secs(30)).await {
        Ok(out) => panic!("{cmd} ran for a viewer: {}", out.render()),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn e2e_share_a_space_with_a_second_account() {
    let Some(relay) = env("CUA_SHARE_E2E_RELAY") else {
        eprintln!("skipped: set CUA_SHARE_E2E_RELAY (run tests/e2e/run-share-space-e2e.sh)");
        return;
    };
    let issuer = env("CUA_SHARE_E2E_ISSUER").expect("CUA_SHARE_E2E_ISSUER");
    let space_url = env("CUA_SHARE_E2E_SPACE").expect("CUA_SHARE_E2E_SPACE");
    let space_token = env("CUA_SHARE_E2E_TOKEN").expect("CUA_SHARE_E2E_TOKEN");
    let evidence = std::path::PathBuf::from(
        env("CUA_SHARE_E2E_EVIDENCE").unwrap_or_else(|| std::env::temp_dir().display().to_string()),
    );
    std::fs::create_dir_all(&evidence).unwrap();
    let mut log = Log(
        std::fs::File::create(evidence.join("share-space-e2e.log")).unwrap(),
        Instant::now(),
    );
    let t = Duration::from_secs(20);
    let ada_home = tempfile::tempdir().unwrap();
    let phone_home = tempfile::tempdir().unwrap();
    let bob_home = tempfile::tempdir().unwrap();
    let ada = account(&relay, token(&issuer, "ada").await, ada_home.path());
    let phone = account(&relay, token(&issuer, "ada").await, phone_home.path());
    let bob = account(&relay, token(&issuer, "bob").await, bob_home.path());

    // The owner's Space, reached directly (its own token) from this machine.
    let info = ada
        .add(&space_url, Some(space_token), Some("studio".into()))
        .await
        .unwrap();
    let owner_space = ada.space(&info.id).await.unwrap();
    log.line(&format!("owner added {} ({})", info.id, space_url));

    // 1. Publish it on the relay for the account's other devices.
    // #region docs:rs-relay-register
    let reg = ada.relay_register(&info.id).await.unwrap();
    // #endregion docs:rs-relay-register
    assert!(reg.online, "the Space's driver joined the relay: {reg:?}");
    let relay_id = reg.relay_space.clone();
    log.line(&format!(
        "relay_register -> {relay_id} online={}",
        reg.online
    ));

    // Her "phone" (another device of the same account, off this network)
    // reaches it only through the relay.
    let listed = phone.relay_machines().await.unwrap();
    assert!(
        listed
            .iter()
            .any(|m| format!("relay:{}", m.id) == relay_id && m.role == "owner")
    );
    let from_phone = phone.space(&relay_id).await.unwrap();
    let out = from_phone
        .bash("echo reached-through-the-relay", t)
        .await
        .unwrap();
    assert!(out.success() && out.stdout.contains("reached-through-the-relay"));
    log.line("owner's second device ran a command through the relay");

    // Bob is another account: nothing until it is shared.
    assert!(bob.relay_machines().await.unwrap().is_empty());
    let e = match bob.space(&relay_id).await {
        Ok(s) => s.bash("true", t).await.map(|_| ()).unwrap_err(),
        Err(e) => e,
    };
    log.line(&format!("bob before any share: refused ({})", e.tag()));

    // 2. Viewer: presence with his own cursor, everything else refused.
    // #region docs:rs-share-space
    let shares = ada
        .share_space(&info.id, "bob@example.com", ShareRole::Viewer)
        .await
        .unwrap();
    // #endregion docs:rs-share-space
    assert_eq!(shares.invitee_space, relay_id);
    assert_eq!(shares.shares[0].role, ShareRole::Viewer);
    log.line("shared with bob@example.com as viewer");
    let seen = bob.relay_machines().await.unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].role, "viewer");
    let bob_space = bob.space(&relay_id).await.unwrap();
    let mut ada_presence = owner_space
        .join_presence(
            Identity {
                id: "ada".into(),
                display_name: "Ada".into(),
                ..Default::default()
            },
            t,
        )
        .await
        .unwrap();
    let bob_presence = bob_space
        .join_presence(
            Identity {
                id: "bob".into(),
                display_name: "Bob".into(),
                ..Default::default()
            },
            t,
        )
        .await
        .unwrap();
    let bob_pid = bob_presence.me().participant_id.clone();
    let joined = ada_presence
        .wait_for(t, 100, |e| matches!(e, PresenceEvent::Joined { participant } if participant.participant_id == bob_pid))
        .await
        .unwrap();
    if let PresenceEvent::Joined { participant } = &joined {
        assert!(
            participant.display_name.to_lowercase().contains("bob"),
            "{participant:?}"
        );
        assert_eq!(participant.kind, "human");
        log.line(&format!(
            "viewer joined presence as {:?} ({}), kind {}",
            participant.display_name, participant.principal_id, participant.kind
        ));
    }
    bob_presence
        .update_cursor(&Cursor::at(0.4, 0.6))
        .await
        .unwrap();
    ada_presence
        .wait_for(t, 100, |e| matches!(e, PresenceEvent::CursorMoved { participant_id, .. } if *participant_id == bob_pid))
        .await
        .unwrap();
    log.line("the owner saw the viewer's own cursor move");
    let refused = bash_err(&bob_space, "echo should-not-run").await;
    assert!(
        refused.contains("view-only share:")
            && refused.contains("cannot call /cua.env.v1.ProcessService/"),
        "{refused}"
    );
    log.line(&format!("viewer shell refused: {refused}"));
    let refused_tool = bob_space
        .call_tool(None, "move_cursor", Map::new(), None)
        .await
        .map(|r| format!("{:?}", r.content))
        .unwrap_err()
        .to_string();
    assert!(refused_tool.contains("view-only share:"), "{refused_tool}");
    log.line(&format!("viewer input refused: {refused_tool}"));
    bob_presence.leave().await.ok();

    // 3. Editor: his input lands, as a human.
    ada.share_space(&info.id, "bob@example.com", ShareRole::Editor)
        .await
        .unwrap();
    log.line("changed bob@example.com to editor");
    let bob_space = {
        bob.forget_connection(&relay_id).await.ok();
        bob.space(&relay_id).await.unwrap()
    };
    let out = bob_space.bash("echo editor-ran", t).await.unwrap();
    assert!(out.stdout.contains("editor-ran"));
    let args = |v: Value| v.as_object().cloned().unwrap();
    let moved = bob_space
        .call_tool(
            None,
            "move_cursor",
            args(
                json!({"x": 211, "y": 157, "target": {"kind": "desktop", "display_id": "primary"}}),
            ),
            Some(t),
        )
        .await
        .unwrap();
    log.line(&format!(
        "editor move_cursor: is_error={} {}",
        moved.is_error,
        serde_json::to_string(&moved.content).unwrap_or_default()
    ));
    assert!(!moved.is_error, "{:?}", moved.content);
    let clicked = bob_space
        .call_tool(
            None,
            "click",
            args(
                json!({"x": 223, "y": 171, "target": {"kind": "desktop", "display_id": "primary"},
                        "delivery_mode": "foreground"}),
            ),
            Some(t),
        )
        .await
        .unwrap();
    log.line(&format!(
        "editor click: is_error={} {}",
        clicked.is_error,
        serde_json::to_string(&clicked.content).unwrap_or_default()
    ));
    assert!(!clicked.is_error, "{:?}", clicked.content);
    let pointer = owner_space
        .bash("DISPLAY=:1 xdotool getmouselocation", t)
        .await
        .unwrap();
    log.line(&format!(
        "pointer after the editor's click: {}",
        pointer.stdout.trim()
    ));
    assert!(pointer.stdout.contains("x:223 y:171"), "{}", pointer.stdout);

    // 4. Unshared: refused on the next call.
    let t0 = Instant::now();
    ada.unshare_space(&info.id, Some("bob@example.com"))
        .await
        .unwrap();
    let e = bob_space.bash("echo after-unshare", t).await.unwrap_err();
    log.line(&format!(
        "unshared; bob refused {} ms later: {e}",
        t0.elapsed().as_millis()
    ));
    assert!(bob.relay_machines().await.unwrap().is_empty());

    // 5. The audit trail: owner side, and the Space's own access log.
    let audit = ada.share_audit(Some(&info.id), 20).unwrap();
    let actions: Vec<&str> = audit.iter().rev().map(|a| a.action.as_str()).collect();
    log.line(&format!("owner audit: {actions:?}"));
    assert_eq!(actions, ["attach", "share", "share", "unshare"]);
    let access = owner_space
        .bash(
            "f=$(find / -xdev -name access.log -path '*spacesd*' 2>/dev/null | head -1); cat \"$f\"",
            t,
        )
        .await
        .unwrap();
    std::fs::write(evidence.join("access.log"), &access.stdout).unwrap();
    assert!(
        access
            .stdout
            .contains("refused ProcessService (view-only share)"),
        "{}",
        access.stdout
    );
    assert!(access.stdout.contains("bob"), "{}", access.stdout);
    log.line("the Space's access log records the viewer's refusals and the editor's calls");

    // 6. Off the relay: the second device loses it.
    assert!(ada.relay_unregister(&info.id).await.unwrap());
    phone.forget_connection(&relay_id).await.ok();
    let gone = match phone.space(&relay_id).await {
        Ok(s) => s.bash("true", t).await.is_err(),
        Err(_) => true,
    };
    assert!(gone, "unregistered Space still reachable through the relay");
    log.line("relay_unregister: the relay no longer reaches the Space");
}
