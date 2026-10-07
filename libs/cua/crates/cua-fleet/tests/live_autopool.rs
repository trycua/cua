//! Live auto pool tests against Fleet. Skipped unless `CUA_E2E_FLEET=1`
//! (and Fleet credentials).
//!
//! Every pool is a `cua-auto-*` pool made unique per run by an extra
//! `e2e-<hex>` service, every claim carries `cua.ai/e2e=1`, and every pool
//! is deleted at the end whatever happened. The automatic GC is off, and
//! the explicit GC is scoped to this run's pools, so nothing else in the
//! account is touched.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 cargo test -p cua-fleet --test live_autopool -- --nocapture --test-threads=2
//! ```

use cua_fleet::{AcquireOpts, AutoPoolConfig, FleetClient, PoolManager, PoolSpecKey, RuntimeKind};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

/// The public Ubuntu image's gVisor rootfs (`docker-*` tag); the pool is
/// daemon-agnostic, nothing here probes a guest daemon.
const GVISOR_IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-main-809e3f81";

fn enabled() -> bool {
    std::env::var("CUA_E2E_FLEET").as_deref() == Ok("1")
}

fn manager(home: &std::path::Path) -> PoolManager {
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let cfg = AutoPoolConfig {
        home: home.to_path_buf(),
        idle_gc: None,
        ..AutoPoolConfig::from_env()
    };
    PoolManager::new(fleet, cfg)
}

fn key(tag: &str, cpu: u32) -> PoolSpecKey {
    PoolSpecKey::new(GVISOR_IMAGE)
        .runtime(RuntimeKind::Gvisor)
        .resources(Some(cpu), Some(2048))
        .services([("env".to_string(), 3211u16), (format!("e2e-{tag}"), 9)])
}

fn opts(ttl: Option<Duration>) -> AcquireOpts {
    AcquireOpts {
        claim_ttl: ttl,
        labels: BTreeMap::from([("cua.ai/e2e".to_string(), "1".to_string())]),
        ..Default::default()
    }
}

async fn replicas(mgr: &PoolManager, pool: &str) -> (u64, u64) {
    let raw = mgr
        .fleet()
        .get_pool_json(pool)
        .await
        .unwrap()
        .unwrap_or_default();
    (
        raw["spec"]["replicas"].as_u64().unwrap_or(0),
        raw["status"]["readyReplicas"].as_u64().unwrap_or(0),
    )
}

async fn cleanup(fleet: &FleetClient, pools: &[String]) {
    for p in pools {
        match fleet.sdk().get_pool(p.clone()).await {
            Ok(pool) => match fleet.sdk().delete_pool(pool).await {
                Ok(()) => eprintln!("[cleanup] deleted {p}"),
                Err(e) => eprintln!("[cleanup] LEFT BEHIND {p}: {e}"),
            },
            Err(_) => {
                let _ = fleet.sdk().delete_namespace(p.clone()).await;
            }
        }
    }
}

#[tokio::test]
async fn live_autopool_reuse_keda_and_gc() {
    if !enabled() {
        eprintln!("skipped: set CUA_E2E_FLEET=1 to run live Fleet tests");
        return;
    }
    let tag = format!("{:08x}", rand::random::<u32>());
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path());
    let fleet = mgr.fleet().clone();
    let mut pools: Vec<String> = vec![];
    let outcome: Result<(), String> = async {
        // (a) first acquire: creates the pool, cold bind (KEDA from zero).
        let t0 = Instant::now();
        let c1 = mgr
            .acquire(key(&tag, 2), opts(None))
            .await
            .map_err(|e| format!("acquire 1: {e}"))?;
        pools.push(c1.pool.clone());
        let cold_bind = c1.bind_time;
        eprintln!(
            "[a] cold: pool {} created={} bind={:?}",
            c1.pool, c1.created_pool, c1.bind_time
        );
        if !c1.created_pool {
            return Err("first acquire should create the pool".into());
        }
        let raw = fleet.get_pool_json(&c1.pool).await.unwrap().unwrap();
        eprintln!(
            "[a] pool spec: replicas={} autoscaling={} ttl={} labels={}",
            raw["spec"]["replicas"],
            raw["spec"]["autoscaling"],
            raw["spec"]["ttlSecondsAfterCreated"],
            raw["metadata"]["labels"]
        );
        if raw["spec"]["autoscaling"]["minPoolSize"] != 0 {
            return Err("autoscaling min must be 0".into());
        }

        // (e) KEDA owns replicas: a second manager (fresh cache) resolving
        // the same key must not write them back.
        let (before, _) = replicas(&mgr, &c1.pool).await;
        let home2 = tempfile::tempdir().unwrap();
        let mgr2 = manager(home2.path());
        let k = key(&tag, 2);
        let (p, created) = mgr2
            .ensure_pool(&k, &k.spec_hash(), &AcquireOpts::default())
            .await
            .map_err(|e| format!("ensure_pool: {e}"))?;
        let (after, _) = replicas(&mgr, &c1.pool).await;
        eprintln!(
            "[e] KEDA replicas before={before} after re-resolve={after} (reused {} created={created})",
            p.metadata.name
        );
        if created || p.metadata.name != c1.pool {
            return Err("second manager should reuse the pool".into());
        }
        if before >= 1 && after == 0 {
            return Err("replicas were reset: reuse overwrote the KEDA-owned spec".into());
        }

        c1.release().await.map_err(|e| format!("release 1: {e}"))?;

        // Wait (bounded) for a warm replica after the claim's sandbox is
        // replaced.
        let warm_deadline = Instant::now() + Duration::from_secs(240);
        let mut warm = false;
        while Instant::now() < warm_deadline {
            let (spec, ready) = replicas(&mgr, &pools[0]).await;
            if ready >= 1 {
                eprintln!("[a] warm replica ready (spec={spec}) after {:?}", t0.elapsed());
                warm = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }

        // Second acquire, another process: same pool, faster when warm.
        let c2 = mgr2
            .acquire(key(&tag, 2), opts(None))
            .await
            .map_err(|e| format!("acquire 2: {e}"))?;
        eprintln!(
            "[a] reuse: pool {} created={} bind={:?} (warm={warm})",
            c2.pool, c2.created_pool, c2.bind_time
        );
        if c2.pool != pools[0] || c2.created_pool {
            return Err("second acquire should reuse the pool".into());
        }
        if warm && c2.bind_time >= cold_bind {
            return Err(format!(
                "a warm bind ({:?}) should beat the cold one ({cold_bind:?})",
                c2.bind_time
            ));
        }

        // (b) a different cpu count: a different pool.
        let c3 = mgr
            .acquire(key(&tag, 4), opts(None))
            .await
            .map_err(|e| format!("acquire 3: {e}"))?;
        pools.push(c3.pool.clone());
        eprintln!(
            "[b] cpu=4: pool {} created={} bind={:?}",
            c3.pool, c3.created_pool, c3.bind_time
        );
        if c3.pool == pools[0] || !c3.created_pool {
            return Err("a different cpu count must use another pool".into());
        }
        let listed = mgr.list().await.map_err(|e| format!("list: {e}"))?;
        for p in &pools {
            let info = listed
                .iter()
                .find(|i| &i.name == p)
                .ok_or(format!("{p} not listed"))?;
            eprintln!(
                "[list] {} managed={} replicas={} ready={:?} claims={} image={:?}",
                info.name, info.managed, info.replicas, info.ready_replicas, info.claims, info.image
            );
        }
        c2.release().await.map_err(|e| format!("release 2: {e}"))?;
        c3.release().await.map_err(|e| format!("release 3: {e}"))?;

        // (d) GC (scoped to this run's pools) deletes the idle pools once
        // the released claims are gone.
        let t_gc = Instant::now();
        let mut deleted: Vec<String> = vec![];
        for _ in 0..24 {
            let r = mgr
                .gc_pools(Duration::ZERO, &pools)
                .await
                .map_err(|e| format!("gc: {e}"))?;
            deleted.extend(r.deleted_pools);
            if pools.iter().all(|p| deleted.contains(p)) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        eprintln!("[d] gc deleted {deleted:?} in {:?}", t_gc.elapsed());
        if !pools.iter().all(|p| deleted.contains(p)) {
            return Err(format!("gc left pools behind: {deleted:?}"));
        }
        eprintln!("[total] {:?}", t0.elapsed());
        Ok(())
    }
    .await;
    cleanup(&fleet, &pools).await;
    if let Err(e) = outcome {
        panic!("{e}");
    }
}

/// The child half of the crash test: acquires a claim, prints it, and
/// waits to be killed. Does nothing unless `CUA_AUTOPOOL_CHILD` is set.
#[tokio::test]
async fn live_autopool_child() {
    let Ok(tag) = std::env::var("CUA_AUTOPOOL_CHILD") else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let mgr = manager(home.path());
    let ttl = Duration::from_secs(180);
    let mut o = opts(Some(ttl));
    // The parent knows the claim by this name from the moment it exists
    // (a cold bind can take many minutes).
    o.name = std::env::var("CUA_AUTOPOOL_CLAIM").ok();
    let c = match mgr.acquire(key(&tag, 2), o).await {
        Ok(c) => c,
        Err(e) => {
            println!("ERROR {e}");
            return;
        }
    };
    println!("CLAIM {} {} {}", c.pool, c.claim, c.bind_time.as_secs_f64());
    use std::io::Write as _;
    std::io::stdout().flush().unwrap();
    // Bounded: the parent SIGKILLs us long before this.
    tokio::time::sleep(Duration::from_secs(1800)).await;
}

#[tokio::test]
async fn live_autopool_crashed_process_claim_expires() {
    if !enabled() {
        eprintln!("skipped: set CUA_E2E_FLEET=1 to run live Fleet tests");
        return;
    }
    use tokio::io::{AsyncBufReadExt, BufReader};
    let tag = format!("{:08x}", rand::random::<u32>());
    let fleet = FleetClient::from_env().unwrap();
    let mut pool: Option<String> = None;
    let outcome: Result<(), String> = async {
        // Warm the pool first (a claim is KEDA's demand signal; a cold
        // gVisor desktop pool takes minutes), then hand the warm replica to
        // the child right after releasing.
        let home = tempfile::tempdir().unwrap();
        let mgr = manager(home.path());
        let t_warm = Instant::now();
        let warm = mgr
            .acquire(key(&tag, 2), opts(Some(Duration::from_secs(180))))
            .await
            .map_err(|e| format!("warm-up acquire: {e}"))?;
        pool = Some(warm.pool.clone());
        eprintln!("[c] warm-up bind {:?} ({})", t_warm.elapsed(), warm.pool);
        warm.release().await.map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().unwrap();
        let claim = format!("cua-e2e-kill-{tag}");
        let p = pool.clone().expect("warm-up pool");
        let mut child = tokio::process::Command::new(exe)
            .args([
                "live_autopool_child",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("CUA_AUTOPOOL_CHILD", &tag)
            .env("CUA_AUTOPOOL_CLAIM", &claim)
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("spawn: {e}"))?;
        // Relay the child's output (bounded by the child's lifetime).
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[c] child: {line}");
            }
        });
        // Watch the named claim from creation to Bound (bounded).
        let t_spawn = Instant::now();
        let mut last_phase = String::new();
        loop {
            let phase = fleet
                .list_claims(&p)
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|c| c.metadata.name == claim)
                .map(|c| {
                    c.status
                        .and_then(|s| s.phase)
                        .unwrap_or_else(|| "Pending".into())
                })
                .unwrap_or_else(|| "absent".into());
            if phase != last_phase {
                eprintln!("[c] claim {claim}: {phase} after {:?}", t_spawn.elapsed());
                last_phase = phase.clone();
            }
            if phase == "Bound" {
                break;
            }
            if phase == "Failed" {
                return Err(format!("claim {claim} failed to bind"));
            }
            if t_spawn.elapsed() > Duration::from_secs(1800) {
                return Err(format!("claim {claim} still {phase} after 30 min"));
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("child exited ({status}) before its claim bound"));
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        pool = Some(p.clone());
        let claim_obj = |c: &cua_fleet::Claim| {
            c.spec
                .lifecycle
                .as_ref()
                .and_then(|l| l.shutdown_time.clone())
                .unwrap_or_default()
        };
        let first = fleet
            .list_claims(&p)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| c.metadata.name == claim)
            .ok_or("claim missing")?;
        eprintln!("[c] shutdownTime at bind: {}", claim_obj(&first));
        // Let one heartbeat (every 60 s for a 180 s TTL) land.
        tokio::time::sleep(Duration::from_secs(75)).await;
        let renewed = fleet
            .list_claims(&p)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| c.metadata.name == claim)
            .ok_or("claim vanished while the holder was alive")?;
        eprintln!("[c] shutdownTime after heartbeat: {}", claim_obj(&renewed));
        if claim_obj(&renewed) <= claim_obj(&first) {
            return Err("the heartbeat did not move shutdownTime".into());
        }
        // SIGKILL: no release, no more heartbeats.
        child.start_kill().map_err(|e| e.to_string())?;
        let _ = child.wait().await;
        let killed = Instant::now();
        eprintln!("[c] child SIGKILLed");
        let limit = Duration::from_secs(180 + 120);
        loop {
            let alive = fleet
                .list_claims(&p)
                .await
                .map_err(|e| e.to_string())?
                .iter()
                .any(|c| c.metadata.name == claim);
            if !alive {
                eprintln!(
                    "[c] claim reaped {:?} after the kill (TTL 180 s)",
                    killed.elapsed()
                );
                return Ok(());
            }
            if killed.elapsed() > limit {
                return Err(format!(
                    "claim still present {:?} after the kill",
                    killed.elapsed()
                ));
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    }
    .await;
    if let Some(p) = &pool {
        cleanup(&fleet, std::slice::from_ref(p)).await;
    }
    if let Err(e) = outcome {
        panic!("{e}");
    }
}
