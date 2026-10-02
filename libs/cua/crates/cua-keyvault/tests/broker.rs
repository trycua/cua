// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Broker behaviour end to end, with a fake capture/delivery backend and a
//! fake presence gate. Nothing touches the real machine: the vault lives in
//! a temp dir and uses passphrase protectors.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_keyvault::broker::{
    AccessRequest, ApproveOptions, Backend, Broker, BrokerConfig, Captured, Decision,
    DeliveryOutcome, FakePresence, ImportSpec, InitRequest, Inventory, RuleSpec, Selector,
    SiteChoice, TeleportRequest, TeleportStage, UnlockRequest,
};
use cua_keyvault::model::{
    ItemKind, ItemMeta, ItemPolicy, LoginRecord, NO_EXPIRY, PayloadEntry, RuleCaller,
};
use cua_keyvault::protector::ProtectorKind;
use cua_keyvault::record::{self, CookieRecord};
use cua_keyvault::{CallerIdentity, Error, Signing};

#[derive(Default)]
#[allow(clippy::type_complexity)]
struct FakeBackend {
    delivered: Mutex<Vec<(String, String, Vec<String>, u64)>>,
    /// The cookie hosts each delivery carried (from its `cookies.json`).
    delivered_hosts: Mutex<Vec<Vec<String>>>,
    wiped: Mutex<Vec<(String, String)>>,
    notified: Mutex<Vec<String>>,
    /// The `launch` flag of every launching delivery.
    launches: Mutex<Vec<bool>>,
    captures: Mutex<Vec<ImportSpec>>,
    /// Name -> immutable Space id. A name absent here resolves to a stable
    /// `sandbox-<name>`; a test can insert a new id to model a rename or a
    /// re-created Space (red-team F1).
    target_ids: Mutex<std::collections::HashMap<String, String>>,
}

impl FakeBackend {
    /// Repoints a name at a different immutable id (an attacker deleting and
    /// re-creating a Space under the same name).
    fn rebind(&self, name: &str, id: &str) {
        self.target_ids
            .lock()
            .unwrap()
            .insert(name.to_string(), id.to_string());
    }
}

fn cap(n: record::NewRecord, app: &str) -> Captured {
    let (mut meta, payload) = n.into_item(app, app, "Default", "full");
    meta.session = true;
    Captured { meta, payload }
}

/// One cookie of `site`.
fn cookie_item(app: &str, site: &str, name: &str) -> Captured {
    cap(
        record::cookie_record(&CookieRecord {
            creation_utc: None,
            expires_utc: 0,
            host_key: format!(".{site}"),
            http_only: true,
            last_update_utc: None,
            name: name.into(),
            partition_key: None,
            last_access_utc: None,
            source_type: None,
            has_cross_site_ancestor: None,
            path: "/".into(),
            priority: None,
            same_site: 1,
            secure: true,
            source_port: None,
            source_scheme: None,
            value: b"FIXTURE-SECRET".to_vec(),
        })
        .unwrap(),
        app,
    )
}

fn file_item(app: &str, path: &str) -> Captured {
    cap(
        record::file_record(path, 0o600, b"FILE-SECRET").unwrap(),
        app,
    )
}

fn password_item(app: &str, site: &str, user: &str) -> Captured {
    cap(
        record::password_record(&LoginRecord {
            origin: format!("https://{site}"),
            username: user.into(),
            password: "PW-SECRET".into(),
        })
        .unwrap(),
        app,
    )
}

#[async_trait::async_trait]
impl Backend for FakeBackend {
    fn resolve_target(&self, name: &str) -> cua_keyvault::Result<String> {
        Ok(self
            .target_ids
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_else(|| format!("sandbox-{name}")))
    }

    fn inventory(&self, app: &str, _profile: Option<&str>) -> cua_keyvault::Result<Inventory> {
        Ok(Inventory {
            provider_id: app.into(),
            app_display: app.into(),
            profile: None,
            domains: vec![cua_keyvault::broker::DomainInventory {
                domain: "github.com".into(),
                cookies: 2,
                ..Default::default()
            }],
            notes: vec![],
        })
    }

    fn capture(&self, spec: &ImportSpec) -> cua_keyvault::Result<Vec<Captured>> {
        self.captures.lock().unwrap().push(spec.clone());
        let mut out = Vec::new();
        for s in &spec.sites {
            out.push(cookie_item(&spec.app, &s.site, "user_session"));
            if s.include_passwords {
                out.push(password_item(&spec.app, &s.site, "octo"));
            }
        }
        if spec.whole_app {
            for p in spec
                .paths
                .clone()
                .unwrap_or_else(|| vec!["app/session.json".into()])
            {
                out.push(file_item(&spec.app, &p));
            }
        }
        Ok(out)
    }

    async fn deliver(
        &self,
        target: &str,
        provider_id: &str,
        _scope: &str,
        entries: Vec<PayloadEntry>,
        expires_ms: u64,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        let paths: Vec<String> = entries.iter().map(|e| e.rel_path.clone()).collect();
        let hosts: Vec<String> = entries
            .iter()
            .filter(|e| e.rel_path == record::COOKIES_ENTRY)
            .flat_map(|e| {
                use base64::Engine as _;
                let raw = base64::engine::general_purpose::STANDARD
                    .decode(&e.data)
                    .unwrap();
                let rows: Vec<serde_json::Value> = serde_json::from_slice(&raw).unwrap();
                rows.iter()
                    .map(|r| r["host_key"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        self.delivered_hosts.lock().unwrap().push(hosts);
        let mut d = self.delivered.lock().unwrap();
        d.push((target.into(), provider_id.into(), paths.clone(), expires_ms));
        Ok(DeliveryOutcome {
            import_id: format!("imp-{}", d.len()),
            imported: paths,
            skipped: vec![],
            launched: false,
        })
    }

    async fn deliver_with_progress(
        &self,
        target: &str,
        provider_id: &str,
        scope: &str,
        entries: Vec<PayloadEntry>,
        expires_ms: u64,
        stage: cua_keyvault::broker::StageSink,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        stage(TeleportStage::Packing);
        stage(TeleportStage::Uploading { done: 0, total: 8 });
        stage(TeleportStage::Uploading { done: 8, total: 8 });
        stage(TeleportStage::Importing);
        self.deliver(target, provider_id, scope, entries, expires_ms)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver_launching(
        &self,
        target: &str,
        provider_id: &str,
        scope: &str,
        entries: Vec<PayloadEntry>,
        expires_ms: u64,
        stage: cua_keyvault::broker::StageSink,
        launch: bool,
    ) -> cua_keyvault::Result<DeliveryOutcome> {
        self.launches.lock().unwrap().push(launch);
        let mut out = self
            .deliver_with_progress(target, provider_id, scope, entries, expires_ms, stage)
            .await?;
        out.launched = launch;
        Ok(out)
    }

    async fn wipe(&self, target: &str, import_id: &str) -> cua_keyvault::Result<Vec<String>> {
        self.wiped
            .lock()
            .unwrap()
            .push((target.into(), import_id.into()));
        Ok(vec![])
    }

    fn notify_consent(&self, pending: &cua_keyvault::broker::PendingView) {
        self.notified.lock().unwrap().push(pending.id.clone());
    }
}

struct Rig {
    _dir: tempfile::TempDir,
    broker: Broker,
    backend: Arc<FakeBackend>,
    presence: Arc<FakePresence>,
    cua: CallerIdentity,
    koala: CallerIdentity,
    evil: CallerIdentity,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::default());
    let presence = Arc::new(FakePresence::new(true));
    let broker = Broker::new(
        BrokerConfig {
            dir: dir.path().join("keyvault"),
            keychain_path: None,
            os_protector: false,
        },
        backend.clone(),
        presence.clone(),
    )
    .unwrap();
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    let koala = CallerIdentity::for_tests("com.example.koalabot", false);
    let evil = CallerIdentity::for_tests("com.example.evil", false);
    broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("correct horse battery".into()),
                recovery_key: true,
            },
        )
        .await
        .unwrap();
    Rig {
        _dir: dir,
        broker,
        backend,
        presence,
        cua,
        koala,
        evil,
    }
}

/// Every item, with names visible (opens the browse window).
async fn all_items(r: &Rig) -> Vec<ItemMeta> {
    r.broker.browse(&r.cua).await.unwrap();
    r.broker.list_items(&r.cua, 0, 1000).await.unwrap().items
}

/// Imports `sites` and returns their cookie items, in site order.
async fn import_sites(r: &Rig, sites: &[&str]) -> Vec<ItemMeta> {
    r.broker
        .import(
            &r.cua,
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
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let items = all_items(r).await;
    sites
        .iter()
        .filter_map(|s| {
            items
                .iter()
                .find(|i| {
                    i.kind == ItemKind::Cookie
                        && i.domain.as_deref().map(|d| d.trim_start_matches('.')) == Some(*s)
                })
                .cloned()
        })
        .collect()
}

async fn grant(
    r: &Rig,
    caller: &CallerIdentity,
    site: &str,
    target: &str,
) -> (String, Vec<String>) {
    let pending = r
        .broker
        .request_access(
            caller,
            AccessRequest {
                selectors: vec![Selector::Site {
                    app: "chrome".into(),
                    site: site.into(),
                }],
                targets: vec![target.into()],
                uses: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.broker
        .approve(&r.cua, &pending.id, ApproveOptions::default())
        .await
        .unwrap();
    match r
        .broker
        .await_decision(caller, &pending.id, Duration::from_millis(10))
        .await
        .unwrap()
    {
        Decision::Granted { token, items, .. } => (token, items),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn third_parties_cannot_use_first_party_operations() {
    let r = rig().await;
    import_sites(&r, &["github.com"]).await;
    for res in [
        r.broker.list_items(&r.koala, 0, 10).await.map(|_| ()),
        r.broker.list_pending(&r.koala).await.map(|_| ()),
        r.broker.set_disabled(&r.koala, false).await,
        r.broker.list_grants(&r.koala).await.map(|_| ()),
        r.broker.audit_tail(&r.koala, 10).await.map(|_| ()),
        r.broker
            .import(&r.koala, ImportSpec::default())
            .await
            .map(|_| ()),
    ] {
        assert!(matches!(res, Err(Error::Forbidden(_))), "{res:?}");
    }
    // A third party asking to approve its own request is refused.
    let p = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::Site {
                    app: "chrome".into(),
                    site: "github.com".into(),
                }],
                targets: vec!["dev-1".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        r.broker
            .approve(&r.koala, &p.id, ApproveOptions::default())
            .await,
        Err(Error::Forbidden(_))
    ));
    // Status never leaks counts to third parties.
    let s = r.broker.status(&r.koala).await;
    assert_eq!((s.items, s.pending, s.caller_first_party), (0, 0, false));
    assert!(r.backend.notified.lock().unwrap().contains(&p.id));
}

#[tokio::test]
async fn consent_grants_a_caller_bound_scoped_token() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com", "gitlab.com"]).await;
    let github = items
        .iter()
        .find(|i| i.domain.as_deref() == Some(".github.com"))
        .unwrap();
    let gitlab = items
        .iter()
        .find(|i| i.domain.as_deref() == Some(".gitlab.com"))
        .unwrap();
    let (token, granted) = grant(&r, &r.koala, "github.com", "dev-1").await;
    assert_eq!(granted, vec![github.id.clone()]);
    let asked = r.presence.asked.lock().unwrap().clone();
    assert!(
        asked
            .iter()
            .any(|a| a.contains("com.example.koalabot") && a.contains("dev-1")),
        "the presence prompt names the verified caller and target: {asked:?}"
    );

    let req = |items: Vec<String>, target: &str| TeleportRequest {
        token: Some(token.clone()),
        items,
        target: target.into(),
        include_passwords: false,
        launch: false,
    };
    let out = r
        .broker
        .teleport(&r.koala, req(vec![github.id.clone()], "dev-1"))
        .await
        .unwrap();
    assert!(out.authority.starts_with("grant="));
    // Stolen by another app: bound to koalabot's fingerprint.
    assert!(matches!(
        r.broker
            .teleport(&r.evil, req(vec![github.id.clone()], "dev-1"))
            .await,
        Err(Error::Capability(_))
    ));
    // Another item or another target: out of scope.
    assert!(
        r.broker
            .teleport(&r.koala, req(vec![gitlab.id.clone()], "dev-1"))
            .await
            .is_err()
    );
    assert!(
        r.broker
            .teleport(&r.koala, req(vec![github.id.clone()], "prod"))
            .await
            .is_err()
    );
    // A forged or attenuated-then-edited token fails.
    let mut forged = token.clone();
    forged.pop();
    forged.push(if token.ends_with('A') { 'B' } else { 'A' });
    assert!(
        r.broker
            .teleport(
                &r.koala,
                TeleportRequest {
                    token: Some(forged),
                    items: vec![github.id.clone()],
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                }
            )
            .await
            .is_err()
    );
    // Without a token a third party gets nothing.
    assert!(matches!(
        r.broker
            .teleport(
                &r.koala,
                TeleportRequest {
                    token: None,
                    items: vec![github.id.clone()],
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                }
            )
            .await,
        Err(Error::Forbidden(_))
    ));
}

#[tokio::test]
async fn f1_grant_is_bound_to_the_immutable_target_id_not_the_name() {
    // Red-team F1: a grant to "dev-1" must fail closed once "dev-1" is deleted
    // and re-created (a new immutable id) under the same name, even though the
    // caller, item, target name, expiry and epoch all still match.
    let r = rig().await;
    import_sites(&r, &["github.com"]).await;
    let (token, granted) = grant(&r, &r.koala, "github.com", "dev-1").await;
    let req = TeleportRequest {
        token: Some(token),
        items: granted.clone(),
        target: "dev-1".into(),
        include_passwords: false,
        launch: false,
    };
    // While the id is unchanged the token works.
    r.broker.teleport(&r.koala, req.clone()).await.unwrap();
    // The attacker re-points the name at a Space they control.
    r.backend.rebind("dev-1", "sandbox-ATTACKER");
    let err = r.broker.teleport(&r.koala, req).await.unwrap_err();
    assert!(
        matches!(err, Error::Capability(_)),
        "expected rebinding refusal, got {err:?}"
    );
    assert!(
        r.backend.delivered.lock().unwrap().len() == 1,
        "no delivery after rebinding"
    );
}

#[tokio::test]
async fn f1_unattended_rule_is_bound_to_the_immutable_target_id() {
    // Red-team F1 for the unattended path: a rule naming "dev-1" must not
    // deliver into a re-created "dev-1" with a different id.
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let id = items[0].id.clone();
    r.broker
        .set_item_policy(
            &r.cua,
            &id,
            ItemPolicy {
                unattended: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.broker.remember_caller(&r.koala).await;
    let rule = r
        .broker
        .add_rule(
            &r.cua,
            RuleSpec {
                items: vec![id.clone()],
                targets: vec!["dev-1".into()],
                callers: vec![RuleCaller {
                    fp: r.koala.fingerprint(),
                    display: String::new(),
                }],
                duration_secs: Some(3600),
                note: "nightly".into(),
            },
        )
        .await
        .unwrap();
    let req = TeleportRequest {
        token: None,
        items: vec![id],
        target: "dev-1".into(),
        include_passwords: false,
        launch: false,
    };
    let out = r.broker.teleport(&r.koala, req.clone()).await.unwrap();
    assert_eq!(out.authority, format!("rule={}", rule.id));
    // Re-create dev-1 under a new id: the rule must now refuse.
    r.backend.rebind("dev-1", "sandbox-ATTACKER");
    assert!(matches!(
        r.broker.teleport(&r.koala, req).await,
        Err(Error::Forbidden(_))
    ));
}

#[tokio::test]
async fn single_use_grants_and_other_callers_cannot_collect_decisions() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let p = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::Item {
                    id: items[0].id.clone(),
                }],
                targets: vec!["dev-1".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        r.broker
            .await_decision(&r.koala, &p.id, Duration::from_millis(5))
            .await
            .unwrap(),
        Decision::Pending
    ));
    r.broker
        .approve(&r.cua, &p.id, ApproveOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        r.broker
            .await_decision(&r.evil, &p.id, Duration::from_millis(5))
            .await,
        Err(Error::Forbidden(_))
    ));
    let Decision::Granted { token, .. } = r
        .broker
        .await_decision(&r.koala, &p.id, Duration::from_millis(5))
        .await
        .unwrap()
    else {
        panic!()
    };
    let req = TeleportRequest {
        token: Some(token),
        items: vec![items[0].id.clone()],
        target: "dev-1".into(),
        include_passwords: false,
        launch: false,
    };
    r.broker.teleport(&r.koala, req.clone()).await.unwrap();
    // Default grants are single use.
    assert!(r.broker.teleport(&r.koala, req).await.is_err());
}

#[tokio::test]
async fn presence_is_required_to_expand_access() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    r.presence.set(false);
    assert!(matches!(
        import_sites_result(&r, "gitlab.com").await,
        Err(Error::PresenceFailed(_))
    ));
    let p = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::Item {
                    id: items[0].id.clone(),
                }],
                targets: vec!["dev-1".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        r.broker
            .approve(&r.cua, &p.id, ApproveOptions::default())
            .await,
        Err(Error::PresenceFailed(_))
    ));
    // Interactive first-party teleport also needs presence.
    assert!(matches!(
        r.broker
            .teleport(
                &r.cua,
                TeleportRequest {
                    token: None,
                    items: vec![items[0].id.clone()],
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                }
            )
            .await,
        Err(Error::PresenceFailed(_))
    ));
    assert!(r.backend.delivered.lock().unwrap().is_empty());
    // Contracting access never needs presence.
    r.broker.deny(&r.cua, &p.id).await.unwrap();
    r.broker.set_disabled(&r.cua, true).await.unwrap();
    // ...but turning the vault back on does.
    assert!(r.broker.set_disabled(&r.cua, false).await.is_err());
    // Passwords need the explicit second confirmation even with presence.
    r.presence.set(true);
    r.broker.set_disabled(&r.cua, false).await.unwrap();
    let err = r
        .broker
        .import(
            &r.cua,
            ImportSpec {
                app: "chrome".into(),
                sites: vec![SiteChoice {
                    site: "bank.example".into(),
                    include_storage: false,
                    include_passwords: true,
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("confirm_passwords"), "{err}");
}

async fn import_sites_result(
    r: &Rig,
    site: &str,
) -> cua_keyvault::Result<cua_keyvault::broker::ImportReport> {
    r.broker
        .import(
            &r.cua,
            ImportSpec {
                app: "chrome".into(),
                sites: vec![SiteChoice {
                    site: site.into(),
                    include_storage: false,
                    include_passwords: false,
                }],
                ..Default::default()
            },
        )
        .await
}

#[tokio::test]
async fn kill_switch_blocks_everything_and_kills_old_tokens() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let (token, _) = grant(&r, &r.koala, "github.com", "dev-1").await;
    r.broker.set_disabled(&r.cua, true).await.unwrap();
    let req = TeleportRequest {
        token: Some(token),
        items: vec![items[0].id.clone()],
        target: "dev-1".into(),
        include_passwords: false,
        launch: false,
    };
    assert!(matches!(
        r.broker.teleport(&r.koala, req.clone()).await,
        Err(Error::Disabled)
    ));
    assert!(matches!(
        r.broker
            .request_access(
                &r.koala,
                AccessRequest {
                    selectors: vec![Selector::Item {
                        id: items[0].id.clone()
                    }],
                    targets: vec!["dev-1".into()],
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Disabled)
    ));
    assert!(matches!(
        import_sites_result(&r, "x.example").await,
        Err(Error::Disabled)
    ));
    r.broker.set_disabled(&r.cua, false).await.unwrap();
    // Re-enabling does not resurrect tokens minted before the switch.
    assert!(matches!(
        r.broker.teleport(&r.koala, req).await,
        Err(Error::Capability(_))
    ));
}

#[tokio::test]
async fn revoking_one_site_leaves_the_others() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com", "gitlab.com"]).await;
    let (t1, i1) = grant(&r, &r.koala, "github.com", "dev-1").await;
    let (t2, i2) = grant(&r, &r.koala, "gitlab.com", "dev-1").await;
    let grants = r.broker.list_grants(&r.cua).await.unwrap();
    let g1 = grants.iter().find(|g| g.items == i1).unwrap().id.clone();
    r.broker.revoke_grant(&r.cua, &g1).await.unwrap();
    assert!(
        r.broker
            .teleport(
                &r.koala,
                TeleportRequest {
                    token: Some(t1),
                    items: i1,
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                }
            )
            .await
            .is_err()
    );
    r.broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(t2),
                items: i2.clone(),
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    // Deleting github.com wipes nothing of gitlab's delivery.
    let github = items
        .iter()
        .find(|i| i.domain.as_deref() == Some(".github.com"))
        .unwrap();
    r.broker
        .delete_items(&r.cua, vec![github.id.clone()])
        .await
        .unwrap();
    assert!(r.backend.wiped.lock().unwrap().is_empty());
    let left = all_items(&r).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, i2[0]);
}

#[tokio::test]
async fn unattended_rules_need_signed_known_callers_and_expire() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let id = items[0].id.clone();
    // The item must opt in to unattended use first.
    let spec = |fp: String| RuleSpec {
        items: vec![id.clone()],
        targets: vec!["dev-1".into()],
        callers: vec![RuleCaller {
            fp,
            display: String::new(),
        }],
        duration_secs: Some(3600),
        note: "nightly agent".into(),
    };
    r.broker.remember_caller(&r.koala).await;
    assert!(
        r.broker
            .add_rule(&r.cua, spec(r.koala.fingerprint()))
            .await
            .is_err()
    );
    r.broker
        .set_item_policy(
            &r.cua,
            &id,
            ItemPolicy {
                unattended: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    // Unknown and unsigned callers are refused.
    assert!(
        r.broker
            .add_rule(&r.cua, spec("fp1-unknown".into()))
            .await
            .is_err()
    );
    let mut unsigned = r.koala.clone();
    unsigned.signing = Signing::Unsigned;
    unsigned.path = Some("/tmp/bot".into());
    r.broker.remember_caller(&unsigned).await;
    assert!(matches!(
        r.broker
            .add_rule(&r.cua, spec(unsigned.fingerprint()))
            .await,
        Err(Error::Forbidden(_))
    ));
    // Third parties cannot add rules.
    assert!(
        r.broker
            .add_rule(&r.koala, spec(r.koala.fingerprint()))
            .await
            .is_err()
    );
    let rule = r
        .broker
        .add_rule(&r.cua, spec(r.koala.fingerprint()))
        .await
        .unwrap();
    let req = TeleportRequest {
        token: None,
        items: vec![id.clone()],
        target: "dev-1".into(),
        include_passwords: false,
        launch: false,
    };
    let asked_before = r.presence.asked.lock().unwrap().len();
    let out = r.broker.teleport(&r.koala, req.clone()).await.unwrap();
    assert_eq!(out.authority, format!("rule={}", rule.id));
    assert_eq!(
        r.presence.asked.lock().unwrap().len(),
        asked_before,
        "unattended: no prompt"
    );
    // Other callers and other targets do not match.
    assert!(r.broker.teleport(&r.evil, req.clone()).await.is_err());
    assert!(
        r.broker
            .teleport(
                &r.koala,
                TeleportRequest {
                    target: "prod".into(),
                    ..req.clone()
                }
            )
            .await
            .is_err()
    );
    // Turning unattended off on the item drops it from the rule.
    r.broker
        .set_item_policy(&r.cua, &id, ItemPolicy::default())
        .await
        .unwrap();
    assert!(r.broker.list_rules(&r.cua).await.unwrap().is_empty());
    assert!(r.broker.teleport(&r.koala, req).await.is_err());
}

#[tokio::test]
async fn deliveries_supersede_and_release_wipes() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com", "gitlab.com"]).await;
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    for id in &ids {
        r.broker
            .teleport(
                &r.cua,
                TeleportRequest {
                    token: None,
                    items: vec![id.clone()],
                    target: "dev-1".into(),
                    include_passwords: false,
                    launch: false,
                },
            )
            .await
            .unwrap();
    }
    let delivered = r.backend.delivered.lock().unwrap().clone();
    assert_eq!(delivered.len(), 2);
    // The second delivery carries both sites (one live import per target and
    // browser) and supersedes the first. Cookies travel as one `cookies.json`.
    assert_eq!(delivered[1].2, vec!["cookies.json".to_string()]);
    let hosts = r.backend.delivered_hosts.lock().unwrap().clone();
    assert_eq!(hosts[0], vec![".github.com".to_string()]);
    let mut both = hosts[1].clone();
    both.sort();
    assert_eq!(
        both,
        vec![".github.com".to_string(), ".gitlab.com".to_string()]
    );
    assert_eq!(
        r.backend.wiped.lock().unwrap().as_slice(),
        [("dev-1".to_string(), "imp-1".to_string())]
    );
    let live: Vec<_> = r
        .broker
        .list_deliveries(&r.cua)
        .await
        .unwrap()
        .into_iter()
        .filter(|d| !d.wiped)
        .collect();
    assert_eq!(live.len(), 1);
    let wiped = r.broker.release(&r.cua, "dev-1").await.unwrap();
    assert_eq!(wiped, vec!["imp-2".to_string()]);
    // Auto-wipe is off by default: the copies carry no expiry and stay
    // until wiped (the receiver reads 0 as never).
    assert!(delivered.iter().all(|d| d.3 == NO_EXPIRY), "{delivered:?}");
}

async fn teleport_to(r: &Rig, id: &str, target: &str) {
    r.broker
        .teleport(
            &r.cua,
            TeleportRequest {
                token: None,
                items: vec![id.to_string()],
                target: target.into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn auto_wipe_is_off_by_default_and_the_setting_picks_the_expiry() {
    let r = rig().await;
    let status = r.broker.status(&r.cua).await;
    assert_eq!(status.auto_wipe, Some(false), "off by default");
    // Third parties never learn the setting, nor change it.
    assert_eq!(r.broker.status(&r.koala).await.auto_wipe, None);
    assert!(r.broker.set_auto_wipe(&r.koala, true).await.is_err());
    let items = import_sites(&r, &["github.com", "gitlab.com"]).await;
    // Off: no expiry, and the copy stays live (it is superseded, not
    // dropped, by the next delivery to the same Space).
    teleport_to(&r, &items[0].id, "dev-1").await;
    let live = r
        .broker
        .list_deliveries(&r.cua)
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.live(cua_keyvault::now_ms()))
        .count();
    assert_eq!(live, 1);
    // On: turning it on needs no presence; the item's TTL applies again.
    let asked = r.presence.asked.lock().unwrap().len();
    r.broker.set_auto_wipe(&r.cua, true).await.unwrap();
    assert_eq!(r.presence.asked.lock().unwrap().len(), asked);
    assert_eq!(r.broker.status(&r.cua).await.auto_wipe, Some(true));
    teleport_to(&r, &items[0].id, "dev-2").await;
    // A shorter per-item TTL is respected.
    let mut short = items[1].policy.clone();
    short.ttl_secs = 600;
    r.broker
        .set_item_policy(&r.cua, &items[1].id, short)
        .await
        .unwrap();
    teleport_to(&r, &items[1].id, "dev-3").await;
    let delivered = r.backend.delivered.lock().unwrap().clone();
    let now = cua_keyvault::now_ms();
    assert_eq!(delivered[0].3, NO_EXPIRY);
    let hour = delivered[1].3.saturating_sub(now);
    assert!(hour > 3500 * 1000 && hour <= 3600 * 1000, "{hour}");
    let ten = delivered[2].3.saturating_sub(now);
    assert!(ten > 500 * 1000 && ten <= 600 * 1000, "{ten}");
    // Turning it off keeps copies longer: it asks for presence, and a
    // declined prompt changes nothing.
    r.presence.set(false);
    assert!(r.broker.set_auto_wipe(&r.cua, false).await.is_err());
    assert_eq!(r.broker.status(&r.cua).await.auto_wipe, Some(true));
    r.presence.set(true);
    r.broker.set_auto_wipe(&r.cua, false).await.unwrap();
    assert_eq!(r.broker.status(&r.cua).await.auto_wipe, Some(false));
    let tail = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    assert!(
        tail.iter()
            .any(|e| e.event.kind == "settings.update" && e.event.detail == "auto_wipe=false")
    );
}

#[tokio::test]
async fn a_copy_without_expiry_still_goes_on_wipe_delete_and_the_kill_switch() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com", "gitlab.com"]).await;
    teleport_to(&r, &items[0].id, "dev-1").await;
    teleport_to(&r, &items[1].id, "dev-2").await;
    teleport_to(&r, &items[1].id, "dev-3").await;
    let live = |ds: Vec<cua_keyvault::model::Delivery>| -> Vec<String> {
        let now = cua_keyvault::now_ms() + 365 * 24 * 3600 * 1000;
        let mut t: Vec<String> = ds
            .into_iter()
            .filter(|d| d.live(now))
            .map(|d| d.target)
            .collect();
        t.sort();
        t
    };
    // A year on, nothing expired by itself.
    assert_eq!(
        live(r.broker.list_deliveries(&r.cua).await.unwrap()),
        ["dev-1", "dev-2", "dev-3"]
    );
    // Wipe.
    r.broker.release(&r.cua, "dev-1").await.unwrap();
    assert_eq!(
        live(r.broker.list_deliveries(&r.cua).await.unwrap()),
        ["dev-2", "dev-3"]
    );
    // Deleting the item wipes its copies.
    r.broker
        .delete_items(&r.cua, vec![items[1].id.clone()])
        .await
        .unwrap();
    assert!(live(r.broker.list_deliveries(&r.cua).await.unwrap()).is_empty());
    // The kill switch wipes the rest.
    teleport_to(&r, &items[0].id, "dev-4").await;
    r.broker.set_disabled(&r.cua, true).await.unwrap();
    assert!(live(r.broker.list_deliveries(&r.cua).await.unwrap()).is_empty());
}

#[tokio::test]
async fn audit_records_decisions_without_secrets_or_sites() {
    let r = rig().await;
    import_sites(&r, &["github.com"]).await;
    let (token, items) = grant(&r, &r.koala, "github.com", "dev-1").await;
    r.broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token.clone()),
                items,
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    let _ = r
        .broker
        .teleport(
            &r.evil,
            TeleportRequest {
                token: Some(token.clone()),
                items: vec!["00".into()],
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await;
    let v = r.broker.verify_audit(&r.cua).await.unwrap();
    assert!(v.ok(), "{v:?}");
    let kinds: Vec<String> = r
        .broker
        .audit_tail(&r.cua, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.event.kind)
        .collect();
    for k in [
        "vault.create",
        "item.import",
        "consent.request",
        "consent.allow",
        "teleport.deliver",
    ] {
        assert!(kinds.iter().any(|x| x == k), "{k} missing from {kinds:?}");
    }
    let raw = std::fs::read_to_string(r._dir.path().join("keyvault/audit.log")).unwrap();
    assert!(!raw.contains("github.com"), "the audit log names no sites");
    assert!(!raw.contains("RklYVFVSRS1TRUNSRVQ"), "no payload bytes");
    assert!(!raw.contains(&token), "no tokens");
}

#[tokio::test]
async fn pending_requests_are_capped_per_caller() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let req = || AccessRequest {
        selectors: vec![Selector::Item {
            id: items[0].id.clone(),
        }],
        targets: vec!["dev-1".into()],
        ..Default::default()
    };
    for _ in 0..3 {
        r.broker.request_access(&r.koala, req()).await.unwrap();
    }
    assert!(matches!(
        r.broker.request_access(&r.koala, req()).await,
        Err(Error::RateLimited(_))
    ));
    // Another app is not starved by koalabot.
    r.broker.request_access(&r.evil, req()).await.unwrap();
    // Bad target names are refused before anything else.
    assert!(
        r.broker
            .request_access(
                &r.evil,
                AccessRequest {
                    targets: vec!["../etc".into()],
                    ..req()
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn approval_imports_missing_sites_and_can_narrow() {
    let r = rig().await;
    let p = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![
                    Selector::Site {
                        app: "chrome".into(),
                        site: "github.com".into(),
                    },
                    Selector::Site {
                        app: "chrome".into(),
                        site: "bank.example".into(),
                    },
                ],
                targets: vec!["dev-1".into(), "dev-2".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p.needs_import.len(), 2);
    let grant = r
        .broker
        .approve(
            &r.cua,
            &p.id,
            ApproveOptions {
                targets: Some(vec!["dev-1".into()]),
                duration_secs: Some(10 * 3600 * 24 * 365),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(grant.targets, vec!["dev-1".to_string()]);
    // Lifetimes are clamped (30 days max).
    assert!(grant.not_after_ms - grant.created_ms <= 30 * 24 * 3600 * 1000);
    // The capture was cookie-only and never included passwords.
    let caps = r.backend.captures.lock().unwrap().clone();
    assert!(
        caps.iter()
            .all(|c| c.sites.iter().all(|s| !s.include_passwords))
    );
    assert_eq!(all_items(&r).await.len(), 2);
}

#[tokio::test]
async fn lock_and_unlock() {
    let r = rig().await;
    import_sites(&r, &["github.com"]).await;
    r.broker.lock(&r.cua).await.unwrap();
    assert!(matches!(
        r.broker.list_items(&r.cua, 0, 10).await,
        Err(Error::Locked)
    ));
    assert!(
        r.broker
            .unlock(
                &r.cua,
                UnlockRequest {
                    passphrase: Some("nope nope nope".into()),
                    recovery_key: None
                }
            )
            .await
            .is_err()
    );
    r.broker
        .unlock(
            &r.cua,
            UnlockRequest {
                passphrase: Some("correct horse battery".into()),
                recovery_key: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(all_items(&r).await.len(), 1);
    let v = r.broker.verify_audit(&r.cua).await.unwrap();
    assert!(v.ok());
    assert!(v.entries >= v.unauthenticated);
}

/// Red-team F9: unsigned / foreign callers draw from ONE shared consent budget,
/// so spawning many copies of itself at many paths (each a distinct
/// fingerprint) cannot multiply the pending-prompt allowance.
#[tokio::test]
async fn unsigned_callers_share_one_prompt_budget() {
    let r = rig().await;
    import_sites(&r, &["github.com"]).await;
    let unsigned = |path: &str| {
        let mut c = CallerIdentity::for_tests("x", false);
        c.signing = Signing::Unsigned;
        c.path = Some(path.into());
        c
    };
    let ask = |c: CallerIdentity| {
        let broker = &r.broker;
        async move {
            broker
                .request_access(
                    &c,
                    AccessRequest {
                        selectors: vec![Selector::Site {
                            app: "chrome".into(),
                            site: "github.com".into(),
                        }],
                        targets: vec!["dev-1".into()],
                        ..Default::default()
                    },
                )
                .await
        }
    };
    // Each is a distinct fingerprint (distinct path). Before the fix each got
    // its own 3-pending budget; now they share one.
    assert!(ask(unsigned("/tmp/a")).await.is_ok());
    assert!(ask(unsigned("/tmp/b")).await.is_ok());
    assert!(ask(unsigned("/tmp/c")).await.is_ok());
    let fourth = ask(unsigned("/tmp/d")).await;
    assert!(
        matches!(fourth, Err(Error::RateLimited(_))),
        "a fourth unsigned caller at a new path must hit the shared budget: {fourth:?}"
    );
    // A distinct team-signed identity keeps its own budget (not the shared one).
    assert!(ask(r.koala.clone()).await.is_ok());
}

/// Red-team F17: bulk `ListItems` returns the app, type and lock state only
/// (no domains or keys); names need the browse window, which needs user
/// presence, and a third party never gets them at all.
#[tokio::test]
async fn bulk_enumeration_hides_names_until_the_browse_window_opens() {
    let r = rig().await;
    import_sites(&r, &["github.com"]).await;
    // (the helper browsed to read the names back: close the window)
    r.broker.end_browse(&r.cua).await.unwrap();

    let page = r.broker.list_items(&r.cua, 0, 100).await.unwrap();
    assert!(!page.names_visible);
    assert_eq!(page.total, 1);
    let it = &page.items[0];
    assert_eq!(it.provider_id, "chrome");
    assert_eq!(it.kind, ItemKind::Cookie);
    assert!(
        it.domain.is_none() && it.key.is_empty(),
        "names leaked in the bulk list"
    );
    let json = serde_json::to_string(&page).unwrap();
    assert!(
        !json.contains("github.com") && !json.contains("user_session"),
        "{json}"
    );

    // Browsing needs presence.
    r.presence.set(false);
    assert!(matches!(
        r.broker.browse(&r.cua).await,
        Err(Error::PresenceFailed(_))
    ));
    assert!(
        !r.broker
            .list_items(&r.cua, 0, 100)
            .await
            .unwrap()
            .names_visible
    );
    // Inventory is a name map of the host app too: it needs the window.
    assert!(matches!(
        r.broker.inventory(&r.cua, "chrome", None).await,
        Err(Error::PresenceFailed(_))
    ));
    // With presence the window opens and the names appear.
    r.presence.set(true);
    r.broker.browse(&r.cua).await.unwrap();
    let page = r.broker.list_items(&r.cua, 0, 100).await.unwrap();
    assert!(page.names_visible);
    assert_eq!(page.items[0].domain.as_deref(), Some(".github.com"));
    assert_eq!(page.items[0].key, "user_session");
    // While it is open, the inventory does not ask again.
    r.presence.set(false);
    assert!(r.broker.inventory(&r.cua, "chrome", None).await.is_ok());
    // Locking the vault closes the window.
    r.presence.set(true);
    r.broker.lock(&r.cua).await.unwrap();
    r.broker
        .unlock(
            &r.cua,
            UnlockRequest {
                passphrase: Some("correct horse battery".into()),
                recovery_key: None,
            },
        )
        .await
        .unwrap();
    assert!(
        !r.broker
            .list_items(&r.cua, 0, 100)
            .await
            .unwrap()
            .names_visible
    );

    // A third party cannot browse or list at all.
    assert!(matches!(
        r.broker.browse(&r.koala).await,
        Err(Error::Forbidden(_))
    ));
    assert!(matches!(
        r.broker.list_items(&r.koala, 0, 10).await,
        Err(Error::Forbidden(_))
    ));
}

/// The broker records consent decisions as counts and each teleport with
/// the verified caller kind, and nothing about the items, sites, targets or
/// callers themselves.
#[tokio::test]
async fn broker_records_counts_and_verified_caller_kinds_only() {
    let mut r = rig().await;
    let home = tempfile::tempdir().unwrap();
    let sink = Arc::new(cua_telemetry::sink::MemorySink::new());
    let t = cua_telemetry::Telemetry::builder()
        .env(|_| None)
        .home(home.path())
        .sink(sink.clone())
        .product("daemon", "1.0.0")
        .foreground()
        .build();
    t.acknowledge_notice();
    r.broker = r.broker.with_telemetry(t.clone());
    let items = import_sites(&r, &["github.com"]).await;
    let (token, granted) = grant(&r, &r.koala, "github.com", "dev-1").await;
    assert_eq!(granted, vec![items[0].id.clone()]);
    r.broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token.clone()),
                items: granted.clone(),
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    // Another caller replaying the token is refused.
    let _ = r
        .broker
        .teleport(
            &r.evil,
            TeleportRequest {
                token: Some(token),
                items: granted,
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await;
    t.flush(Duration::from_secs(1));
    let events = sink.events();
    let text = serde_json::to_string(&events).unwrap();
    for leak in [
        "github",
        "dev-1",
        "koalabot",
        "example",
        "fp1-",
        &items[0].id,
    ] {
        assert!(!text.contains(leak), "{leak} leaked: {text}");
    }
    let decisions: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "cua_keyvault_consent")
        .map(|e| e["properties"]["decision"].as_str().unwrap())
        .collect();
    assert_eq!(decisions, ["requested", "approved"]);
    let done: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["event"] == "cua_teleport_completed")
        .map(|e| {
            (
                e["properties"]["caller_kind"].as_str().unwrap(),
                e["properties"]["outcome"].as_str().unwrap(),
            )
        })
        .collect();
    // for_tests identities are team-signed and OS-verified third parties.
    assert_eq!(
        done,
        [("embedded_sdk", "ok"), ("embedded_sdk", "forbidden")]
    );
    assert!(
        events
            .iter()
            .filter(|e| e["event"] == "cua_teleport_completed")
            .all(|e| e["properties"]["app"] == "chrome"
                && e["properties"]["caller_kind_source"] == "broker_verified")
    );
    // Keyvault adoption: the import, then each site sign-in used in a
    // Space (the refused replay is an error), as fixed words only.
    let actions: Vec<(&str, &str, &str)> = events
        .iter()
        .filter(|e| e["event"] == "cua_keyvault_action")
        .map(|e| {
            let p = &e["properties"];
            (
                p["action"].as_str().unwrap(),
                p["method"].as_str().unwrap(),
                p["outcome"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        actions,
        [
            ("import", "none", "ok"),
            ("site_login", "none", "ok"),
            ("site_login", "none", "error"),
        ]
    );
    // Lock, then unlock with the passphrase: the method, never the secret.
    r.broker.lock(&r.cua).await.unwrap();
    let _ = r
        .broker
        .unlock(
            &r.cua,
            UnlockRequest {
                passphrase: Some("not the passphrase".into()),
                recovery_key: None,
            },
        )
        .await;
    t.flush(Duration::from_secs(1));
    let events = sink.events();
    let text = serde_json::to_string(&events).unwrap();
    assert!(!text.contains("not the passphrase"));
    let last: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["event"] == "cua_keyvault_action")
        .skip(3)
        .map(|e| {
            (
                e["properties"]["action"].as_str().unwrap(),
                e["properties"]["method"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(last, [("lock", "none"), ("unlock", "passphrase")]);
}

fn bare_broker(os_protector: bool) -> (tempfile::TempDir, Broker, Arc<FakePresence>) {
    let dir = tempfile::tempdir().unwrap();
    let presence = Arc::new(FakePresence::new(true));
    let broker = Broker::new(
        BrokerConfig {
            dir: dir.path().join("keyvault"),
            // A throwaway path: nothing here may reach the login keychain.
            keychain_path: Some(dir.path().join("never-created.keychain")),
            os_protector,
        },
        Arc::new(FakeBackend::default()),
        presence.clone(),
    )
    .unwrap();
    (dir, broker, presence)
}

/// The reported bug: a daemon that cannot use the OS key store must refuse
/// an OS-protected setup before asking for Touch ID, and say so in status.
#[tokio::test]
async fn os_setup_fails_fast_without_a_presence_prompt_when_unavailable() {
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    // `false`: a debug daemon serving a test identity. `true`: a daemon
    // allowed to use it, but this test binary is not the signed Cua daemon,
    // so it may not create the OS protector (red-team F2) either.
    for configured in [false, true] {
        let (_dir, broker, presence) = bare_broker(configured);
        let st = broker.status(&cua).await;
        assert!(!st.os_protector_available, "configured={configured}");
        assert!(st.passphrase_available);
        assert!(st.unlock_protectors.is_empty());
        let err = broker
            .init(
                &cua,
                InitRequest {
                    os_protector: true,
                    passphrase: None,
                    recovery_key: true,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
        assert!(
            err.to_string().contains("cua keyvault init --passphrase"),
            "{err}"
        );
        assert!(
            presence.asked.lock().unwrap().is_empty(),
            "no Touch ID prompt for a setup that cannot happen"
        );
        assert!(!broker.status(&cua).await.initialized);
    }
}

#[tokio::test]
async fn a_short_passphrase_is_refused_before_presence() {
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    let (_dir, broker, presence) = bare_broker(false);
    let err = broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("too short".into()),
                recovery_key: true,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert!(!err.to_string().contains("too short"), "never echo it");
    assert!(presence.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn passphrase_setup_and_unlock_round_trip() {
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    let koala = CallerIdentity::for_tests("com.example.koalabot", false);
    let (_dir, broker, presence) = bare_broker(false);
    let key = broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("a long passphrase for tests".into()),
                recovery_key: true,
            },
        )
        .await
        .unwrap()
        .expect("recovery key");
    assert_eq!(key.len(), 47);
    assert_eq!(presence.asked.lock().unwrap().len(), 1, "setup asks once");
    let st = broker.status(&cua).await;
    assert!(st.initialized && st.unlocked && !st.os_protector_available);
    assert_eq!(
        st.unlock_protectors,
        [ProtectorKind::Passphrase, ProtectorKind::Recovery]
    );
    // Third parties do not learn how the vault unlocks.
    assert!(broker.status(&koala).await.unlock_protectors.is_empty());

    broker.lock(&cua).await.unwrap();
    assert!(!broker.status(&cua).await.unlocked);
    // Unlocking with the OS key store is refused at once: this vault has
    // none and this daemon cannot use one.
    assert!(matches!(
        broker.unlock(&cua, UnlockRequest::default()).await,
        Err(Error::Unsupported(_))
    ));
    broker
        .unlock(
            &cua,
            UnlockRequest {
                passphrase: Some("a long passphrase for tests".into()),
                recovery_key: None,
            },
        )
        .await
        .unwrap();
    assert!(broker.status(&cua).await.unlocked);
    // The recovery key opens it too.
    broker.lock(&cua).await.unwrap();
    broker
        .unlock(
            &cua,
            UnlockRequest {
                passphrase: None,
                recovery_key: Some(key.to_lowercase()),
            },
        )
        .await
        .unwrap();
    assert!(broker.status(&cua).await.unlocked);
    assert_eq!(
        presence.asked.lock().unwrap().len(),
        1,
        "unlocking with a credential asks for nothing more"
    );
}

#[tokio::test]
async fn a_wrong_passphrase_is_refused_and_audited() {
    let cua = CallerIdentity::for_tests("com.trycua.cua", true);
    let (dir, broker, _presence) = bare_broker(false);
    broker
        .init(
            &cua,
            InitRequest {
                os_protector: false,
                passphrase: Some("a long passphrase for tests".into()),
                recovery_key: false,
            },
        )
        .await
        .unwrap();
    broker.lock(&cua).await.unwrap();
    let err = broker
        .unlock(
            &cua,
            UnlockRequest {
                passphrase: Some("not the passphrase at all".into()),
                recovery_key: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::WrongCredential), "{err:?}");
    assert!(!broker.status(&cua).await.unlocked);
    broker
        .unlock(
            &cua,
            UnlockRequest {
                passphrase: Some("a long passphrase for tests".into()),
                recovery_key: None,
            },
        )
        .await
        .unwrap();
    let tail = broker.audit_tail(&cua, 50).await.unwrap();
    let unlocks: Vec<(&str, &str)> = tail
        .iter()
        .filter(|e| e.event.kind == "vault.unlock")
        .map(|e| (e.event.decision.as_str(), e.event.detail.as_str()))
        .collect();
    assert_eq!(unlocks, [("deny", "Passphrase"), ("ok", "Passphrase")]);
    let raw = std::fs::read_to_string(dir.path().join("keyvault/audit.log")).unwrap();
    assert!(
        !raw.contains("not the passphrase"),
        "never audit the secret"
    );
    assert!(!raw.contains("a long passphrase"), "never audit the secret");
}

#[test]
fn credential_requests_never_print_secrets() {
    let init = InitRequest {
        os_protector: false,
        passphrase: Some("hunter2hunter2".into()),
        recovery_key: true,
    };
    let unlock = UnlockRequest {
        passphrase: Some("hunter2hunter2".into()),
        recovery_key: None,
    };
    let req = cua_keyvault::ipc::Request::Init(init.clone());
    for text in [
        format!("{init:?}"),
        format!("{unlock:?}"),
        format!("{req:?}"),
    ] {
        assert!(!text.contains("hunter2"), "{text}");
    }
}

#[tokio::test]
async fn a_delivery_the_audit_log_cannot_record_is_refused() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let (token, ids) = grant(&r, &r.koala, "github.com", "dev-1").await;
    // Make the log unwritable (a directory in its place refuses appends even
    // for root).
    let log = r._dir.path().join("keyvault/audit.log");
    std::fs::remove_file(&log).unwrap();
    std::fs::create_dir(&log).unwrap();
    let out = r
        .broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items: ids,
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await;
    assert!(matches!(out, Err(Error::Corrupt(_))), "{out:?}");
    // Interactive first-party teleports are refused the same way.
    let out = r
        .broker
        .teleport(
            &r.cua,
            TeleportRequest {
                token: None,
                items: vec![items[0].id.clone()],
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await;
    assert!(matches!(out, Err(Error::Corrupt(_))), "{out:?}");
    assert!(
        r.backend.delivered.lock().unwrap().is_empty(),
        "nothing left the vault without an audit record"
    );
}

#[tokio::test]
async fn deliveries_are_authorized_in_the_log_and_the_kill_switch_wipes_them() {
    let r = rig().await;
    let items = import_sites(&r, &["github.com"]).await;
    let (token, ids) = grant(&r, &r.koala, "github.com", "dev-1").await;
    r.broker
        .teleport(
            &r.koala,
            TeleportRequest {
                token: Some(token),
                items: ids,
                target: "dev-1".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    r.broker
        .teleport(
            &r.cua,
            TeleportRequest {
                token: None,
                items: vec![items[0].id.clone()],
                target: "dev-2".into(),
                include_passwords: false,
                launch: false,
            },
        )
        .await
        .unwrap();
    let tail = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    let authorized: Vec<_> = tail
        .iter()
        .filter(|e| e.event.kind == "teleport.authorize")
        .collect();
    assert_eq!(authorized.len(), 2, "{tail:?}");
    assert!(authorized[0].event.detail.starts_with("grant="));
    assert_eq!(authorized[0].event.target.as_deref(), Some("dev-1"));
    // One switch stops sharing: every live copy is wiped, and audited.
    r.broker.set_disabled(&r.cua, true).await.unwrap();
    let mut wiped: Vec<String> = r
        .backend
        .wiped
        .lock()
        .unwrap()
        .iter()
        .map(|(t, _)| t.clone())
        .collect();
    wiped.sort();
    assert_eq!(wiped, ["dev-1", "dev-2"]);
    assert!(
        r.broker
            .list_deliveries(&r.cua)
            .await
            .unwrap()
            .iter()
            .all(|d| d.wiped)
    );
    let tail = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    assert_eq!(
        tail.iter()
            .filter(|e| e.event.kind == "target.wipe")
            .count(),
        2
    );
    assert!(r.broker.verify_audit(&r.cua).await.unwrap().ok());
}

/// A direct teleport says where it is: reading (when macOS asks for the
/// Keychain), saving only when asked, then the backend's packing, upload
/// and import. Without a sink nothing changes.
#[tokio::test]
async fn import_and_teleport_reports_its_stages_in_order() {
    let r = rig().await;
    let spec = || ImportSpec {
        app: "chrome".into(),
        sites: vec![SiteChoice {
            site: "github.com".into(),
            include_storage: false,
            include_passwords: false,
        }],
        ..Default::default()
    };
    for save in [true, false] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let seen = seen.clone();
            Arc::new(move |s: TeleportStage| seen.lock().unwrap().push(s))
        };
        r.broker
            .import_and_teleport_with_progress(&r.cua, spec(), "dev-1".into(), save, Some(sink))
            .await
            .unwrap();
        let mut want = vec![TeleportStage::Reading];
        if save {
            want.push(TeleportStage::Saving);
        }
        want.extend([
            TeleportStage::Packing,
            TeleportStage::Uploading { done: 0, total: 8 },
            TeleportStage::Uploading { done: 8, total: 8 },
            TeleportStage::Importing,
        ]);
        assert_eq!(*seen.lock().unwrap(), want, "save={save}");
    }
}

/// A direct teleport opens the app in the Space (and says so), unless the
/// caller asked not to; the grant-gated agent path never launches anything.
#[tokio::test]
async fn import_and_teleport_launches_the_app_unless_told_not_to() {
    let r = rig().await;
    let spec = || ImportSpec {
        app: "chrome".into(),
        sites: vec![SiteChoice {
            site: "github.com".into(),
            include_storage: false,
            include_passwords: false,
        }],
        ..Default::default()
    };
    let on = r
        .broker
        .import_and_teleport(&r.cua, spec(), "dev-1".into(), false)
        .await
        .unwrap();
    assert!(on.deliveries[0].launched);
    let off = r
        .broker
        .import_and_teleport_launching(&r.cua, spec(), "dev-1".into(), false, false, None)
        .await
        .unwrap();
    assert!(!off.deliveries[0].launched);
    // Only the first asked for a launching delivery.
    assert_eq!(*r.backend.launches.lock().unwrap(), vec![true]);
}

/// `import_and_teleport` is what a direct (non-MCP) teleport uses instead of
/// exporting and uploading on its own: one first-party call, one presence
/// prompt, capture AND delivery both happen, and (save: true) the item stays
/// in the vault for later reuse -- the "Save to Keyvault" case.
#[tokio::test]
async fn import_and_teleport_asks_presence_once_and_delivers_and_saves() {
    let r = rig().await;
    let spec = ImportSpec {
        app: "chrome".into(),
        sites: vec![SiteChoice {
            site: "github.com".into(),
            include_storage: false,
            include_passwords: false,
        }],
        ..Default::default()
    };
    let before = r.presence.asked.lock().unwrap().len();
    let outcome = r
        .broker
        .import_and_teleport(&r.cua, spec, "dev-1".into(), true)
        .await
        .unwrap();
    let after = r.presence.asked.lock().unwrap().len();
    assert_eq!(after - before, 1, "exactly one presence prompt");
    assert_eq!(outcome.authority, "interactive (user presence)");
    assert_eq!(r.backend.delivered.lock().unwrap().len(), 1);
    assert_eq!(r.backend.delivered.lock().unwrap()[0].0, "dev-1");
    // Saved: the item is still in the vault afterward.
    let items = all_items(&r).await;
    assert_eq!(items.len(), 1, "{items:?}");
}

/// `save: false` still delivers, but forgets the item from the vault
/// afterward WITHOUT wiping the delivery it just made -- "don't save" means
/// "don't keep this for later", not "undo what was just sent".
#[tokio::test]
async fn import_and_teleport_with_save_false_forgets_the_item_but_keeps_the_delivery() {
    let r = rig().await;
    let spec = ImportSpec {
        app: "chrome".into(),
        sites: vec![SiteChoice {
            site: "github.com".into(),
            include_storage: false,
            include_passwords: false,
        }],
        ..Default::default()
    };
    r.broker
        .import_and_teleport(&r.cua, spec, "dev-1".into(), false)
        .await
        .unwrap();
    // The delivery happened...
    assert_eq!(r.backend.delivered.lock().unwrap().len(), 1);
    // ...and was never wiped (a save:false teleport is not an undo).
    assert!(r.backend.wiped.lock().unwrap().is_empty());
    // ...but the vault no longer holds the item.
    assert!(all_items(&r).await.is_empty());
    // The audit still shows the whole story: capture, authorize, deliver,
    // and the forgetting.
    let tail = r.broker.audit_tail(&r.cua, 100).await.unwrap();
    assert!(tail.iter().any(
        |e| e.event.kind == "teleport.authorize" && e.event.target.as_deref() == Some("dev-1")
    ));
    assert!(
        tail.iter().any(|e| e.event.kind == "teleport.deliver"
            && e.event.detail == "interactive (user presence)")
    );
    assert!(
        tail.iter()
            .any(|e| e.event.kind == "item.delete" && e.event.detail.contains("not saved"))
    );
}

/// A third party (not the signed Cua app/CLI) can never reach
/// `import_and_teleport`: nothing is captured or delivered.
#[tokio::test]
async fn import_and_teleport_fails_closed_for_third_parties() {
    let r = rig().await;
    let spec = ImportSpec {
        app: "chrome".into(),
        sites: vec![SiteChoice {
            site: "github.com".into(),
            include_storage: false,
            include_passwords: false,
        }],
        ..Default::default()
    };
    let err = r
        .broker
        .import_and_teleport(&r.koala, spec, "dev-1".into(), true)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Forbidden(_)), "{err:?}");
    assert!(r.backend.captures.lock().unwrap().is_empty());
    assert!(r.backend.delivered.lock().unwrap().is_empty());
}

/// The kill switch refuses `import_and_teleport` before the backend is ever
/// touched, same as the separate `import`/`teleport` calls.
#[tokio::test]
async fn import_and_teleport_respects_the_kill_switch() {
    let r = rig().await;
    r.broker.set_disabled(&r.cua, true).await.unwrap();
    let spec = ImportSpec {
        app: "chrome".into(),
        sites: vec![SiteChoice {
            site: "github.com".into(),
            include_storage: false,
            include_passwords: false,
        }],
        ..Default::default()
    };
    let err = r
        .broker
        .import_and_teleport(&r.cua, spec, "dev-1".into(), true)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Disabled), "{err:?}");
    assert!(r.backend.captures.lock().unwrap().is_empty());
}

/// An empty selection (e.g. `ImportSpec::paths` given as `Some(vec![])`,
/// which the fake backend below turns into no captured items) is refused
/// rather than "succeeding" at teleporting nothing.
#[tokio::test]
async fn import_and_teleport_refuses_when_nothing_was_captured() {
    let r = rig().await;
    let spec = ImportSpec {
        app: "chrome".into(),
        whole_app: false,
        sites: vec![],
        ..Default::default()
    };
    let err = r
        .broker
        .import_and_teleport(&r.cua, spec, "dev-1".into(), true)
        .await
        .unwrap_err();
    // `import_confirmed` itself refuses an empty selection before capture.
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert!(r.backend.delivered.lock().unwrap().is_empty());
}

/// A whole-app (`Selector::App`) request only ever captures the provider's
/// existing default (login-only) selection UNLESS the human's own approval
/// widens it with `ApproveOptions::paths` -- the requester's own `include`
/// (an MCP/automation caller, never authoritative, red-team E2) never
/// reaches the capture on its own.
#[tokio::test]
async fn approve_options_paths_widens_a_whole_app_capture_the_requester_cannot() {
    let r = rig().await;
    let pending = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::App {
                    app: "chrome".into(),
                }],
                targets: vec!["dev-1".into()],
                uses: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // The human approves, explicitly widening to a specific path selection
    // (what a consent screen's checked items would produce) -- this is the
    // ONLY way that selection reaches the capture for a whole-app item.
    r.broker
        .approve(
            &r.cua,
            &pending.id,
            ApproveOptions {
                paths: Some(vec!["cookies.sqlite".into(), "prefs.js".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let captures = r.backend.captures.lock().unwrap();
    let spec = captures
        .iter()
        .find(|s| s.whole_app)
        .expect("a whole-app capture happened");
    assert_eq!(
        spec.paths.as_deref(),
        Some(&["cookies.sqlite".to_string(), "prefs.js".to_string()][..])
    );
}

/// Without that widening, a whole-app capture's `paths` stays `None` (the
/// existing login-only default), even though the request named an app.
#[tokio::test]
async fn approve_without_paths_leaves_the_whole_app_default_selection_alone() {
    let r = rig().await;
    let pending = r
        .broker
        .request_access(
            &r.koala,
            AccessRequest {
                selectors: vec![Selector::App {
                    app: "chrome".into(),
                }],
                targets: vec!["dev-1".into()],
                uses: Some(0),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    r.broker
        .approve(&r.cua, &pending.id, ApproveOptions::default())
        .await
        .unwrap();
    let captures = r.backend.captures.lock().unwrap();
    let spec = captures
        .iter()
        .find(|s| s.whole_app)
        .expect("a whole-app capture happened");
    assert_eq!(spec.paths, None);
}
