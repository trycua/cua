// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The core's Keyvault mirror types decode the broker's real serde output
//! (and the flows' fixtures), so the shells and the broker cannot drift.

use cua_keyvault::audit::AuditEvent;
use cua_keyvault::broker::Status;
use cua_keyvault::caller::{CallerIdentity, Signing};
use cua_keyvault::model::{
    Delivery, Grant, ItemKind, ItemMeta, ItemPolicy, ItemSummary, RuleCaller, UnattendedRule,
};
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
    let item = ItemMeta {
        id: "i1".into(),
        kind: ItemKind::AppSession,
        label: "Slack (whole app session)".into(),
        provider_id: "slack".into(),
        app_display: "Slack".into(),
        site: None,
        account: Some("Example".into()),
        source: String::new(),
        summary: ItemSummary::default(),
        warnings: vec![],
        identity_provider: false,
        policy: ItemPolicy::default(),
        created_ms: 1,
        updated_ms: 2,
        rev: 3,
        record_digest: "00".into(),
    };
    let k: KvItem = round(&item);
    assert_eq!(k.kind, "app_session");
    assert_eq!(k.account.as_deref(), Some("Example"));

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
        os_protector_available: false,
        passphrase_available: true,
        unlock_protectors: vec![
            cua_keyvault::protector::ProtectorKind::Passphrase,
            cua_keyvault::protector::ProtectorKind::Recovery,
        ],
    };
    let kv: KvStatus = round(&st);
    assert_eq!(kv.unlock_protectors, ["passphrase", "recovery"]);
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
    assert_eq!(o.items.len(), 4);
    assert_eq!(o.pending.len(), 2);
}

#[test]
fn the_passphrase_minimum_is_the_brokers() {
    assert_eq!(
        cua_spaces_app_core::keyvault::credential::MIN_PASSPHRASE_CHARS as usize,
        cua_keyvault::protector::MIN_PASSPHRASE_CHARS
    );
}
