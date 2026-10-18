//! Live image doctor on Fleet: `cua-spacesd doctor` in a Fleet sandbox,
//! driven through the gateway. Skipped unless `CUA_E2E_FLEET=1` and
//! `CUA_E2E_FLEET_DOCTOR_IMAGE` are set (and Fleet credentials).
//!
//! One `cua-e2e-doctor-<rand>` pool with per-claim Secrets (the #7885 pod
//! Secret on gVisor, the #7888 virtio-fs bridge on KubeVirt), one claim with
//! a fresh claim token, then `SystemService.Diagnose` over the gateway
//! (Fleet bearer in `authorization`, the claim token in
//! `x-cua-env-authorization`): the guest runs its own doctor with effects on
//! its virtual display only. The report is written to
//! `CUA_E2E_FLEET_DOCTOR_OUT` (default `./fleet-doctor-report.json`). The pool
//! and its namespace are deleted whatever happened.
//!
//! This is the claim path the canonical linux image needs on
//! Fleet: without a claim Secret the image mints its own local token, so a
//! claim on a pool without `claim_secrets` cannot be reached.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 CUA_E2E_FLEET_DOCTOR_IMAGE=ghcr.io/trycua/linux@sha256:... \
//!   CUA_E2E_FLEET_DOCTOR_RUNTIME=gvisor|kubevirt CUA_E2E_FLEET_DOCTOR_STRICT=1 \
//!   cargo test -p cua-fleet --test live_image_doctor -- --nocapture
//! ```
//!
//! `CUA_E2E_FLEET_DOCTOR_GUEST_ENV=K=V,...` exports guest environment around
//! the image's entrypoint (never a token), e.g. `CUA_ENV_RUNTIME=gvisor`.

use std::time::{Duration, Instant, SystemTime};

use cua_fleet::{
    ClaimOptions, FleetClient, PoolOptions, RuntimeKind, SandboxSpec,
    claim_secrets::generate_claim_token,
};
use cua_spacesd_client::diagnose::Status;
use cua_spacesd_client::{SpacesdClient, pb};

fn image() -> Option<String> {
    if std::env::var("CUA_E2E_FLEET").as_deref() != Ok("1") {
        return None;
    }
    std::env::var("CUA_E2E_FLEET_DOCTOR_IMAGE")
        .ok()
        .filter(|s| !s.is_empty())
}

/// `sh -c 'export K=V ...; exec <entrypoint>'` for
/// `CUA_E2E_FLEET_DOCTOR_GUEST_ENV`, or `None` (the image's own command).
fn guest_env_command() -> Option<Vec<String>> {
    let raw = std::env::var("CUA_E2E_FLEET_DOCTOR_GUEST_ENV").unwrap_or_default();
    let exports: Vec<String> = raw
        .split(',')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .filter(|(k, v)| {
            !k.is_empty()
                && !k.contains("TOKEN")
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-/:".contains(c))
        })
        .map(|(k, v)| format!("export {k}={v};"))
        .collect();
    if exports.is_empty() {
        return None;
    }
    let entrypoint = std::env::var("CUA_E2E_FLEET_DOCTOR_ENTRYPOINT")
        .unwrap_or_else(|_| "/opt/cua/desktop/entrypoint.sh".into());
    Some(vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("{} exec {entrypoint}", exports.join(" ")),
    ])
}

fn runtime() -> RuntimeKind {
    match std::env::var("CUA_E2E_FLEET_DOCTOR_RUNTIME").as_deref() {
        Ok("kubevirt") => RuntimeKind::Kubevirt,
        _ => RuntimeKind::Gvisor,
    }
}

async fn run(fleet: &FleetClient, name: &str, image: &str) -> Result<Status, String> {
    let t0 = Instant::now();
    let spec = SandboxSpec {
        services: [("env".to_string(), 3211)].into(),
        claim_secrets: true,
        // Optional guest environment, `K=V,K=V`, exported by a shell around
        // the image's entrypoint (Fleet templates carry no env yet); for
        // example CUA_ENV_RUNTIME=gvisor to compare an image whose spacesd
        // predates Fleet's runtime detection. Never a token.
        command: guest_env_command(),
        memory_mb: Some(4096),
        cpu: Some(2),
        ..SandboxSpec::new(image.to_string())
    };
    let options = PoolOptions {
        runtime: Some(runtime()),
        warm: Some(true),
        max_pool_size: Some(1),
        idle_ttl: Some(Duration::from_secs(3600)),
        pool_ttl: Some(Duration::from_secs(2 * 3600)),
        ..Default::default()
    };
    let handle = fleet
        .apply(name, &spec, &options)
        .await
        .map_err(|e| format!("apply: {e}"))?;
    println!(
        "[{:>4}s] applied {name} ({:?})",
        t0.elapsed().as_secs(),
        runtime()
    );
    fleet
        .wait_pool_ready(&handle, Duration::from_secs(1500))
        .await
        .map_err(|e| format!("warm: {e}"))?;
    println!("[{:>4}s] warm replica ready", t0.elapsed().as_secs());
    let token = generate_claim_token();
    let bound = fleet
        .acquire(
            &handle.pool,
            ClaimOptions {
                name: Some(format!("{name}-c")),
                claim_token: Some(token.clone()),
                ttl_seconds_after_created: Some(3600),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("acquire with a claim token: {e}"))?;
    println!(
        "[{:>4}s] claim {} bound, token delivered",
        t0.elapsed().as_secs(),
        bound.claim
    );
    let connect = fleet
        .env_connect_options(&bound, "env", Some(token))
        .map_err(|e| format!("gateway options: {e}"))?;
    let client = SpacesdClient::connect(connect)
        .await
        .map_err(|e| format!("connect through the gateway: {e}"))?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let options = pb::DiagnoseOptions {
        strict: std::env::var("CUA_E2E_FLEET_DOCTOR_STRICT").as_deref() == Ok("1"),
        effects: pb::DiagnoseEffects::VirtualOnly as i32,
        timeout: Some(cua_proto::wkt::Duration {
            seconds: 600,
            nanos: 0,
        }),
        host_time: Some(cua_proto::wkt::Timestamp {
            seconds: now.as_secs() as i64,
            nanos: now.subsec_nanos() as i32,
        }),
        ..Default::default()
    };
    let report = client
        .diagnose_report(options, |c| {
            if c.status != Status::Pass {
                println!("  {:?} {}: {}", c.status, c.id, c.message);
            }
        })
        .await
        .map_err(|e| format!("Diagnose through the gateway: {e}"))?;
    let out = std::env::var("CUA_E2E_FLEET_DOCTOR_OUT")
        .unwrap_or_else(|_| "fleet-doctor-report.json".into());
    std::fs::write(&out, report.to_json()).map_err(|e| format!("write {out}: {e}"))?;
    let s = &report.summary;
    println!(
        "[{:>4}s] doctor {:?}: {} pass, {} warn, {} fail, {} skip (runtime {}, report {out})",
        t0.elapsed().as_secs(),
        s.status,
        s.pass,
        s.warn,
        s.fail,
        s.skip,
        report.environment.runtime
    );
    fleet
        .release(&bound.namespace, &bound.claim)
        .await
        .map_err(|e| format!("release: {e}"))?;
    Ok(s.status)
}

#[tokio::test]
async fn live_image_doctor_through_the_gateway() {
    let Some(image) = image() else {
        eprintln!("skipped: set CUA_E2E_FLEET=1 and CUA_E2E_FLEET_DOCTOR_IMAGE");
        return;
    };
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let rt = match runtime() {
        RuntimeKind::Kubevirt => "kv",
        _ => "gv",
    };
    let name = format!(
        "cua-e2e-doctor-{:06x}-{rt}",
        rand::random::<u32>() & 0xff_ffff
    );
    let result = run(&fleet, &name, &image).await;
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
    let status = result.unwrap();
    assert_ne!(status, Status::Fail, "the image doctor failed on Fleet");
}
