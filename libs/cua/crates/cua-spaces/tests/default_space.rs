//! The account's default Space and removing a computer: settings saved on
//! the relay and read back, the default marked in lists from a short cache
//! (never holding a list up when signed out or the relay is down), and a
//! computer taken off the relay detaching its Spaces, or deleting them with
//! `delete_spaces`. Hermetic: the fake relay directory
//! (`cua_host::testing::FakeRelay`) and a throwaway cua home.

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use std::path::Path;
use std::sync::Arc;

const TOKEN: &str = "owner-token";
const COMPUTER: &str = "aaaa111122223333";
const CHILD: &str = "bbbb111122223333";

fn signed_in(home: &Path, relay: &FakeRelay) -> Spaces {
    Spaces::builder()
        .home(home)
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken(TOKEN.into())),
        ))
        .build()
}

async fn register(relay: &FakeRelay, id: &str, name: &str, host: Option<&str>) {
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            TOKEN,
            &cua_host::relay::RegisterRequest {
                id: id.into(),
                name: name.into(),
                allow: vec![],
                host: host.map(str::to_string),
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn the_default_is_saved_resolved_and_marked_without_holding_a_list_up() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    register(&relay, COMPUTER, "studio", None).await;
    relay.set_online(COMPUTER, true, "1.0.0");
    let home = tempfile::tempdir().unwrap();
    let app = signed_in(home.path(), &relay);

    // Nothing chosen: no default, settings are empty.
    assert_eq!(app.default_space_id().await, None);
    let s = app.spaces_settings().await.unwrap();
    assert!(s.default_space.is_none() && s.fallback.is_empty() && s.fallback_on);

    // Set it: the relay keeps it, resolution finds the online computer, and
    // the next list marks it at once (the save refreshed the cache).
    let want = format!("relay:{COMPUTER}");
    let saved = app
        .set_spaces_settings(&cua_host::SpacesSettings {
            default_space: Some(want.clone()),
            fallback: vec![],
            fallback_on: true,
            updated_at: 0,
        })
        .await
        .unwrap();
    assert_eq!(saved.default_space.as_deref(), Some(want.as_str()));
    assert!(saved.updated_at > 0);
    assert_eq!(app.default_space_id().await.as_deref(), Some(want.as_str()));
    let d = app.resolve_default_space(None).await.unwrap();
    assert_eq!(d.reason, "default");
    assert_eq!(d.space.as_deref(), Some(want.as_str()));
    assert_eq!(d.default_online, Some(true));

    // A computer the account cannot see is refused with the relay's reason.
    let e = app
        .set_spaces_settings(&cua_host::SpacesSettings {
            default_space: Some("relay:ffff000000000000".into()),
            fallback: vec![],
            fallback_on: true,
            updated_at: 0,
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("no such computer"), "{e}");

    // A relay that predates the routes, and signing out: no default, no error.
    relay.without_spaces_settings(true);
    let fresh = signed_in(home.path(), &relay);
    assert_eq!(fresh.default_space_id().await, None);
    let e = fresh.spaces_settings().await.unwrap_err();
    assert!(e.to_string().contains("needs an update"), "{e}");
    let signed_out = Spaces::builder().home(home.path()).build();
    assert_eq!(signed_out.default_space_id().await, None);
}

#[tokio::test]
async fn removing_a_computer_detaches_its_spaces_or_deletes_them() {
    for delete_spaces in [false, true] {
        let relay = FakeRelay::start().await;
        relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
        register(&relay, COMPUTER, "studio", None).await;
        register(&relay, CHILD, "research", Some(COMPUTER)).await;
        let home = tempfile::tempdir().unwrap();
        let app = signed_in(home.path(), &relay);
        let listed = |app: &Spaces| {
            let app = app.clone();
            async move {
                app.relay_machines()
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|m| m.id)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(listed(&app).await.len(), 2);

        // Both are offline in the fake: the computer goes, taking its
        // Spaces with it only when asked.
        assert!(
            app.remove_computer(&format!("relay:{COMPUTER}"), delete_spaces)
                .await
                .unwrap()
        );
        assert!(relay.machine(COMPUTER).is_none());
        assert!(
            listed(&app).await.is_empty(),
            "a detached Space is hidden (delete_spaces={delete_spaces})"
        );
        assert_eq!(
            relay.machine(CHILD).is_some(),
            !delete_spaces,
            "its record stays when detached and goes with delete_spaces"
        );

        if !delete_spaces {
            // The computer registers again under the same id: they are back.
            register(&relay, COMPUTER, "studio", None).await;
            assert_eq!(listed(&app).await.len(), 2);
        }
    }
}
