//! Cua Cloud billing through the exported SDK over `FakeFleet`: the status
//! the Spaces apps' Settings show, and a cloud create refused for want of
//! credit as a typed `CloudCreditExhausted` (nothing reaches Stripe).

use cua_daemon::{Runtime, RuntimeConfig};
use cua_fleet::testing::FakeFleet;
use cua_sdk::{Cua, CuaError, SandboxCreateOptions};
use serde_json::json;
use std::time::Duration;

fn cua(fake: &FakeFleet, dirs: &tempfile::TempDir) -> std::sync::Arc<Cua> {
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        fleet_client: Some(fake.client()),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    Cua::from_runtime(runtime)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_status_carries_the_credit_and_the_billing_page() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().billing = Some(json!({
        "billing_enabled": true, "payment_method_present": false, "card": null,
        "plan": "none", "payg_available": true,
        "credit": {"balance_usd_cents": 742, "signup_grant_usd_cents": 100, "signup_grant_unused": false},
        "billing_url": "https://run.cua.ai/billing"
    }));
    let dirs = tempfile::tempdir().unwrap();
    let s = cua(&fake, &dirs)
        .fleet()
        .unwrap()
        .billing_status()
        .await
        .unwrap();
    assert_eq!(s.credit.as_ref().map(|c| c.balance_usd_cents), Some(742));
    assert_eq!(s.billing_url.as_deref(), Some("https://run.cua.ai/billing"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cloud_create_out_of_credit_is_a_typed_error_with_the_billing_page() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().credit_exhausted = Some("https://run.cua.ai/billing".into());
    let dirs = tempfile::tempdir().unwrap();
    let cua = cua(&fake, &dirs);
    let mut o = SandboxCreateOptions::new("cloud", "ghcr.io/trycua/linux:24.04");
    o.kind = Some("vm".into());
    match cua.sandboxes().create(o).await {
        Err(CuaError::CloudCreditExhausted(m)) => {
            assert_eq!(
                m,
                "You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing"
            );
        }
        Err(e) => panic!("expected CloudCreditExhausted, got {e:?}"),
        Ok(_) => panic!("expected CloudCreditExhausted, got a sandbox"),
    }
}
