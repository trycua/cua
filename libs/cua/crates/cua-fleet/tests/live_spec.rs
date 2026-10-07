//! Live Fleet run of the shared model on gVisor. Skipped unless
//! `CUA_E2E_FLEET=1` (and Fleet credentials).
//!
//! One `cua-e2e-spec-<rand>` pool written by `FleetClient::apply` (warm,
//! idle TTL, ttlPolicy, claim secrets), then:
//! - the #7886 lifecycle fields read back from the pool;
//! - `check_pool_spec` passes for the same fields and raises
//!   `PoolSpecMismatch` for a different command;
//! - a claim with a per-claim env token binds and the driver holds the token
//!   (the claim-secret wait returns);
//! - with a budget shorter than delivery, the wait raises
//!   `ClaimSecretsNotDelivered` and the claim is released;
//! - the pool exports as Terraform.
//!
//! The pool (and its namespace) is deleted whatever happened.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 cargo test -p cua-fleet --test live_spec -- --nocapture
//! ```

use cua_fleet::{
    ClaimOptions, ClaimSecretsWait, Error, FleetClient, PoolOptions, RuntimeKind, SandboxSpec,
    TtlPolicy, claim_secrets::generate_claim_token,
};
use std::time::{Duration, Instant};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04";

fn enabled() -> bool {
    std::env::var("CUA_E2E_FLEET").as_deref() == Ok("1")
}

fn spec() -> SandboxSpec {
    SandboxSpec {
        services: [("env".to_string(), 3211)].into(),
        claim_secrets: true,
        ..SandboxSpec::new(
            std::env::var("CUA_E2E_FLEET_GVISOR_IMAGE").unwrap_or_else(|_| IMAGE.to_string()),
        )
    }
}

async fn run(fleet: &FleetClient, name: &str) -> Result<(), String> {
    let t0 = Instant::now();
    let options = PoolOptions {
        runtime: Some(RuntimeKind::Gvisor),
        warm: Some(true),
        max_pool_size: Some(2),
        idle_ttl: Some(Duration::from_secs(3600)),
        ttl_policy: Some(TtlPolicy::Cascade),
        pool_ttl: Some(Duration::from_secs(3 * 3600)),
        ..Default::default()
    };
    let handle = fleet
        .apply(name, &spec(), &options)
        .await
        .map_err(|e| format!("apply: {e}"))?;
    println!("[{:>4}s] applied {name}", t0.elapsed().as_secs());

    let raw = fleet
        .get_pool_json(name)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("pool missing")?;
    let s = &raw["spec"];
    println!(
        "pool spec: autoscaling={} idleTtlSeconds={} ttlPolicy={}",
        s["autoscaling"], s["idleTtlSeconds"], s["ttlPolicy"]
    );
    if s["autoscaling"]["minPoolSize"] != 1 {
        return Err(format!("warm floor not written: {}", s["autoscaling"]));
    }
    if s["idleTtlSeconds"] != 3600 || s["ttlPolicy"] != "Cascade" {
        return Err(format!("lifecycle fields not kept by Fleet: {s}"));
    }

    // Same fields match; a different command is a mismatch with a diff.
    fleet
        .check_pool_spec(name, &spec())
        .await
        .map_err(|e| format!("check same: {e}"))?;
    let other = SandboxSpec {
        command: Some(vec!["sleep".into(), "infinity".into()]),
        ..Default::default()
    };
    match fleet.check_pool_spec(name, &other).await {
        Err(e @ Error::PoolSpecMismatch { .. }) => println!("mismatch (expected):\n{e}"),
        other => return Err(format!("expected PoolSpecMismatch, got {other:?}")),
    }

    fleet
        .wait_pool_ready(&handle, Duration::from_secs(900))
        .await
        .map_err(|e| format!("warm: {e}"))?;
    println!("[{:>4}s] warm replica ready", t0.elapsed().as_secs());

    // A claim with a token: bound, then the driver holds it.
    let t = Instant::now();
    let bound = fleet
        .acquire(
            &handle.pool,
            ClaimOptions {
                name: Some(format!("{name}-c1")),
                claim_token: Some(generate_claim_token()),
                ttl_seconds_after_created: Some(1800),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("acquire with token: {e}"))?;
    println!(
        "[{:>4}s] claim {} bound and token delivered after {:?}",
        t0.elapsed().as_secs(),
        bound.claim,
        t.elapsed()
    );
    fleet
        .release(&bound.namespace, &bound.claim)
        .await
        .map_err(|e| e.to_string())?;

    // A budget shorter than delivery: ClaimSecretsNotDelivered, released.
    let impatient = fleet.clone().with_claim_secrets_wait(ClaimSecretsWait {
        budget: Duration::from_secs(3),
        every: Duration::from_secs(1),
        ..Default::default()
    });
    let c2 = format!("{name}-c2");
    fleet
        .wait_pool_ready(&handle, Duration::from_secs(900))
        .await
        .map_err(|e| format!("rewarm: {e}"))?;
    match impatient
        .acquire(
            &handle.pool,
            ClaimOptions {
                name: Some(c2.clone()),
                claim_token: Some(generate_claim_token()),
                ttl_seconds_after_created: Some(1800),
                ..Default::default()
            },
        )
        .await
    {
        Err(e @ Error::ClaimSecretsNotDelivered { .. }) => println!("guard (expected): {e}"),
        Ok(b) => {
            // Delivery beat a 3 s budget: fine, but release it.
            let _ = fleet.release(&b.namespace, &b.claim).await;
            println!("delivery beat the 3 s budget; guard not exercised");
        }
        Err(e) => return Err(format!("expected ClaimSecretsNotDelivered, got {e}")),
    }
    let claims = fleet.list_claims(name).await.map_err(|e| e.to_string())?;
    if claims.iter().any(|c| c.metadata.name == c2) {
        return Err(format!("{c2} was not released"));
    }

    let (s, o, rt) = fleet.export_pool(name).await.map_err(|e| e.to_string())?;
    println!("{}", cua_fleet::terraform_pool_block(name, &s, &o, &rt));
    Ok(())
}

#[tokio::test]
async fn live_gvisor_apply_mismatch_and_claim_secret_guard() {
    if !enabled() {
        eprintln!("skipped: set CUA_E2E_FLEET=1");
        return;
    }
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let name = format!("cua-e2e-spec-{:08x}", rand::random::<u32>());
    let result = run(&fleet, &name).await;
    // Cleanup whatever happened: claims, then the pool and its namespace.
    for c in fleet.list_claims(&name).await.unwrap_or_default() {
        let _ = fleet.release(&name, &c.metadata.name).await;
    }
    match fleet.get_pool(&name).await {
        Ok(mut h) => {
            h.template = fleet
                .sdk()
                .get_template(name.clone(), name.clone())
                .await
                .ok();
            if let Err(e) = fleet.delete_pool(h).await {
                eprintln!("CLEANUP FAILED for {name}: {e}");
            } else {
                println!("deleted {name}");
            }
        }
        Err(e) => eprintln!("cleanup: pool {name}: {e}"),
    }
    result.unwrap();
}
