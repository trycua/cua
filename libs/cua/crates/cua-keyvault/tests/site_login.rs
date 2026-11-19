// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Site login through the broker: which saved login is filled, where, under
//! which authority, and that the password never shows up anywhere but the
//! fill itself.

mod common;

use std::time::Duration;

use common::{PASSWORD, import_passwords, rig};
use cua_keyvault::broker::{
    AccessRequest, ApproveOptions, BrowserRef, Decision, LoginRequest, RuleSpec, Selector,
    TeleportRequest,
};
use cua_keyvault::model::{ItemKind, ItemPolicy, RuleCaller};
use cua_keyvault::{Action, Error};

async fn approved_token(r: &common::Rig, site: &str, target: &str) -> String {
    let p = r
        .broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Login { site: site.into() }],
                targets: vec![target.into()],
                actions: vec![Action::Login],
                agent: Some("ada".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.broker
        .approve(&r.cua, &p.id, ApproveOptions::default())
        .await
        .unwrap();
    match r
        .broker
        .await_decision(&r.agent, &p.id, Duration::from_millis(20))
        .await
        .unwrap()
    {
        Decision::Granted { token, .. } => token,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn import_makes_one_sealed_item_per_site() {
    let r = rig().await;
    let items = import_passwords(&r).await;
    assert_eq!(items.len(), 2);
    for it in &items {
        assert_eq!(it.kind, ItemKind::Password);
        assert!(it.domain.is_some() && !it.key.is_empty(), "{it:?}");
    }
    // Presence was asked once, naming the browser.
    let asked = r.presence.asked.lock().unwrap().clone();
    assert!(
        asked
            .iter()
            .any(|a| a.contains("Import every saved password from chrome")),
        "{asked:?}"
    );
    // Only those sites when named.
    let r2 = rig().await;
    let one = r2
        .broker
        .import_passwords(
            &r2.cua,
            cua_keyvault::broker::PasswordImportSpec {
                app: "chrome".into(),
                sites: vec!["github.com".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(one.saved, 1);
    let held = common::all_items(&r2).await;
    assert_eq!(held[0].domain.as_deref(), Some("https://github.com"));
    assert_eq!(held[0].key, "octo");
    // A third party may not import.
    assert!(matches!(
        r.broker
            .import_passwords(&r.agent, Default::default())
            .await,
        Err(Error::Forbidden(_))
    ));
}

#[tokio::test]
async fn an_approved_login_fills_the_exact_saved_login_and_never_echoes_it() {
    let r = rig().await;
    import_passwords(&r).await;
    let token = approved_token(&r, "example.test", "work").await;
    let out = r
        .broker
        .login(
            &r.agent,
            LoginRequest {
                token: Some(token),
                url: "http://LOGIN.example.test:8000/login?next=/".into(),
                target: "work".into(),
                agent: Some("ada".into()),
                browser: BrowserRef {
                    session: Some("s".into()),
                    target_id: Some("bt-1".into()),
                    tab_id: Some("tab-1".into()),
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(out.site, "example.test");
    assert_eq!(out.origin, "http://login.example.test:8000");
    assert_eq!(out.username_hint, "a***@example.test");
    assert!(out.filled.submitted);
    assert_eq!(out.filled.browser.tab_id.as_deref(), Some("tab-1"));
    let fills = r.backend.fills.lock().unwrap().clone();
    assert_eq!(fills.len(), 1);
    assert_eq!(
        fills[0],
        (
            "work".to_string(),
            "http://LOGIN.example.test:8000/login?next=/".to_string(),
            "http://login.example.test:8000".to_string(),
            "ada@example.test".to_string(),
            PASSWORD.to_string()
        )
    );
    // Nothing the caller or the audit log sees carries the password.
    let shown = serde_json::to_string(&out).unwrap();
    assert!(!shown.contains(PASSWORD), "{shown}");
    let audit = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    let text = serde_json::to_string(&audit).unwrap();
    assert!(!text.contains(PASSWORD), "{text}");
    assert!(
        !text.contains("ada@example.test"),
        "username is masked: {text}"
    );
    let fill = audit
        .iter()
        .find(|e| e.event.kind == "login.fill")
        .expect("login.fill audited");
    assert_eq!(fill.event.decision, "allow");
    assert!(
        fill.event.detail.contains("agent=ada"),
        "{}",
        fill.event.detail
    );
    assert!(
        fill.event.detail.contains("grant="),
        "{}",
        fill.event.detail
    );
}

#[tokio::test]
async fn a_token_for_one_site_or_space_does_not_sign_in_elsewhere() {
    let r = rig().await;
    import_passwords(&r).await;
    let token = approved_token(&r, "example.test", "work").await;
    let other_site = r
        .broker
        .login(
            &r.agent,
            LoginRequest {
                token: Some(token.clone()),
                url: "https://github.com/login".into(),
                target: "work".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(other_site, Error::Capability(_) | Error::Forbidden(_)),
        "{other_site:?}"
    );
    let other_space = r
        .broker
        .login(
            &r.agent,
            LoginRequest {
                token: Some(token),
                url: "http://login.example.test:8000/".into(),
                target: "elsewhere".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(other_space, Error::Capability(_) | Error::Forbidden(_)),
        "{other_space:?}"
    );
    // A same-site page on a different origin has no saved login.
    let token = approved_token(&r, "example.test", "work").await;
    let wrong_origin = r
        .broker
        .login(
            &r.agent,
            LoginRequest {
                token: Some(token),
                url: "https://evil.example.test/".into(),
                target: "work".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(wrong_origin, Error::NotFound(_)),
        "{wrong_origin:?}"
    );
    assert!(r.backend.fills.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_refused_fill_is_audited_as_an_error() {
    let r = rig().await;
    import_passwords(&r).await;
    *r.backend.refuse_fill.lock().unwrap() = true;
    let token = approved_token(&r, "example.test", "work").await;
    let err = r
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
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)), "{err:?}");
    let audit = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    assert!(
        audit
            .iter()
            .any(|e| e.event.kind == "login.fill" && e.event.decision == "error")
    );
}

#[tokio::test]
async fn unattended_login_only_after_the_user_writes_a_rule() {
    let r = rig().await;
    let items = import_passwords(&r).await;
    let item = items
        .iter()
        .find(|i| i.domain.as_deref() == Some("http://login.example.test:8000"))
        .unwrap();
    let req = LoginRequest {
        url: "http://login.example.test:8000/".into(),
        target: "work".into(),
        ..Default::default()
    };
    assert!(r.broker.login(&r.agent, req.clone()).await.is_err());
    // The user opts the item in (presence) and writes a rule for the caller.
    r.broker
        .set_item_policy(
            &r.cua,
            &item.id,
            ItemPolicy {
                unattended: true,
                ..item.policy.clone()
            },
        )
        .await
        .unwrap();
    r.broker.remember_caller(&r.agent).await;
    r.broker
        .add_rule(
            &r.cua,
            RuleSpec {
                items: vec![item.id.clone()],
                targets: vec!["work".into()],
                callers: vec![RuleCaller {
                    fp: r.agent.fingerprint(),
                    display: r.agent.display(),
                }],
                duration_secs: None,
                note: "ada signs in on her own".into(),
            },
        )
        .await
        .unwrap();
    let out = r.broker.login(&r.agent, req.clone()).await.unwrap();
    assert!(out.authority.starts_with("rule="), "{}", out.authority);
    // The rule does not reach another Space.
    assert!(
        r.broker
            .login(
                &r.agent,
                LoginRequest {
                    target: "elsewhere".into(),
                    ..req
                }
            )
            .await
            .is_err()
    );
    assert_eq!(r.backend.fills.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn saved_logins_are_never_teleported() {
    let r = rig().await;
    let items = import_passwords(&r).await;
    let err = r
        .broker
        .teleport(
            &r.cua,
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
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert!(r.backend.delivered.lock().unwrap().is_empty());
    // And a request for a site with no saved password says so up front.
    let none = r
        .broker
        .request_access(
            &r.agent,
            AccessRequest {
                selectors: vec![Selector::Login {
                    site: "nowhere.test".into(),
                }],
                targets: vec!["work".into()],
                actions: vec![Action::Login],
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(none, Error::NotFound(_)), "{none:?}");
}

#[tokio::test]
async fn a_sign_in_is_audited_under_the_agent_the_user_approved() {
    let r = rig().await;
    import_passwords(&r).await;
    let token = approved_token(&r, "example.test", "work").await;
    let grants = r.broker.list_grants(&r.cua).await.unwrap();
    assert_eq!(grants[0].agent.as_deref(), Some("ada"));
    // The retry names another agent: the audit keeps the approved one.
    r.broker
        .login(
            &r.agent,
            LoginRequest {
                token: Some(token),
                url: "http://login.example.test:8000/".into(),
                target: "work".into(),
                agent: Some("mallory".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let audit = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    let fill = audit.iter().find(|e| e.event.kind == "login.fill").unwrap();
    assert!(
        fill.event.detail.contains("agent=ada"),
        "{}",
        fill.event.detail
    );
    assert!(
        !fill.event.detail.contains("mallory"),
        "{}",
        fill.event.detail
    );
}
