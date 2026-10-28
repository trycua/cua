// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The registry contract, run against the in-memory store and, when
//! `CUA_OIDC_TEST_DATABASE_URL` names a scratch Postgres database, against
//! the Postgres store (its migrations included).

use std::collections::BTreeMap;

use cua_oidc::model::{
    AuditRecord, Auth, Cloud, Connection, CreatedBy, Limits, SealedToken, Status,
};
use cua_oidc::registry::{Ledger, MemoryLedger, MemoryRegistry, Registry, StoreError};

fn connection(account: &str, cloud: Cloud, primary: &str) -> Connection {
    let (_, key) = cloud.identifier_keys();
    let mut identifiers = BTreeMap::new();
    identifiers.insert(key.to_owned(), primary.to_owned());
    let now = cua_oidc::now_secs();
    Connection {
        id: format!("conn_{}", hex::encode(rand::random::<[u8; 8]>())),
        account: account.to_owned(),
        cloud,
        auth: cloud.auth(),
        identifiers,
        region: Some("us-west-2".into()),
        limits: Limits {
            budget_usd_month: Some(25.0),
            ..Limits::default()
        },
        pending_limits: None,
        pending_agent: None,
        status: Status::Connected,
        created_by: CreatedBy {
            user: "u".into(),
            device: "dev_0123456789abcdef01234567".into(),
            agent: None,
        },
        approved_by: None,
        sealed_token: None,
        created_at: now,
        updated_at: now,
        version: 1,
    }
}

async fn contract(r: &dyn Registry) {
    let acct = format!("t{}", hex::encode(rand::random::<[u8; 6]>()))[..12].to_owned();
    let other = format!("u{}", hex::encode(rand::random::<[u8; 6]>()))[..12].to_owned();

    let mut aws = connection(&acct, Cloud::Aws, "123456789012");
    r.create(&aws).await.unwrap();
    // One live connection per cloud and identifier.
    let dup = connection(&acct, Cloud::Aws, "123456789012");
    assert_eq!(
        r.create(&dup).await,
        Err(StoreError::Exists(aws.id.clone()))
    );
    // Another account may link the same cloud account.
    r.create(&connection(&other, Cloud::Aws, "123456789012"))
        .await
        .unwrap();

    let mut sealed = connection(&acct, Cloud::Hetzner, "proj");
    assert_eq!(sealed.auth, Auth::Sealed);
    sealed.sealed_token = Some(SealedToken {
        ciphertext: vec![0, 1, 2, 255],
        generation: 7,
    });
    r.create(&sealed).await.unwrap();

    let listed = r.list(&acct).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(r.get(&acct, &sealed.id).await.unwrap().unwrap(), sealed);
    assert_eq!(r.get(&other, &aws.id).await.unwrap(), None);

    // Optimistic updates.
    let stale = aws.clone();
    aws.status = Status::Revoked;
    aws.version += 1;
    r.update(&aws).await.unwrap();
    let mut late = stale;
    late.version += 1;
    assert_eq!(r.update(&late).await, Err(StoreError::Conflict));
    assert_eq!(
        r.get(&acct, &aws.id).await.unwrap().unwrap().status,
        Status::Revoked
    );
    // A revoked connection frees the identifier.
    r.create(&connection(&acct, Cloud::Aws, "123456789012"))
        .await
        .unwrap();

    assert!(r.delete(&acct, &sealed.id).await.unwrap());
    assert!(!r.delete(&acct, &sealed.id).await.unwrap());
    assert!(!r.delete(&other, &aws.id).await.unwrap());

    // Audit: newest first, per account and connection.
    for kind in ["token_issued", "token_refused"] {
        let mut rec = AuditRecord::new(&acct, kind);
        rec.connection = Some(aws.id.clone());
        rec.jti = Some("j".into());
        r.append_audit(&rec).await.unwrap();
    }
    r.append_audit(&AuditRecord::new(&other, "token_issued"))
        .await
        .unwrap();
    let rows = r.audit(&acct, None, 10).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].kind, "token_refused");
    assert_eq!(r.audit(&acct, Some(&aws.id), 1).await.unwrap().len(), 1);
    assert!(r
        .audit(&acct, Some("conn_none"), 10)
        .await
        .unwrap()
        .is_empty());

    // Replay cache.
    let key = format!("dev_x:{}", hex::encode(rand::random::<[u8; 8]>()));
    let exp = cua_oidc::now_secs() + 120;
    assert!(r.claim_proof(&key, exp).await.unwrap());
    assert!(!r.claim_proof(&key, exp).await.unwrap());
    // An expired entry can be claimed again (it could no longer verify).
    let old = format!("dev_x:{}", hex::encode(rand::random::<[u8; 8]>()));
    assert!(r.claim_proof(&old, cua_oidc::now_secs() - 1).await.unwrap());
    assert!(r.claim_proof(&old, exp).await.unwrap());
    r.ready().await.unwrap();
}

/// The spend ledger: per device, the latest report wins; this month only;
/// per account and connection.
async fn ledger_contract(l: &dyn Ledger) {
    let acct = format!("t{}", hex::encode(rand::random::<[u8; 6]>()))[..12].to_owned();
    let month = cua_oidc::this_month();
    assert_eq!(l.spent_month(&acct, "conn_a").await.unwrap(), 0.0);
    l.record_spend(&acct, "conn_a", "dev_1", &month, 10.0)
        .await
        .unwrap();
    l.record_spend(&acct, "conn_a", "dev_1", &month, 12.5)
        .await
        .unwrap();
    l.record_spend(&acct, "conn_a", "dev_2", &month, 7.5)
        .await
        .unwrap();
    l.record_spend(&acct, "conn_a", "dev_2", "2001-01", 1000.0)
        .await
        .unwrap();
    l.record_spend(&acct, "conn_b", "dev_1", &month, 3.0)
        .await
        .unwrap();
    l.record_spend("someoneelse", "conn_a", "dev_1", &month, 3.0)
        .await
        .unwrap();
    assert_eq!(l.spent_month(&acct, "conn_a").await.unwrap(), 20.0);
    assert_eq!(l.spent_month(&acct, "conn_b").await.unwrap(), 3.0);
}

#[tokio::test]
async fn memory_ledger_contract() {
    ledger_contract(&MemoryLedger::new()).await;
}

#[tokio::test]
async fn memory_registry_contract() {
    contract(&MemoryRegistry::new()).await;
}

#[tokio::test]
async fn postgres_registry_contract() {
    let Ok(url) = std::env::var("CUA_OIDC_TEST_DATABASE_URL") else {
        eprintln!("skipped: set CUA_OIDC_TEST_DATABASE_URL to a scratch Postgres database");
        return;
    };
    let pg = cua_oidc::pg::PgRegistry::connect(&url, None).await.unwrap();
    contract(&pg).await;
    ledger_contract(&pg).await;
    // Migrating again is a no-op.
    let again = cua_oidc::pg::PgRegistry::connect(&url, None).await.unwrap();
    again.ready().await.unwrap();
}
