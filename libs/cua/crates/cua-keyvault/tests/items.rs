// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The per-app secret items: upsert by key, the unattended lock (one
//! presence for a batch, an agent answered without a prompt, never a value
//! back), batch delete that wipes live copies, the "never ask again"
//! setting, selective send and the old-vault reset. A fake backend and a
//! throwaway vault: nothing touches the machine.

mod common;

use std::time::Duration;

use common::{FakeBackend, Rig, all_items, import_passwords, rig};
use cua_keyvault::broker::{
    AccessRequest, Backend, Broker, BrokerConfig, Decision, FakePresence, ImportSpec, LoginRequest,
    Selector, SiteChoice, TeleportRequest,
};
use cua_keyvault::model::{ItemKind, ItemMeta};
use cua_keyvault::{Action, Error};
use std::sync::Arc;

fn spec(sites: &[&str], whole_app: bool) -> ImportSpec {
    ImportSpec {
        app: "chrome".into(),
        sites: sites
            .iter()
            .map(|s| SiteChoice {
                site: s.to_string(),
                include_storage: false,
                include_passwords: false,
            })
            .collect(),
        whole_app,
        ..Default::default()
    }
}

async fn import(r: &Rig, sites: &[&str], whole_app: bool) -> cua_keyvault::broker::ImportReport {
    r.broker
        .import(&r.cua, spec(sites, whole_app))
        .await
        .unwrap()
}

fn ids(items: &[ItemMeta]) -> Vec<String> {
    items.iter().map(|i| i.id.clone()).collect()
}

fn find<'a>(items: &'a [ItemMeta], domain: &str, key: &str) -> &'a ItemMeta {
    items
        .iter()
        .find(|i| i.domain.as_deref() == Some(domain) && i.key == key)
        .unwrap_or_else(|| panic!("no {domain} / {key}"))
}

fn asked(r: &Rig) -> usize {
    r.presence.asked.lock().unwrap().len()
}

/// An agent asks to write `site`'s cookies into `target`.
fn ask(site: &str, target: &str) -> AccessRequest {
    AccessRequest {
        selectors: vec![Selector::Site {
            app: "chrome".into(),
            site: site.into(),
        }],
        targets: vec![target.into()],
        ..Default::default()
    }
}

// ---------------------------------------------------------------- upsert

#[tokio::test]
async fn saving_the_same_app_twice_upserts_by_key_and_never_duplicates() {
    let r = rig().await;
    let first = import(&r, &["github.com", "slack.com"], true).await;
    assert_eq!((first.saved, first.created), (6, 6), "{first:?}");
    let before = all_items(&r).await;
    assert_eq!(before.len(), 6);
    assert_eq!(
        before.iter().filter(|i| i.kind == ItemKind::Cookie).count(),
        4
    );
    assert_eq!(
        before.iter().filter(|i| i.kind == ItemKind::File).count(),
        2
    );

    // The user unlocks one item between the two saves.
    let session = find(&before, ".github.com", "session").clone();
    r.broker
        .set_locked(&r.cua, vec![session.id.clone()], false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(3)).await;

    // Teleporting Chrome again with Save to Keyvault: same keys, same values.
    let again = import(&r, &["github.com", "slack.com"], true).await;
    assert_eq!(
        (again.created, again.updated, again.unchanged),
        (0, 0, 6),
        "{again:?}"
    );
    let after = all_items(&r).await;
    assert_eq!(after.len(), 6, "no duplicates");
    let kept = find(&after, ".github.com", "session");
    assert_eq!(kept.id, session.id, "the same item");
    assert!(!kept.locked(), "the unlock survived the second save");
    assert_eq!(kept.created_ms, session.created_ms);
    assert!(kept.updated_ms > session.updated_ms, "updated-at moved");
    assert_eq!(kept.rev, session.rev, "an unchanged value is not rewritten");

    // A changed value updates the item in place.
    *r.backend.generation.lock().unwrap() = 1;
    let changed = import(&r, &["github.com"], false).await;
    assert_eq!((changed.created, changed.updated), (0, 2), "{changed:?}");
    let after = all_items(&r).await;
    assert_eq!(after.len(), 6);
    assert_eq!(find(&after, ".github.com", "session").rev, session.rev + 1);
    // A site not in this save is left alone.
    assert_eq!(find(&after, ".slack.com", "session").rev, 1);
}

#[tokio::test]
async fn a_changed_file_updates_and_a_cookie_and_a_file_never_collide() {
    let r = rig().await;
    import(&r, &["github.com"], true).await;
    let a = all_items(&r).await;
    let files: Vec<_> = a.iter().filter(|i| i.kind == ItemKind::File).collect();
    assert_eq!(
        files.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(),
        ["Default/Bookmarks", "Local State"],
        "a file's key is its path"
    );
    assert!(files.iter().all(|f| f.domain.is_none()));
    let cookies: Vec<_> = a.iter().filter(|i| i.kind == ItemKind::Cookie).collect();
    assert!(
        cookies
            .iter()
            .all(|c| c.domain.as_deref() == Some(".github.com") && c.path.as_deref() == Some("/")),
        "a cookie's key is its host, name and path"
    );
}

// ------------------------------------------------------------------ lock

#[tokio::test]
async fn items_start_locked_and_unlocking_a_batch_asks_presence_once() {
    let r = rig().await;
    import(&r, &["github.com", "gitlab.com"], false).await;
    let items = all_items(&r).await;
    assert!(items.iter().all(|i| i.locked()), "locked by default");

    // A declined presence changes nothing.
    r.presence.set(false);
    let err = r
        .broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PresenceFailed(_)), "{err:?}");
    assert!(all_items(&r).await.iter().all(|i| i.locked()));

    // One prompt for the whole batch, naming what it allows.
    r.presence.set(true);
    let before = asked(&r);
    let out = r
        .broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap();
    assert_eq!(asked(&r) - before, 1, "one presence for the batch");
    assert_eq!(out.changed.len(), 4);
    assert!(out.skipped.is_empty());
    let prompt = r.presence.asked.lock().unwrap().last().cloned().unwrap();
    assert!(
        prompt.contains("4 Keyvault items") && prompt.contains("Cua Spaces MCP"),
        "{prompt}"
    );
    assert!(all_items(&r).await.iter().all(|i| !i.locked()));

    // Unlocking what is already unlocked asks nothing.
    let before = asked(&r);
    let out = r
        .broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap();
    assert!(out.changed.is_empty());
    assert_eq!(asked(&r), before);

    // Locking narrows access: no presence, one call for the batch.
    r.presence.set(false);
    let before = asked(&r);
    let out = r
        .broker
        .set_locked(&r.cua, ids(&items[..3]), true)
        .await
        .unwrap();
    assert_eq!(out.changed.len(), 3);
    assert_eq!(asked(&r), before, "the declined gate was not even asked");
    let now = all_items(&r).await;
    assert_eq!(now.iter().filter(|i| i.locked()).count(), 3);

    // Unknown ids change nothing; third parties cannot lock.
    assert!(matches!(
        r.broker
            .set_locked(&r.cua, vec![items[3].id.clone(), "nope".into()], true)
            .await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        r.broker.set_locked(&r.agent, ids(&items), false).await,
        Err(Error::Forbidden(_))
    ));
}

#[tokio::test]
async fn an_identity_provider_session_always_asks() {
    let r = rig().await;
    import(&r, &["google.com", "github.com"], false).await;
    let items = all_items(&r).await;
    let google = find(&items, ".google.com", "session");
    assert!(google.identity_provider, "Google is an identity provider");
    assert_eq!(
        google.policy.ttl_secs,
        cua_keyvault::model::IDP_TTL_SECS,
        "a short lifetime by default"
    );
    let out = r
        .broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap();
    assert_eq!(out.skipped.len(), 2, "both Google cookies are skipped");
    assert_eq!(out.changed.len(), 2);
    let after = all_items(&r).await;
    assert!(find(&after, ".google.com", "session").locked());
    assert!(!find(&after, ".github.com", "session").locked());
    // And the policy path cannot sneak it in.
    let mut p = google.policy.clone();
    p.unattended = true;
    assert!(matches!(
        r.broker.set_item_policy(&r.cua, &google.id, p).await,
        Err(Error::Forbidden(_))
    ));
}

#[tokio::test]
async fn the_never_ask_again_setting_is_stored_visible_and_revertible() {
    let r = rig().await;
    assert_eq!(
        r.broker.status(&r.cua).await.skip_unlock_prompt,
        Some(false),
        "the prompt is on by default"
    );
    let before = asked(&r);
    r.broker.set_skip_unlock_prompt(&r.cua, true).await.unwrap();
    assert_eq!(asked(&r), before, "a prompt preference needs no presence");
    assert_eq!(r.broker.status(&r.cua).await.skip_unlock_prompt, Some(true));
    // It survives a lock and an unlock (it is in the sealed settings).
    r.broker.lock(&r.cua).await.unwrap();
    r.broker
        .unlock(
            &r.cua,
            cua_keyvault::broker::UnlockRequest {
                passphrase: Some("correct horse battery".into()),
                recovery_key: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.broker.status(&r.cua).await.skip_unlock_prompt, Some(true));
    // It never skips the presence check that unlocking asks for.
    import(&r, &["github.com"], false).await;
    let items = all_items(&r).await;
    let before = asked(&r);
    r.broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap();
    assert_eq!(asked(&r), before + 1, "Touch ID is still asked");
    // Settings can turn it back on; a third party never sees it.
    r.broker
        .set_skip_unlock_prompt(&r.cua, false)
        .await
        .unwrap();
    assert_eq!(
        r.broker.status(&r.cua).await.skip_unlock_prompt,
        Some(false)
    );
    assert_eq!(r.broker.status(&r.agent).await.skip_unlock_prompt, None);
    assert!(matches!(
        r.broker.set_skip_unlock_prompt(&r.agent, true).await,
        Err(Error::Forbidden(_))
    ));
}

// ---------------------------------------------------- unattended access

#[tokio::test]
async fn unlocked_items_answer_an_agent_without_asking_and_no_value_comes_back() {
    let r = rig().await;
    import(&r, &["github.com", "gitlab.com"], false).await;
    let items = all_items(&r).await;
    let github: Vec<ItemMeta> = items
        .iter()
        .filter(|i| i.domain.as_deref() == Some(".github.com"))
        .cloned()
        .collect();
    r.broker
        .set_locked(&r.cua, ids(&github), false)
        .await
        .unwrap();

    // A locked site still waits for the user.
    let locked = r
        .broker
        .request_access(&r.agent, ask("gitlab.com", "dev-1"))
        .await
        .unwrap();
    assert_eq!(r.broker.list_pending(&r.cua).await.unwrap().len(), 1);
    assert!(matches!(
        r.broker
            .await_decision(&r.agent, &locked.id, Duration::from_millis(5))
            .await
            .unwrap(),
        Decision::Pending
    ));

    // The unlocked one is answered at once: no pending request, no prompt.
    let before = asked(&r);
    let view = r
        .broker
        .request_access(&r.agent, ask("github.com", "dev-1"))
        .await
        .unwrap();
    assert_eq!(asked(&r), before, "no presence for an unlocked item");
    assert_eq!(r.broker.list_pending(&r.cua).await.unwrap().len(), 1);
    let Decision::Granted {
        token, items: ids, ..
    } = r
        .broker
        .await_decision(&r.agent, &view.id, Duration::from_millis(5))
        .await
        .unwrap()
    else {
        panic!("granted without asking")
    };
    assert_eq!(ids.len(), 2);

    // The agent writes the items into the Space; nothing it receives holds a
    // value, a domain or a key.
    let outcome = r
        .broker
        .teleport(
            &r.agent,
            TeleportRequest {
                token: Some(token.clone()),
                items: ids,
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.backend.delivered.lock().unwrap().len(), 1);
    assert_eq!(
        r.backend.delivered_hosts.lock().unwrap()[0],
        vec![".github.com".to_string(), ".github.com".to_string()]
    );
    for seen in [
        serde_json::to_string(&view).unwrap(),
        serde_json::to_string(&outcome).unwrap(),
        token.clone(),
    ] {
        assert!(
            !seen.contains("sess-github") && !seen.contains("csrf-github"),
            "a value reached the agent: {seen}"
        );
    }
    assert!(
        view.items
            .iter()
            .all(|i| i.domain.is_none() && i.key.is_empty()),
        "names are not handed to the agent either"
    );
    // The grant is single use, and the audit says it was unattended.
    assert!(
        r.broker
            .teleport(
                &r.agent,
                TeleportRequest {
                    token: Some(token),
                    items: vec![github[0].id.clone()],
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                },
            )
            .await
            .is_err()
    );
    let audit = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    assert!(audit.iter().any(|e| e.event.kind == "consent.allow"
        && e.event.detail.contains("unattended (items unlocked)")));
}

#[tokio::test]
async fn a_request_that_mixes_locked_and_unlocked_items_still_asks() {
    let r = rig().await;
    import(&r, &["github.com"], false).await;
    let items = all_items(&r).await;
    // Only one of the two cookies is unlocked.
    r.broker
        .set_locked(&r.cua, vec![items[0].id.clone()], false)
        .await
        .unwrap();
    r.broker
        .request_access(&r.agent, ask("github.com", "dev-1"))
        .await
        .unwrap();
    assert_eq!(r.broker.list_pending(&r.cua).await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_unlocked_item_still_respects_its_allowed_targets_and_the_kill_switch() {
    let r = rig().await;
    import(&r, &["github.com"], false).await;
    let items = all_items(&r).await;
    r.broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap();
    // Restrict one item to another Space: a request for dev-1 asks.
    let mut p = items[0].policy.clone();
    p.unattended = true;
    p.allowed_targets = vec!["prod".into()];
    r.broker
        .set_item_policy(&r.cua, &items[0].id, p)
        .await
        .unwrap();
    r.broker
        .request_access(&r.agent, ask("github.com", "dev-1"))
        .await
        .unwrap();
    assert_eq!(
        r.broker.list_pending(&r.cua).await.unwrap().len(),
        1,
        "not auto-answered for a Space the item may not go to"
    );

    // The kill switch refuses unattended requests and further unlocking.
    r.broker.set_disabled(&r.cua, true).await.unwrap();
    assert!(matches!(
        r.broker
            .request_access(&r.agent, ask("github.com", "prod"))
            .await,
        Err(Error::Disabled)
    ));
    assert!(matches!(
        r.broker.set_locked(&r.cua, ids(&items), false).await,
        Err(Error::Disabled)
    ));
    // Locking is still allowed while it is on.
    assert!(r.broker.set_locked(&r.cua, ids(&items), true).await.is_ok());
}

#[tokio::test]
async fn locking_an_item_revokes_the_unattended_grants_it_backed() {
    let r = rig().await;
    import(&r, &["github.com"], false).await;
    let items = all_items(&r).await;
    r.broker
        .set_locked(&r.cua, ids(&items), false)
        .await
        .unwrap();
    let mut req = ask("github.com", "dev-1");
    req.uses = Some(0);
    let view = r.broker.request_access(&r.agent, req).await.unwrap();
    let Decision::Granted {
        token, items: held, ..
    } = r
        .broker
        .await_decision(&r.agent, &view.id, Duration::from_millis(5))
        .await
        .unwrap()
    else {
        panic!("granted")
    };
    r.broker
        .set_locked(&r.cua, vec![items[0].id.clone()], true)
        .await
        .unwrap();
    let err = r
        .broker
        .teleport(
            &r.agent,
            TeleportRequest {
                token: Some(token),
                items: held,
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Capability(_) | Error::Forbidden(_)),
        "{err:?}"
    );
    assert!(r.backend.delivered.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unlocked_password_signs_in_without_asking_and_never_echoes_it() {
    let r = rig().await;
    let pw = import_passwords(&r).await;
    let example = pw
        .iter()
        .find(|i| i.domain.as_deref() == Some("http://login.example.test:8000"))
        .unwrap();
    assert_eq!(example.key, "ada@example.test");
    r.broker
        .set_locked(&r.cua, vec![example.id.clone()], false)
        .await
        .unwrap();
    let view = r
        .broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Login {
                    site: "example.test".into(),
                }],
                targets: vec!["work".into()],
                actions: vec![Action::Login],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(r.broker.list_pending(&r.cua).await.unwrap().is_empty());
    let Decision::Granted { token, .. } = r
        .broker
        .await_decision(&r.agent, &view.id, Duration::from_millis(5))
        .await
        .unwrap()
    else {
        panic!("granted")
    };
    let out = r
        .broker
        .login(
            &r.agent,
            LoginRequest {
                token: Some(token),
                url: "http://login.example.test:8000/".into(),
                target: "work".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(r.backend.fills.lock().unwrap().len(), 1);
    let seen = serde_json::to_string(&out).unwrap();
    assert!(
        !seen.contains(common::PASSWORD) && !seen.contains("ada@example.test"),
        "{seen}"
    );
    // The other password (github) is still locked and still asks.
    r.broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Login {
                    site: "github.com".into(),
                }],
                targets: vec!["work".into()],
                actions: vec![Action::Login],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(r.broker.list_pending(&r.cua).await.unwrap().len(), 1);
}

// ----------------------------------------------------------------- delete

#[tokio::test]
async fn deleting_a_batch_wipes_their_live_copies_and_is_all_or_nothing() {
    let r = rig().await;
    import(&r, &["github.com", "gitlab.com"], true).await;
    let items = all_items(&r).await;
    let github: Vec<ItemMeta> = items
        .iter()
        .filter(|i| i.domain.as_deref() == Some(".github.com"))
        .cloned()
        .collect();
    let gitlab: Vec<ItemMeta> = items
        .iter()
        .filter(|i| i.domain.as_deref() == Some(".gitlab.com"))
        .cloned()
        .collect();
    for set in [&github, &gitlab] {
        r.broker
            .teleport(
                &r.cua,
                TeleportRequest {
                    token: None,
                    items: ids(set),
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                },
            )
            .await
            .unwrap();
    }

    // An unknown id deletes and wipes nothing. (The second teleport already
    // superseded, and so wiped, the first copy.)
    let wiped_before = r.backend.wiped.lock().unwrap().len();
    let mut bad = ids(&github);
    bad.push("deadbeef".into());
    assert!(matches!(
        r.broker.delete_items(&r.cua, bad).await,
        Err(Error::NotFound(_))
    ));
    assert_eq!(all_items(&r).await.len(), items.len());
    assert_eq!(r.backend.wiped.lock().unwrap().len(), wiped_before);

    // Deleting github's items wipes the copy that carried them, in the
    // Space, and removes them from the vault in one call.
    let wiped = r.broker.delete_items(&r.cua, ids(&github)).await.unwrap();
    assert!(!wiped.is_empty(), "the live copy was wiped");
    assert!(
        r.backend.wiped.lock().unwrap().len() > wiped_before,
        "the Space was told to wipe"
    );
    let left = all_items(&r).await;
    assert_eq!(left.len(), items.len() - github.len());
    assert!(
        left.iter()
            .all(|i| i.domain.as_deref() != Some(".github.com"))
    );
    let audit = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.event.kind == "item.delete" && e.event.detail.contains("count=2"))
    );
    // Third parties cannot delete.
    assert!(matches!(
        r.broker.delete_items(&r.agent, ids(&gitlab)).await,
        Err(Error::Forbidden(_))
    ));
}

// ------------------------------------------------------- selective send

#[tokio::test]
async fn sending_part_of_an_app_filters_records_and_nothing_else_leaves() {
    let r = rig().await;
    import(&r, &["github.com", "gitlab.com", "slack.com"], true).await;
    let items = all_items(&r).await;
    let chosen: Vec<ItemMeta> = items
        .iter()
        .filter(|i| {
            matches!(
                i.domain.as_deref(),
                Some(".github.com") | Some(".slack.com")
            ) || i.key == "Local State"
        })
        .cloned()
        .collect();
    r.broker
        .teleport(
            &r.cua,
            TeleportRequest {
                token: None,
                items: ids(&chosen),
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    let delivered = r.backend.delivered.lock().unwrap().clone();
    let mut paths = delivered[0].1.clone();
    paths.sort();
    assert_eq!(paths, ["Local State", "cookies.json"]);
    let mut hosts = r.backend.delivered_hosts.lock().unwrap()[0].clone();
    hosts.sort();
    assert_eq!(
        hosts,
        [".github.com", ".github.com", ".slack.com", ".slack.com"],
        "gitlab and the other file stayed home"
    );
}

#[tokio::test]
async fn saved_passwords_are_delivered_only_when_the_user_ticked_them_and_never_to_an_agent() {
    let r = rig().await;
    let pw = import_passwords(&r).await;
    assert!(!pw.is_empty());
    let target = |include: bool, items: Vec<String>| TeleportRequest {
        token: None,
        items,
        target: "dev-1".into(),
        include_passwords: include,
        launch: false,
    };
    // Without the explicit choice a password is never part of a delivery.
    let err = r
        .broker
        .teleport(&r.cua, target(false, ids(&pw)))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert!(r.backend.delivered.lock().unwrap().is_empty());
    // A third party cannot ask for them, token or not.
    let agent = r.broker.teleport(&r.agent, target(true, ids(&pw))).await;
    assert!(agent.is_err(), "an agent never gets a password delivered");
    assert!(r.backend.delivered.lock().unwrap().is_empty());
    // The user's own, explicit choice carries them in their reserved entry,
    // and the audit log says so.
    r.broker
        .teleport(&r.cua, target(true, ids(&pw)))
        .await
        .unwrap();
    let delivered = r.backend.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    assert!(
        delivered[0].1.iter().any(|p| p == "logins.json"),
        "{delivered:?}"
    );
    let log = r.broker.audit_tail(&r.cua, 50).await.unwrap();
    assert!(
        log.iter()
            .any(|e| e.event.detail.contains("INCLUDING SAVED PASSWORDS")),
        "{log:?}"
    );
}

// ------------------------------------------------------------- old vault

#[tokio::test]
async fn a_preview_vault_is_set_aside_with_a_notice_never_misread_or_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let kv = dir.path().join("keyvault");
    {
        let r = Broker::new(
            BrokerConfig {
                dir: kv.clone(),
                keychain_path: None,
                os_protector: false,
            },
            Arc::new(FakeBackend::default()),
            Arc::new(FakePresence::new(true)),
        )
        .unwrap();
        let cua = cua_keyvault::CallerIdentity::for_tests("com.trycua.cua", true);
        r.init(
            &cua,
            cua_keyvault::broker::InitRequest {
                passphrase: Some("correct horse battery".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    // Make it look like an earlier preview wrote it.
    let header_path = kv.join("vault.json");
    let mut header: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&header_path).unwrap()).unwrap();
    header["format"] = 1.into();
    std::fs::write(&header_path, serde_json::to_vec(&header).unwrap()).unwrap();

    let b = Broker::new(
        BrokerConfig {
            dir: kv.clone(),
            keychain_path: None,
            os_protector: false,
        },
        Arc::new(FakeBackend::default()),
        Arc::new(FakePresence::new(true)),
    )
    .unwrap();
    let cua = cua_keyvault::CallerIdentity::for_tests("com.trycua.cua", true);
    let status = b.status(&cua).await;
    assert!(!status.initialized, "a new vault is set up");
    let notice = status.reset_notice.expect("one line for the UI");
    assert!(notice.contains("set aside"), "{notice}");
    assert!(
        dir.path().join("keyvault.preview-v1/vault.json").is_file(),
        "the old vault is kept, not deleted"
    );
    assert!(!kv.exists());
    // A new vault can be created in its place.
    b.init(
        &cua,
        cua_keyvault::broker::InitRequest {
            passphrase: Some("correct horse battery".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(b.status(&cua).await.initialized);
}

// ------------------------------------------------------------- big files

#[tokio::test]
async fn a_big_file_is_stored_as_a_blob_and_delivered_whole() {
    use cua_keyvault::broker::Captured;
    struct Big;
    #[async_trait::async_trait]
    impl Backend for Big {
        fn inventory(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> cua_keyvault::Result<cua_keyvault::broker::Inventory> {
            Ok(Default::default())
        }
        fn capture(&self, _: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
            let bytes = vec![9u8; cua_keyvault::record::INLINE_FILE_LIMIT * 3];
            let n = cua_keyvault::record::file_record("Default/History", 0o600, &bytes)?;
            let (meta, payload) = n.into_item("chrome", "Chrome", "Default", "full");
            Ok(vec![Captured { meta, payload }])
        }
        async fn deliver(
            &self,
            _: &str,
            _: &str,
            _: &str,
            entries: Vec<cua_keyvault::model::PayloadEntry>,
            _: u64,
        ) -> cua_keyvault::Result<cua_keyvault::broker::DeliveryOutcome> {
            use base64::Engine as _;
            let e = &entries[0];
            let got = base64::engine::general_purpose::STANDARD
                .decode(&e.data)
                .unwrap();
            assert_eq!(got.len(), cua_keyvault::record::INLINE_FILE_LIMIT * 3);
            assert!(got.iter().all(|b| *b == 9));
            Ok(Default::default())
        }
        async fn wipe(&self, _: &str, _: &str) -> cua_keyvault::Result<Vec<String>> {
            Ok(vec![])
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let b = Broker::new(
        BrokerConfig {
            dir: dir.path().join("kv"),
            keychain_path: None,
            os_protector: false,
        },
        Arc::new(Big),
        Arc::new(FakePresence::new(true)),
    )
    .unwrap();
    let cua = cua_keyvault::CallerIdentity::for_tests("com.trycua.cua", true);
    b.init(
        &cua,
        cua_keyvault::broker::InitRequest {
            passphrase: Some("correct horse battery".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    b.import(&cua, spec(&[], true)).await.unwrap();
    b.browse(&cua).await.unwrap();
    let items = b.list_items(&cua, 0, 10).await.unwrap().items;
    assert!(items[0].blob.is_some(), "stored as a blob");
    assert!(dir.path().join("kv/blobs").is_dir());
    b.teleport(
        &cua,
        TeleportRequest {
            token: None,
            items: ids(&items),
            target: "dev-1".into(),
            include_passwords: false,
            launch: false,
        },
    )
    .await
    .unwrap();
    // Deleting the item shreds the blob.
    b.delete_items(&cua, ids(&items)).await.unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path().join("kv/blobs"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn site_icons_are_captured_locally_hidden_until_browse_and_pruned_with_the_site() {
    let r = rig().await;
    r.backend
        .icons
        .lock()
        .unwrap()
        .push(("example.com".into(), b"\x89PNG\r\n\x1a\nicon".to_vec()));
    import(&r, &["example.com"], false).await;
    // Names (and so the sites) stay hidden until the browse window opens.
    assert!(r.broker.list_favicons(&r.cua).await.unwrap().is_empty());
    r.broker.browse(&r.cua).await.unwrap();
    let icons = r.broker.list_favicons(&r.cua).await.unwrap();
    assert_eq!(icons.len(), 1);
    assert_eq!(icons[0].site, "example.com");
    // A third party never reads them.
    assert!(r.broker.list_favicons(&r.agent).await.is_err());
    // Deleting the site's last item drops its icon.
    let items = all_items(&r).await;
    r.broker.delete_items(&r.cua, ids(&items)).await.unwrap();
    assert!(r.broker.list_favicons(&r.cua).await.unwrap().is_empty());
}
