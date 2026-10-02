// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault's default is "ask": with default settings, every use of a
//! secret or credential needs a live approval. This file owns that contract
//! for every path a secret can leave the vault by:
//!
//! - a third party's teleport or site login needs an approved request, and
//!   an approval covers exactly one use unless the user widens it;
//! - no unattended rule exists until the user writes one, and an item may not
//!   be named in one until the user opts that item in (with presence);
//! - a first party's teleport or site login asks for user presence each time.

mod common;

use std::time::Duration;

use common::{PASSWORD, all_items, import_passwords, rig};
use cua_keyvault::broker::{
    AccessRequest, ApproveOptions, Decision, ImportSpec, LoginRequest, RuleSpec, Selector,
    SiteChoice, TeleportRequest,
};
use cua_keyvault::model::{ItemPolicy, RuleCaller};
use cua_keyvault::{Action, Error};

fn login(url: &str, target: &str, token: Option<String>) -> LoginRequest {
    LoginRequest {
        token,
        url: url.into(),
        target: target.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_fresh_vault_asks_for_everything() {
    let r = rig().await;
    // No rule and no grant exist until the user makes one.
    assert!(r.broker.list_rules(&r.cua).await.unwrap().is_empty());
    assert!(r.broker.list_grants(&r.cua).await.unwrap().is_empty());
    // Items default to "never unattended" with a bounded lifetime on target.
    assert!(!ItemPolicy::default().unattended);
    for it in import_passwords(&r).await {
        assert!(!it.policy.unattended, "{}", it.label());
        assert!(it.policy.allowed_targets.is_empty());
    }
    r.broker
        .import(
            &r.cua,
            ImportSpec {
                app: "chrome".into(),
                sites: vec![SiteChoice {
                    site: "github.com".into(),
                    include_storage: false,
                    include_passwords: false,
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let sites: Vec<_> = all_items(&r)
        .await
        .into_iter()
        .filter(|i| i.kind == cua_keyvault::ItemKind::Cookie)
        .collect();
    assert!(!sites[0].policy.unattended);
    // A rule naming an item that was not opted in is refused.
    let err = r
        .broker
        .add_rule(
            &r.cua,
            RuleSpec {
                items: vec![sites[0].id.clone()],
                targets: vec!["work".into()],
                callers: vec![RuleCaller {
                    fp: r.agent.fingerprint(),
                    display: r.agent.display(),
                }],
                duration_secs: None,
                note: String::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)), "{err:?}");
}

#[tokio::test]
async fn a_third_party_login_without_approval_is_refused_and_types_nothing() {
    let r = rig().await;
    import_passwords(&r).await;
    let err = r
        .broker
        .login(
            &r.agent,
            login("http://login.example.test:8000/", "work", None),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)), "{err:?}");
    assert!(err.to_string().contains("request access first"), "{err}");
    assert!(r.backend.fills.lock().unwrap().is_empty());
    let audit = r.broker.audit_tail(&r.cua, 50).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.event.kind == "login.denied" && e.event.target.as_deref() == Some("work"))
    );
}

#[tokio::test]
async fn a_request_is_pending_until_the_user_answers_and_denied_on_deny() {
    let r = rig().await;
    import_passwords(&r).await;
    let p = r
        .broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Login {
                    site: "example.test".into(),
                }],
                targets: vec!["work".into()],
                actions: vec![Action::Login],
                agent: Some("ada".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        r.broker
            .await_decision(&r.agent, &p.id, Duration::from_millis(20))
            .await
            .unwrap(),
        Decision::Pending
    );
    r.broker.deny(&r.cua, &p.id).await.unwrap();
    assert!(matches!(
        r.broker
            .await_decision(&r.agent, &p.id, Duration::from_millis(20))
            .await
            .unwrap(),
        Decision::Denied { .. }
    ));
    assert!(r.backend.fills.lock().unwrap().is_empty());
    let kinds: Vec<String> = r
        .broker
        .audit_tail(&r.cua, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.event.kind)
        .collect();
    assert!(kinds.contains(&"login.request".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"login.denied".to_string()), "{kinds:?}");
}

#[tokio::test]
async fn an_approval_covers_exactly_one_use_by_default() {
    let r = rig().await;
    import_passwords(&r).await;
    let p = r
        .broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Login {
                    site: "example.test".into(),
                }],
                targets: vec!["work".into()],
                actions: vec![Action::Login],
                agent: Some("ada".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let g = r
        .broker
        .approve(&r.cua, &p.id, ApproveOptions::default())
        .await
        .unwrap();
    assert_eq!(g.uses_left, Some(1), "one use unless the user widens it");
    // The consent the user saw names the agent, the site and the Space.
    let asked = r.presence.asked.lock().unwrap().last().cloned().unwrap();
    assert!(asked.contains("agent ada"), "{asked}");
    assert!(asked.contains("sign in to example.test"), "{asked}");
    assert!(asked.contains("work"), "{asked}");
    assert!(asked.contains("once"), "{asked}");
    let Decision::Granted { token, .. } = r
        .broker
        .await_decision(&r.agent, &p.id, Duration::from_millis(20))
        .await
        .unwrap()
    else {
        panic!("granted")
    };
    let url = "http://login.example.test:8000/login";
    r.broker
        .login(&r.agent, login(url, "work", Some(token.clone())))
        .await
        .unwrap();
    let again = r
        .broker
        .login(&r.agent, login(url, "work", Some(token)))
        .await
        .unwrap_err();
    assert!(
        matches!(again, Error::Capability(_) | Error::Forbidden(_)),
        "{again:?}"
    );
    assert_eq!(r.backend.fills.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_teleport_token_is_one_use_by_default_and_needs_approval() {
    let r = rig().await;
    r.broker
        .import(
            &r.cua,
            ImportSpec {
                app: "chrome".into(),
                sites: vec![SiteChoice {
                    site: "github.com".into(),
                    include_storage: false,
                    include_passwords: false,
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let items = all_items(&r).await;
    let no_token = r
        .broker
        .teleport(
            &r.agent,
            TeleportRequest {
                token: None,
                items: vec![items[0].id.clone()],
                target: "work".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(no_token, Error::Forbidden(_)), "{no_token:?}");
    let p = r
        .broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Item {
                    id: items[0].id.clone(),
                }],
                targets: vec!["work".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.broker
        .approve(&r.cua, &p.id, ApproveOptions::default())
        .await
        .unwrap();
    let Decision::Granted { token, .. } = r
        .broker
        .await_decision(&r.agent, &p.id, Duration::from_millis(20))
        .await
        .unwrap()
    else {
        panic!("granted")
    };
    let req = TeleportRequest {
        token: Some(token),
        items: vec![items[0].id.clone()],
        target: "work".into(),
        include_passwords: false,
        launch: false,
    };
    r.broker.teleport(&r.agent, req.clone()).await.unwrap();
    assert!(r.broker.teleport(&r.agent, req).await.is_err());
}

#[tokio::test]
async fn a_first_party_login_asks_for_presence_every_time() {
    let r = rig().await;
    import_passwords(&r).await;
    let url = "http://login.example.test:8000/";
    let before = r.presence.asked.lock().unwrap().len();
    r.broker
        .login(&r.cua, login(url, "work", None))
        .await
        .unwrap();
    r.broker
        .login(&r.cua, login(url, "work", None))
        .await
        .unwrap();
    let asked = r.presence.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), before + 2, "{asked:?}");
    assert!(asked.last().unwrap().starts_with("Sign in to example.test"));
    // Declining presence refuses and types nothing more.
    r.presence.set(false);
    let err = r
        .broker
        .login(&r.cua, login(url, "work", None))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PresenceFailed(_)), "{err:?}");
    assert_eq!(r.backend.fills.lock().unwrap().len(), 2);
    let fills = r.backend.fills.lock().unwrap();
    assert_eq!(fills[0].4, PASSWORD);
}
