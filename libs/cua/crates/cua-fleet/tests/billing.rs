//! `FleetClient` billing calls against the in-memory fake Fleet.

use cua_fleet::{Error, testing::FakeFleet};
use serde_json::json;

fn status() -> serde_json::Value {
    json!({
        "billing_enabled": true, "payment_method_present": false, "card": null,
        "plan": "none", "payg_available": true,
        "credit": {"balance_usd_cents": 1000, "signup_grant_usd_cents": 1000, "signup_grant_unused": true},
        "billing_url": "https://run.cua.ai/billing"
    })
}

#[tokio::test]
async fn a_fleet_without_billing_reads_as_disabled() {
    let fake = FakeFleet::new();
    let s = fake.client().billing_status().await.unwrap();
    assert!(!s.billing_enabled && !s.payment_method_present && s.credit.is_none());
}

#[tokio::test]
async fn the_status_carries_the_signup_credit_and_the_billing_page() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().billing = Some(status());
    let fleet = fake.client();
    let s = fleet.billing_status().await.unwrap();
    let credit = s.credit.expect("credit");
    assert_eq!(
        (credit.balance_usd_cents, credit.signup_grant_usd_cents),
        (1000, 1000)
    );
    assert!(credit.signup_grant_unused);
    assert_eq!(s.billing_url.as_deref(), Some("https://run.cua.ai/billing"));
    fake.complete_checkout("visa", "4242");
    let s = fleet.billing_status().await.unwrap();
    assert_eq!(s.card.map(|c| c.last4), Some("4242".into()));
}

#[tokio::test]
#[allow(deprecated)]
async fn out_of_credit_claims_are_a_typed_error_with_the_billing_page() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().credit_exhausted = Some("https://run.cua.ai/billing".into());
    let fleet = fake.client();
    let pool = fleet
        .apply_pool(&cua_fleet::PoolSpec::new(
            "credit-pool",
            "ghcr.io/trycua/linux:24.04-disk",
        ))
        .await
        .unwrap();
    let e = fleet
        .claim(&pool.pool, cua_fleet::ClaimOptions::default())
        .await
        .unwrap_err();
    match &e {
        Error::CreditExhausted {
            message,
            billing_url,
        } => {
            assert_eq!(message, "You're out of Cua Cloud credit.");
            assert_eq!(billing_url, "https://run.cua.ai/billing");
        }
        other => panic!("expected CreditExhausted, got {other:?}"),
    }
    assert_eq!(
        e.to_string(),
        "You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing"
    );
}
