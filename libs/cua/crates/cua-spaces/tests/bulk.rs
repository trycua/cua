//! Sharing facts on listed Spaces, leaving Spaces shared with you, and
//! deleting several at once (`cua_host::testing::FakeRelay`, temp homes).

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use std::sync::Arc;

const SHARED: &str = "space-0000bulk00001";
const MINE: &str = "space-0000bulk00002";

async fn relay_with_shares() -> FakeRelay {
    let relay = FakeRelay::start().await;
    relay.add_account("sam-token", "sam", Some("sam@example.com"));
    relay.add_account("you-token", "you", Some("you@example.com"));
    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    for (token, id, name, allow) in [
        ("sam-token", SHARED, "qa-shared", vec!["you@example.com"]),
        ("you-token", MINE, "ci-runner", vec!["priya@example.com"]),
    ] {
        client
            .register(
                token,
                &cua_host::relay::RegisterRequest {
                    id: id.into(),
                    name: name.into(),
                    allow: allow.into_iter().map(str::to_string).collect(),
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
    }
    client
        .patch(
            "you-token",
            MINE,
            &cua_host::relay::MachinePatch {
                viewers: Some(vec!["sam@example.com".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    relay
}

fn spaces_of(relay: &FakeRelay, home: &std::path::Path, token: &str) -> Spaces {
    Spaces::builder()
        .home(home)
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken(token.into())),
        ))
        .build()
}

#[tokio::test]
async fn listed_relay_spaces_carry_who_owns_them_and_who_has_them() {
    let relay = relay_with_shares().await;
    let home = tempfile::tempdir().unwrap();
    let spaces = spaces_of(&relay, home.path(), "you-token");
    let list = spaces.list_all().await.unwrap();
    let shared = list
        .iter()
        .find(|i| i.id == format!("relay:{SHARED}"))
        .unwrap();
    let s = shared.sharing.as_ref().expect("sharing facts");
    assert_eq!(s.role, "editor");
    assert_eq!(
        (s.owner_id.as_str(), s.owner_email.as_str()),
        ("sam", "sam@example.com")
    );
    // Someone it is shared with sees nobody else.
    assert!(s.editors.is_empty() && s.viewers.is_empty());
    let mine = list
        .iter()
        .find(|i| i.id == format!("relay:{MINE}"))
        .unwrap();
    let s = mine.sharing.as_ref().expect("sharing facts");
    assert_eq!(s.role, "owner");
    assert_eq!(s.editors, vec!["priya@example.com"]);
    assert_eq!(s.viewers, vec!["sam@example.com"]);
    // The JSON the apps read names them too.
    let json = serde_json::to_value(shared).unwrap();
    assert_eq!(json["sharing"]["role"], "editor");
    assert_eq!(json["sharing"]["owner_email"], "sam@example.com");
}

#[tokio::test]
async fn leaving_takes_you_off_the_shared_space_and_keeps_going_past_failures() {
    let relay = relay_with_shares().await;
    let home = tempfile::tempdir().unwrap();
    let spaces = spaces_of(&relay, home.path(), "you-token");
    spaces.list_all().await.unwrap();
    let ids = vec![
        format!("relay:{MINE}"),
        format!("relay:{SHARED}"),
        "local:nowhere".to_string(),
    ];
    let results = spaces.leave_many(&ids).await;
    assert_eq!(results.len(), 3);
    assert_eq!(
        results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>(),
        "one result per id, in order"
    );
    // Your own: refused, it is deleted instead.
    assert!(!results[0].ok);
    assert_eq!(
        results[0].error.as_deref(),
        Some("Couldn't leave ci-runner: it's yours; delete it instead.")
    );
    // Shared with you: gone from the relay's list and from this device.
    assert!(results[1].ok, "{:?}", results[1]);
    assert_eq!(results[1].message, "Left qa-shared.");
    assert!(relay.machine(SHARED).unwrap().allow.is_empty());
    assert!(
        !spaces
            .list()
            .unwrap()
            .iter()
            .any(|i| i.id == format!("relay:{SHARED}"))
    );
    assert!(
        !spaces
            .list_all()
            .await
            .unwrap()
            .iter()
            .any(|i| i.id == format!("relay:{SHARED}"))
    );
    // A local Space is yours too.
    assert!(!results[2].ok);
}

#[tokio::test]
async fn an_older_relay_says_it_needs_an_update() {
    let relay = relay_with_shares().await;
    relay.without_leave(true);
    let home = tempfile::tempdir().unwrap();
    let spaces = spaces_of(&relay, home.path(), "you-token");
    spaces.list_all().await.unwrap();
    let results = spaces.leave_many(&[format!("relay:{SHARED}")]).await;
    assert_eq!(
        results[0].error.as_deref(),
        Some("Couldn't leave qa-shared: the Cua relay needs an update.")
    );
    // Still shared, still listed.
    assert_eq!(
        relay.machine(SHARED).unwrap().allow,
        vec!["you@example.com".to_string()]
    );
}

#[tokio::test]
async fn deleting_several_answers_once_per_space_in_order_past_failures() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    for id in ["direct:127.0.0.1:9", "direct:127.0.0.1:10"] {
        spaces
            .registry()
            .upsert(
                cua_proto::daemon::v1::Space {
                    id: id.into(),
                    name: id.into(),
                    ..Default::default()
                },
                Default::default(),
            )
            .unwrap();
    }
    let ids = vec![
        "direct:127.0.0.1:9".to_string(),
        "no-such-space".to_string(),
        "direct:127.0.0.1:10".to_string(),
    ];
    let results = spaces.delete_many(&ids).await;
    assert_eq!(
        results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(results[0].ok, "{:?}", results[0]);
    assert!(!results[1].ok);
    assert!(results[1].error.as_deref().is_some_and(|e| !e.is_empty()));
    assert!(results[2].ok, "{:?}", results[2]);
    assert!(spaces.list().unwrap().is_empty());
}
