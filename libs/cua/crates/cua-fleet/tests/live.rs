#![allow(deprecated)] // exercises the deprecated `apply_pool` wrapper too
//! Live Fleet tests. Skipped unless `CUA_E2E_FLEET=1` (and Fleet credentials:
//! `CUA_CLIENT_ID`/`CUA_CLIENT_SECRET` or `FLEETS_TOKEN`).
//!
//! Each test creates a `cua-e2e-<rand>` pool, claims a sandbox, checks that
//! the image's `server` service answers `/status` with HTTP 200 through the
//! gateway (reachability only; the check is daemon-agnostic), then releases
//! the claim and deletes the pool whatever happened.
//!
//! Overrides (e.g. for the canonical `ghcr.io/trycua/linux` images, which have
//! no `server` service but serve the cua-spacesd viewer on 3211):
//! `CUA_E2E_FLEET_GVISOR_IMAGE`, `CUA_E2E_FLEET_KUBEVIRT_IMAGE` and
//! `CUA_E2E_FLEET_SERVICE=<name>:<port>:<path>` (default `server:8000:/status`).
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 cargo test -p cua-fleet --test live -- --nocapture
//! CUA_E2E_FLEET=1 CUA_E2E_FLEET_SERVICE=viewer:3211:/viewer/ \
//!   CUA_E2E_FLEET_GVISOR_IMAGE=ghcr.io/trycua/linux:24.04 \
//!   CUA_E2E_FLEET_KUBEVIRT_IMAGE=ghcr.io/trycua/linux:24.04-disk \
//!   cargo test -p cua-fleet --test live -- --nocapture
//! ```

use cua_fleet::{ClaimOptions, FleetClient, PoolSpec, RuntimeKind};
use std::time::{Duration, Instant};

/// The pinned built-in Linux image (cua-sandbox `DEFAULT_LINUX_REGISTRY_IMAGE`,
/// `BUILTIN_REGISTRY_IMAGES[("linux","ubuntu","24.04","vm")]`).
/// Index digest sha256:eb68411ed8b4d7c39829cdfe854b9d0485b78ee064c3171fd8e3f7450f7ccee7.
const KUBEVIRT_IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34";

/// The same image family's gVisor rootfs (`docker-*` tag; multi-arch index
/// sha256:c67f330f287b0e5124f57c64bcf06a7e4b90208186c3036e1312b15d854f6d13).
const GVISOR_IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81";

fn enabled() -> bool {
    std::env::var("CUA_E2E_FLEET").as_deref() == Ok("1")
}

fn image(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The probed service: `(name, port, path)`.
fn probe() -> (String, u16, String) {
    let raw = std::env::var("CUA_E2E_FLEET_SERVICE").unwrap_or_default();
    let mut parts = raw.splitn(3, ':');
    match (
        parts.next(),
        parts.next().and_then(|p| p.parse().ok()),
        parts.next(),
    ) {
        (Some(n), Some(port), Some(path)) if !n.is_empty() => (n.into(), port, path.into()),
        _ => ("server".into(), 8000, "/status".into()),
    }
}

async fn roundtrip(runtime: RuntimeKind, image: &str) {
    if !enabled() {
        eprintln!("skipped: set CUA_E2E_FLEET=1 to run live Fleet tests");
        return;
    }
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let name = format!("cua-e2e-{:08x}", rand::random::<u32>());
    let label = format!("{runtime:?}").to_lowercase();
    let (svc, port, path) = probe();
    eprintln!("[{label}] pool {name} image {image} probe {svc}:{port}{path}");
    let spec = PoolSpec::new(&name, image)
        .runtime(runtime)
        .services([(svc.as_str(), port)]);

    let t0 = Instant::now();
    let mut claim_name: Option<String> = None;
    let mut pool_handle = None;
    let outcome: Result<(), String> = async {
        let handle = fleet
            .apply_pool(&spec)
            .await
            .map_err(|e| format!("apply: {e}"))?;
        let t_apply = t0.elapsed();
        pool_handle = Some(handle.clone());
        let (claim, _) = fleet
            .claim(&handle.pool, ClaimOptions::default())
            .await
            .map_err(|e| format!("claim: {e}"))?;
        claim_name = Some(claim.metadata.name.clone());
        let bound = fleet
            .wait_claim(&claim)
            .await
            .map_err(|e| format!("wait_claim: {e}"))?;
        let t_bound = t0.elapsed();
        eprintln!(
            "[{label}] bound {} (services {:?}) apply={t_apply:?} bound={t_bound:?}",
            bound.name, bound.services
        );
        // Reachability of the probed service through the gateway.
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut last: String;
        loop {
            match fleet
                .service_request(
                    &bound,
                    &svc,
                    &path,
                    "GET",
                    None,
                    Some(Duration::from_secs(20)),
                )
                .await
            {
                Ok(r) if r.status == 200 => {
                    eprintln!(
                        "[{label}] {path} 200 after {:?} (total {:?}): {}",
                        t0.elapsed() - t_bound,
                        t0.elapsed(),
                        String::from_utf8_lossy(&r.body)
                            .chars()
                            .take(120)
                            .collect::<String>()
                    );
                    return Ok(());
                }
                Ok(r) => last = format!("HTTP {}", r.status),
                Err(e) => last = e.to_string(),
            }
            if Instant::now() > deadline {
                return Err(format!("{path} never returned 200 (last: {last})"));
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    .await;

    // Cleanup, always.
    let tc = Instant::now();
    if let Some(c) = &claim_name
        && let Err(e) = fleet.release(&name, c).await
    {
        eprintln!("[{label}] release failed: {e}");
    }
    let handle = match pool_handle {
        Some(h) => Some(h),
        None => fleet.get_pool(&name).await.ok(),
    };
    let mut leaked = false;
    if let Some(h) = handle
        && let Err(e) = fleet.delete_pool(h).await
    {
        eprintln!("[{label}] delete_pool failed: {e}");
        leaked = true;
    }
    // The pool must disappear (deletion is asynchronous; a 403/404 on read
    // means the namespace is gone).
    let gone_deadline = Instant::now() + Duration::from_secs(120);
    while fleet.get_pool(&name).await.is_ok() {
        if Instant::now() > gone_deadline {
            eprintln!("[{label}] pool {name} still exists 120s after delete");
            leaked = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    eprintln!(
        "[{label}] cleanup {:?}; total {:?}",
        tc.elapsed(),
        t0.elapsed()
    );
    if let Err(e) = outcome {
        panic!("[{label}] {e}");
    }
    assert!(!leaked, "[{label}] cleanup failed for {name}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_kubevirt_claim_status_roundtrip() {
    roundtrip(
        RuntimeKind::Kubevirt,
        &image("CUA_E2E_FLEET_KUBEVIRT_IMAGE", KUBEVIRT_IMAGE),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_gvisor_claim_status_roundtrip() {
    roundtrip(
        RuntimeKind::Gvisor,
        &image("CUA_E2E_FLEET_GVISOR_IMAGE", GVISOR_IMAGE),
    )
    .await;
}
