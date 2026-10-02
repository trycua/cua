// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The core's Keyvault mirror types decode the broker's real serde output
//! (and the flows' fixtures), so the shells and the broker cannot drift.

use cua_keyvault::audit::AuditEvent;
use cua_keyvault::broker::Status;
use cua_keyvault::caller::{CallerIdentity, Signing};
use cua_keyvault::model::{Delivery, Grant, ItemKind, ItemMeta, RuleCaller, UnattendedRule};
use cua_spaces_app_core::keyvault::{
    KeyvaultOverview, KvCaller, KvDelivery, KvGrant, KvItem, KvRule, KvSigning, KvStatus,
};
use std::collections::BTreeMap;

fn round<T: serde::Serialize, U: serde::de::DeserializeOwned>(v: &T) -> U {
    serde_json::from_value(serde_json::to_value(v).unwrap())
        .expect("mirror decodes the broker type")
}

#[test]
fn items_callers_grants_rules_deliveries_status() {
    let mut item = ItemMeta::draft(
        ItemKind::Cookie,
        "chrome",
        "Google Chrome",
        Some(".github.com"),
        "user_session",
    );
    item.id = "i1".into();
    item.path = Some("/".into());
    item.expires_ms = Some(1_900_000_000_000);
    item.bytes = 12;
    item.policy.unattended = true;
    item.created_ms = 1;
    item.updated_ms = 2;
    item.rev = 3;
    item.record_digest = "00".into();
    let k: KvItem = round(&item);
    assert_eq!(k.kind, "cookie");
    assert_eq!(k.domain.as_deref(), Some(".github.com"));
    assert_eq!(k.key, "user_session");
    assert_eq!(k.path.as_deref(), Some("/"));
    assert!(k.policy.unattended);
    // Every kind's wire tag is one the list knows.
    for kind in [
        ItemKind::Cookie,
        ItemKind::LocalStorage,
        ItemKind::Password,
        ItemKind::File,
    ] {
        let tag = serde_json::to_value(kind).unwrap();
        assert_eq!(tag, kind.tag(), "the serde tag is the stable tag");
        assert_eq!(
            cua_spaces_app_core::keyvault::vault::KvKind::from_wire(kind.tag()).label(),
            kind.label()
        );
    }
    // A redacted (no browse window) item still decodes.
    let r: KvItem = round(&item.redacted());
    assert!(r.domain.is_none() && r.key.is_empty());
    let page = cua_keyvault::broker::ItemPage {
        items: vec![item.clone()],
        total: 1,
        names_visible: true,
    };
    let _: Vec<KvItem> = round(&page.items);

    for (signing, want) in [
        (
            Signing::Signed {
                team_id: "T".into(),
                identifier: "com.x".into(),
                cdhash: "c".into(),
            },
            KvSigning::Signed {
                team_id: "T".into(),
                identifier: "com.x".into(),
                cdhash: "c".into(),
            },
        ),
        (
            Signing::AdHoc {
                identifier: "com.x".into(),
                cdhash: "c".into(),
            },
            KvSigning::AdHoc {
                identifier: "com.x".into(),
                cdhash: "c".into(),
            },
        ),
        (Signing::Unsigned, KvSigning::Unsigned),
    ] {
        let caller = CallerIdentity {
            pid: 1,
            uid: 501,
            path: None,
            signing,
            first_party: false,
            os_verified: true,
            launched_by: None,
            verified_name: None,
        };
        let c: KvCaller = round(&caller);
        assert_eq!(c.signing, want);
    }

    let grant = Grant {
        id: "g".into(),
        request_id: "r".into(),
        caller_fp: "fp".into(),
        caller_display: "x".into(),
        items: vec!["i1".into()],
        targets: vec!["t".into()],
        target_ids: BTreeMap::new(),
        actions: vec![],
        created_ms: 1,
        not_after_ms: 2,
        uses_left: Some(1),
        revoked: false,
        agent: Some("ada".into()),
        unattended: false,
    };
    let g: KvGrant = round(&grant);
    assert_eq!(g.agent.as_deref(), Some("ada"));
    let rule = UnattendedRule {
        id: "r".into(),
        items: vec![],
        targets: vec!["*".into()],
        target_ids: BTreeMap::new(),
        callers: vec![RuleCaller {
            fp: "fp".into(),
            display: "d".into(),
        }],
        created_ms: 1,
        not_after_ms: 2,
        enabled: true,
        note: String::new(),
    };
    let _: KvRule = round(&rule);
    let d = Delivery {
        import_id: "i".into(),
        target: "t".into(),
        provider_id: "p".into(),
        items: vec![],
        caller_fp: "fp".into(),
        delivered_ms: 1,
        expires_ms: 2,
        wiped: false,
    };
    let _: KvDelivery = round(&d);
    let st = Status {
        version: "0".into(),
        initialized: true,
        unlocked: true,
        disabled: false,
        caller_first_party: true,
        caller_display: "Cua".into(),
        items: 1,
        pending: 0,
        unlock_policy: None,
        auto_wipe: Some(true),
        os_protector_available: false,
        passphrase_available: true,
        unlock_protectors: vec![
            cua_keyvault::protector::ProtectorKind::Passphrase,
            cua_keyvault::protector::ProtectorKind::Recovery,
        ],
        browse_until_ms: Some(9),
        skip_unlock_prompt: Some(true),
        reset_notice: Some("set aside".into()),
    };
    let kv: KvStatus = round(&st);
    assert_eq!(kv.unlock_protectors, ["passphrase", "recovery"]);
    assert_eq!(kv.auto_wipe, Some(true));
    assert_eq!(kv.browse_until_ms, Some(9));
    assert_eq!(kv.skip_unlock_prompt, Some(true));
    assert_eq!(kv.reset_notice.as_deref(), Some("set aside"));
    let ev = AuditEvent {
        kind: "consent.allow".into(),
        actor: "Cua".into(),
        caller_fp: "cua".into(),
        item: Some("i1".into()),
        target: None,
        decision: "allow".into(),
        detail: String::new(),
    };
    let _ = serde_json::to_value(&ev).unwrap();
}

#[test]
fn the_flow_fixture_is_a_valid_overview() {
    let flow: serde_json::Value =
        serde_json::from_str(include_str!("../parity/keyvault-approve-deny.json")).unwrap();
    let o: KeyvaultOverview = serde_json::from_value(flow["overview"].clone()).unwrap();
    assert_eq!(o.items.len(), 8);
    assert_eq!(o.pending.len(), 2);
}

#[test]
fn the_passphrase_minimum_is_the_brokers() {
    assert_eq!(
        cua_spaces_app_core::keyvault::credential::MIN_PASSPHRASE_CHARS as usize,
        cua_keyvault::protector::MIN_PASSPHRASE_CHARS
    );
}

#[test]
fn the_core_groups_sites_the_way_the_broker_does() {
    for host in [
        ".github.com",
        "api.github.com",
        "https://gist.github.com:8443/x",
        "www.bbc.co.uk",
        "localhost",
        "127.0.0.1",
        "http://127.0.0.1:8000",
        "accounts.google.com",
    ] {
        assert_eq!(
            cua_spaces_app_core::keyvault::vault::site_of(host),
            cua_keyvault::record::site_of(host),
            "{host}"
        );
    }
}

#[test]
fn the_inventory_mirror_decodes_the_brokers() {
    let inv = cua_keyvault::broker::Inventory {
        provider_id: "chrome".into(),
        app_display: "Google Chrome".into(),
        profile: None,
        domains: vec![cua_keyvault::broker::DomainInventory {
            domain: "github.com".into(),
            cookies: 4,
            session_cookies: 1,
            local_storage: 2,
            passwords: 1,
            signin: true,
            identity_provider: false,
            unavailable: 3,
            unavailable_reason: "app-bound".into(),
        }],
        notes: vec!["n".into()],
    };
    let k: cua_spaces_app_core::keyvault::KvInventory = round(&inv);
    assert_eq!(k.domains[0].cookies, 4);
    assert_eq!(k.domains[0].unavailable, 3);
    assert_eq!(k.domains[0].unavailable_reason, "app-bound");
    assert!(k.domains[0].signin);
}
