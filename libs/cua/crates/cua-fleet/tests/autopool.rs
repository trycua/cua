//! `cua_fleet::autopool::PoolManager` against the in-memory fake Fleet.

use cua_fleet::{
    AcquireOpts, AutoPoolConfig, PoolManager, PoolSpecKey, RuntimeKind,
    autopool::{
        AUTO_POOL_PREFIX, LABEL_LAST_USED, LABEL_MANAGED_BY, LABEL_SPEC_HASH, MANAGED_BY,
        candidate_names,
    },
    testing::FakeFleet,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const IMAGE: &str = "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:0123";
const DAY: u64 = 86_400;

struct Env {
    fake: FakeFleet,
    _home: tempfile::TempDir,
    cfg: AutoPoolConfig,
}

fn env() -> Env {
    let fake = FakeFleet::new();
    let home = tempfile::tempdir().unwrap();
    let cfg = AutoPoolConfig {
        home: home.path().to_path_buf(),
        idle_gc: None,
        heartbeat_every: Some(Duration::from_millis(20)),
        clock: fake.clock(),
        ..AutoPoolConfig::default()
    };
    Env {
        fake,
        _home: home,
        cfg,
    }
}

impl Env {
    fn manager(&self, tenant: &str) -> PoolManager {
        PoolManager::new(self.fake.client_for_tenant(tenant), self.cfg.clone())
    }

    /// A manager with its own cache directory (another process).
    fn other_process(&self, tenant: &str) -> (PoolManager, tempfile::TempDir) {
        let home = tempfile::tempdir().unwrap();
        let cfg = AutoPoolConfig {
            home: home.path().to_path_buf(),
            ..self.cfg.clone()
        };
        (
            PoolManager::new(self.fake.client_for_tenant(tenant), cfg),
            home,
        )
    }

    fn pool(&self, name: &str) -> Value {
        self.fake.object("pool", name, name).expect("pool exists")
    }

    fn auto_pools(&self) -> Vec<String> {
        self.fake
            .all_namespaces()
            .into_iter()
            .filter(|n| n.starts_with(AUTO_POOL_PREFIX) && self.fake.exists("pool", n, n))
            .collect()
    }

    fn pool_patches(&self, name: &str) -> Vec<Value> {
        self.fake
            .requests()
            .into_iter()
            .filter(|r| {
                r.method == "PATCH" && r.path.ends_with(&format!("osgymsandboxwarmpools/{name}"))
            })
            .filter_map(|r| r.body)
            .collect()
    }

    fn heartbeats(&self, claim: &str) -> usize {
        self.fake
            .requests()
            .iter()
            .filter(|r| {
                r.method == "PATCH"
                    && r.path.ends_with(&format!("osgymsandboxclaims/{claim}"))
                    && r.body
                        .as_ref()
                        .is_some_and(|b| b["spec"]["lifecycle"]["shutdownTime"].is_string())
            })
            .count()
    }
}

fn key() -> PoolSpecKey {
    PoolSpecKey::new(IMAGE).resources(Some(2), Some(4096))
}

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

// ------------------------------------------------------------- creation

#[tokio::test]
async fn first_acquire_creates_an_autoscaled_labeled_pool_and_a_ttl_claim() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    let hash = key().spec_hash();
    assert!(c.created_pool);
    assert_eq!(c.pool, candidate_names("alice", &hash)[0]);
    assert_eq!(e.fake.namespace_owner(&c.pool).as_deref(), Some("alice"));

    let p = e.pool(&c.pool);
    assert_eq!(p["spec"]["replicas"], 0, "cold: KEDA scales from zero");
    assert_eq!(
        p["spec"]["autoscaling"],
        json!({"minPoolSize": 0, "initialPoolSize": 0, "maxPoolSize": 10})
    );
    assert_eq!(p["spec"]["ttlSecondsAfterCreated"], 7 * DAY);
    let labels = &p["metadata"]["labels"];
    assert_eq!(labels[LABEL_MANAGED_BY], MANAGED_BY);
    assert_eq!(labels[LABEL_SPEC_HASH], &hash[..32]);
    assert_eq!(labels[LABEL_LAST_USED], e.fake.now().to_string());

    let t = e.fake.object("template", &c.pool, &c.pool).unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["containerDiskImage"], IMAGE);
    assert_eq!(t["spec"]["vmTemplate"]["cpuCores"], 2);
    assert_eq!(t["spec"]["vmTemplate"]["memory"], "4096Mi");

    let claim = e.fake.object("claim", &c.pool, &c.claim).unwrap();
    assert_eq!(claim["spec"]["ttlSecondsAfterCreated"], 900);
    assert_eq!(claim["spec"]["bindDeadline"], 900);
    assert_eq!(claim["spec"]["lifecycle"]["shutdownPolicy"], "Delete");
    assert_eq!(claim["metadata"]["labels"][LABEL_MANAGED_BY], MANAGED_BY);
    assert_eq!(c.sandbox.namespace, c.pool);
    c.release().await.unwrap();
}

#[tokio::test]
async fn warm_and_max_pool_size_shape_new_pools() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr
        .acquire(
            key(),
            AcquireOpts {
                warm: Some(true),
                max_pool_size: Some(4),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let p = e.pool(&c.pool);
    assert_eq!(p["spec"]["replicas"], 1);
    // Warm is an explicit floor of one: Fleet has no server-side warm floor.
    assert_eq!(
        p["spec"]["autoscaling"],
        json!({"minPoolSize": 1, "initialPoolSize": 1, "maxPoolSize": 4})
    );
    // #7886 lifecycle fields: idle TTL = the client GC threshold, Cascade.
    assert_eq!(
        p["spec"]["idleTtlSeconds"],
        e.cfg
            .idle_gc
            .map(|d| d.as_secs())
            .map_or(json!(null), |s| json!(s))
    );
    assert_eq!(p["spec"]["ttlPolicy"], "Cascade");
    c.release().await.unwrap();

    let cfg_warm = AutoPoolConfig {
        warm: true,
        ..e.cfg.clone()
    };
    let mgr = PoolManager::new(e.fake.client_for_tenant("bob"), cfg_warm);
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(e.pool(&c.pool)["spec"]["replicas"], 1);
    c.release().await.unwrap();
}

// ---------------------------------------------------------------- reuse

#[tokio::test]
async fn same_key_reuses_the_pool_across_processes_and_different_keys_do_not() {
    let e = env();
    let a = e.manager("alice");
    let c1 = a.acquire(key(), AcquireOpts::default()).await.unwrap();
    let pool = c1.pool.clone();
    c1.release().await.unwrap();

    // Another process (empty cache): server-side discovery.
    let (b, _home) = e.other_process("alice");
    let c2 = b.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c2.pool, pool);
    assert!(!c2.created_pool);
    c2.release().await.unwrap();

    // Different cpu, memory, runtime, image, services: different pools.
    let variants = [
        key().resources(Some(4), Some(4096)),
        key().resources(Some(2), Some(8192)),
        key().runtime(RuntimeKind::Gvisor),
        PoolSpecKey::new("public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:4567")
            .resources(Some(2), Some(4096)),
        key().services([("env", 3211u16), ("vnc", 5900)]),
    ];
    let mut seen = vec![pool];
    for k in variants {
        let c = a.acquire(k, AcquireOpts::default()).await.unwrap();
        assert!(c.created_pool);
        assert!(!seen.contains(&c.pool), "{} reused", c.pool);
        seen.push(c.pool.clone());
        c.release().await.unwrap();
    }
    assert_eq!(e.auto_pools().len(), 6);
}

#[tokio::test]
async fn the_cache_is_private_and_a_stale_entry_falls_back_to_discovery() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    let pool = c.pool.clone();
    c.release().await.unwrap();
    let path = e.cfg.home.join("fleet-pools.json");
    let cache: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(cache.to_string().contains(&pool));
    assert!(!cache.to_string().contains("alice"), "tenant is hashed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    // The pool disappears behind our back (another machine's GC).
    let sdk = e.fake.client_for_tenant("alice").sdk();
    let p = sdk.clone().get_pool(pool.clone()).await.unwrap();
    sdk.delete_pool(p).await.unwrap();
    let (fresh, _h) = (
        PoolManager::new(e.fake.client_for_tenant("alice"), e.cfg.clone()),
        (),
    );
    let c = fresh.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c.pool, pool, "recreated under the same deterministic name");
    assert!(c.created_pool);
    c.release().await.unwrap();
}

#[tokio::test]
async fn a_pool_labeled_with_the_spec_hash_is_found_under_any_name() {
    let e = env();
    let hash = key().spec_hash();
    // Seed a managed pool created under another derivation (another key
    // of the same account).
    let odd = "cua-auto-legacyname000001";
    e.fake.add_namespace_for(odd, "alice");
    e.fake.put_object(
        "pool",
        odd,
        odd,
        json!({"apiVersion": "osgym.cua.ai/v1alpha1", "kind": "OSGymSandboxWarmPool",
            "metadata": {"namespace": odd, "name": odd,
                "labels": {LABEL_SPEC_HASH: &hash[..32], LABEL_MANAGED_BY: MANAGED_BY},
                "creationTimestamp": "2026-01-01T00:00:00Z"},
            "spec": {"replicas": 0, "sandboxTemplateRef": {"name": odd}}}),
    );
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c.pool, odd);
    assert!(!c.created_pool);
    assert!(
        e.fake.exists("template", odd, odd),
        "a missing template is recreated on reuse"
    );
    c.release().await.unwrap();
}

#[tokio::test]
async fn a_leftover_namespace_without_a_pool_is_reused_for_creation() {
    let e = env();
    let name = candidate_names("alice", &key().spec_hash())[0].clone();
    e.fake.add_namespace_for(&name, "alice");
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    assert_eq!(c.pool, name);
    assert!(c.created_pool);
    c.release().await.unwrap();
}

#[tokio::test]
async fn a_terminating_pool_is_skipped() {
    let e = env();
    let names = candidate_names("alice", &key().spec_hash());
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    c.release().await.unwrap();
    e.fake.update_object("pool", &names[0], &names[0], |p| {
        p["metadata"]["deletionTimestamp"] = json!("2026-01-01T00:00:00Z");
    });
    let (m, _h) = e.other_process("alice");
    let c = m.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c.pool, names[1]);
    c.release().await.unwrap();
}

// ------------------------------------------------------ tenants / names

#[tokio::test]
async fn tenants_get_separate_pools_and_cannot_see_each_other() {
    let e = env();
    let a = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    let b = e
        .manager("bob")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    assert_ne!(a.pool, b.pool);
    assert!(b.created_pool);
    let bob_list = e.manager("bob").list().await.unwrap();
    assert_eq!(
        bob_list.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        vec![b.pool.clone()]
    );
    a.release().await.unwrap();
    b.release().await.unwrap();
}

#[tokio::test]
async fn a_name_squatted_by_another_account_probes_the_next_suffix() {
    let e = env();
    let names = candidate_names("alice", &key().spec_hash());
    // Mallory holds the first two names (409 on namespace create, then 403).
    e.fake.add_namespace_for(&names[0], "mallory");
    e.fake.add_namespace_for(&names[1], "mallory");
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c.pool, names[2]);
    assert_eq!(
        e.fake.namespace_owner(&names[0]).as_deref(),
        Some("mallory")
    );
    assert!(!e.fake.exists("pool", &names[0], &names[0]));
    c.release().await.unwrap();

    // Stable: the next process lands on the same suffix.
    let (m, _h) = e.other_process("alice");
    let c = m.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c.pool, names[2]);
    c.release().await.unwrap();
}

#[tokio::test]
async fn every_name_taken_is_an_error_not_a_loop() {
    let e = env();
    for n in candidate_names("alice", &key().spec_hash()) {
        e.fake.add_namespace_for(&n, "mallory");
    }
    let err = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("no free managed pool name"),
        "{err}"
    );
}

#[tokio::test]
async fn capsule_adoption_lag_is_retried_not_treated_as_a_foreign_name() {
    let e = env();
    e.fake.faults.lock().unwrap().adoption_denials = 2;
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    assert_eq!(c.pool, candidate_names("alice", &key().spec_hash())[0]);
    c.release().await.unwrap();
}

// ---------------------------------------------------------- concurrency

#[tokio::test]
async fn a_lost_create_race_reuses_the_winner() {
    let e = env();
    e.fake.faults.lock().unwrap().race_pool_create = 1;
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    assert!(
        !c.created_pool,
        "409 AlreadyExists means someone else created it"
    );
    assert_eq!(e.auto_pools(), vec![c.pool.clone()]);
    assert!(e.fake.exists("template", &c.pool, &c.pool));
    c.release().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_processes_converge_on_one_pool() {
    let e = env();
    let mut managers = vec![];
    let mut homes = vec![];
    for _ in 0..4 {
        let (m, h) = e.other_process("alice");
        managers.push(m);
        homes.push(h);
    }
    // Two tasks per "process" as well.
    let tasks: Vec<_> = managers
        .iter()
        .flat_map(|m| [m.clone(), m.clone()])
        .map(|m| tokio::spawn(async move { m.acquire(key(), AcquireOpts::default()).await }))
        .collect();
    let mut claims = vec![];
    for t in tasks {
        claims.push(t.await.unwrap().unwrap());
    }
    let pools: std::collections::BTreeSet<_> = claims.iter().map(|c| c.pool.clone()).collect();
    assert_eq!(pools.len(), 1, "{pools:?}");
    assert_eq!(e.auto_pools().len(), 1);
    assert_eq!(claims.iter().filter(|c| c.created_pool).count(), 1);
    let pool = claims[0].pool.clone();
    assert_eq!(e.fake.names("claim", &pool).len(), 8);
    for c in claims {
        c.release().await.unwrap();
    }
    assert!(e.fake.names("claim", &pool).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_use_in_one_process_creates_one_pool() {
    let e = env();
    let mgr = e.manager("alice");
    let tasks: Vec<_> = (0..12)
        .map(|_| {
            let m = mgr.clone();
            tokio::spawn(async move { m.acquire(key(), AcquireOpts::default()).await })
        })
        .collect();
    let mut claims = vec![];
    for t in tasks {
        claims.push(t.await.unwrap().unwrap());
    }
    let base = candidate_names("alice", &key().spec_hash())[0].clone();
    assert!(claims.iter().all(|c| c.pool == base), "no -2 suffix");
    assert_eq!(e.auto_pools(), vec![base.clone()]);
    assert_eq!(claims.iter().filter(|c| c.created_pool).count(), 1);
    // One creation: our namespace POST plus create_pool's own (409), and a
    // single pool POST.
    let posts = |suffix: &str| {
        e.fake
            .requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path.ends_with(suffix))
            .count()
    };
    assert_eq!(posts("/api/namespaces"), 2);
    assert_eq!(posts("osgymsandboxwarmpools"), 1);
    for c in claims {
        c.release().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_processes_racing_with_adoption_lag_converge_without_a_suffix() {
    let e = env();
    // The namespace creator's first pool POSTs are refused while Capsule
    // adopts the namespace; the other process sees 409 then 403.
    e.fake.faults.lock().unwrap().adoption_denials = 3;
    let (a, _ha) = e.other_process("alice");
    let (b, _hb) = e.other_process("alice");
    let (ca, cb) = tokio::join!(
        a.acquire(key(), AcquireOpts::default()),
        b.acquire(key(), AcquireOpts::default())
    );
    let (ca, cb) = (ca.unwrap(), cb.unwrap());
    let base = candidate_names("alice", &key().spec_hash())[0].clone();
    assert_eq!(
        (ca.pool.as_str(), cb.pool.as_str()),
        (base.as_str(), base.as_str())
    );
    assert_eq!(e.auto_pools(), vec![base]);
    ca.release().await.unwrap();
    cb.release().await.unwrap();
}

// --------------------------------------------------------- KEDA / specs

#[tokio::test]
async fn reuse_never_overwrites_keda_owned_replicas_or_the_spec() {
    let e = env();
    e.fake.faults.lock().unwrap().keda = true;
    let mgr = e.manager("alice");
    let c1 = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(
        e.pool(&c1.pool)["spec"]["replicas"],
        1,
        "KEDA scaled up on demand"
    );
    // KEDA scales further (other demand).
    e.fake.scale_pool(&c1.pool, 3);
    let (m2, _h) = e.other_process("alice");
    let c2 = m2
        .acquire(
            key(),
            AcquireOpts {
                warm: Some(true),
                max_pool_size: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(c2.pool, c1.pool);
    let p = e.pool(&c1.pool);
    assert_eq!(p["spec"]["replicas"], 3, "replicas untouched");
    assert_eq!(p["spec"]["autoscaling"]["minPoolSize"], 0);
    for patch in e.pool_patches(&c1.pool) {
        assert!(
            patch["spec"].get("replicas").is_none()
                && patch["spec"].get("sandboxTemplateRef").is_none(),
            "a pool patch touched the spec: {patch}"
        );
    }
    assert!(
        !e.fake.requests().iter().any(|r| r.method == "PUT"),
        "no full updates"
    );
    // Raising the ceiling is the one spec write, and only that field.
    assert_eq!(p["spec"]["autoscaling"]["maxPoolSize"], 10);
    let c3 = m2
        .acquire(
            key(),
            AcquireOpts {
                max_pool_size: Some(20),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let p = e.pool(&c1.pool);
    assert_eq!(p["spec"]["autoscaling"]["maxPoolSize"], 20);
    assert_eq!(p["spec"]["replicas"], 3);
    for c in [c1, c2, c3] {
        c.release().await.unwrap();
    }
}

// ------------------------------------------------------ claims / leases

#[tokio::test]
async fn heartbeat_renews_until_release_and_release_deletes_the_claim() {
    let e = env();
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    let claim = c.claim.clone();
    sleep_ms(150).await;
    assert!(c.heartbeat_active());
    assert!(e.heartbeats(&claim) >= 2, "{}", e.heartbeats(&claim));
    let pool = c.pool.clone();
    c.release().await.unwrap();
    assert!(!e.fake.exists("claim", &pool, &claim));
    let n = e.heartbeats(&claim);
    sleep_ms(100).await;
    assert_eq!(e.heartbeats(&claim), n, "heartbeat stopped");
    assert!(e.fake.exists("pool", &pool, &pool), "the pool is reusable");
}

#[tokio::test]
async fn detach_keeps_the_claim_and_stops_the_heartbeat() {
    let e = env();
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    let (pool, claim) = (c.pool.clone(), c.claim.clone());
    let bound = c.detach();
    assert_eq!(bound.claim, claim);
    let n = e.heartbeats(&claim);
    sleep_ms(100).await;
    assert_eq!(e.heartbeats(&claim), n);
    assert!(e.fake.exists("claim", &pool, &claim));
}

#[tokio::test]
async fn drop_releases_in_the_background_and_stops_the_heartbeat() {
    let e = env();
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    let (pool, claim) = (c.pool.clone(), c.claim.clone());
    drop(c);
    for _ in 0..100 {
        if !e.fake.exists("claim", &pool, &claim) {
            break;
        }
        sleep_ms(10).await;
    }
    assert!(!e.fake.exists("claim", &pool, &claim));
    let n = e.heartbeats(&claim);
    sleep_ms(80).await;
    assert_eq!(e.heartbeats(&claim), n);

    // Drop with release disabled detaches.
    let mut c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    c.set_release_on_drop(false);
    let claim = c.claim.clone();
    drop(c);
    sleep_ms(50).await;
    assert!(e.fake.exists("claim", &pool, &claim));
}

#[tokio::test]
async fn a_crashed_holder_claim_expires_by_ttl_while_a_live_one_does_not() {
    let e = env();
    let mgr = e.manager("alice");
    let ttl = Duration::from_secs(180);
    let opts = AcquireOpts {
        claim_ttl: Some(ttl),
        ..Default::default()
    };
    let live = mgr.acquire(key(), opts.clone()).await.unwrap();
    let crashed = mgr.acquire(key(), opts).await.unwrap();
    let pool = live.pool.clone();
    let (live_name, dead_name) = (live.claim.clone(), crashed.claim.clone());
    // The holder dies: nothing releases and nothing renews.
    let _ = crashed.detach();
    e.fake.advance(ttl / 2);
    sleep_ms(80).await; // the live heartbeat renews to now + ttl
    assert!(e.fake.reap().is_empty(), "nothing expires within the TTL");
    e.fake.advance(ttl / 2 + Duration::from_secs(60));
    let reaped = e.fake.reap();
    assert_eq!(reaped, vec![("claim".into(), pool.clone(), dead_name)]);
    assert!(e.fake.exists("claim", &pool, &live_name));
    live.release().await.unwrap();
}

#[tokio::test]
async fn a_detached_claim_is_reaped_after_its_ttl() {
    let e = env();
    let mgr = e.manager("alice");
    let ttl = Duration::from_secs(180);
    let c = mgr
        .acquire(
            key(),
            AcquireOpts {
                claim_ttl: Some(ttl),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let (pool, claim) = (c.pool.clone(), c.claim.clone());
    let _ = c.detach(); // what a SIGKILL leaves behind: a claim nobody renews
    e.fake.advance(ttl - Duration::from_secs(30));
    assert!(e.fake.reap().is_empty());
    e.fake.advance(Duration::from_secs(60));
    let reaped = e.fake.reap();
    assert_eq!(reaped, vec![("claim".into(), pool.clone(), claim.clone())]);
    assert!(e.fake.exists("pool", &pool, &pool));
}

#[tokio::test]
async fn the_heartbeat_runs_while_a_cold_claim_is_pending() {
    let e = env();
    // Bind only after many reads (a cold KEDA scale-up + boot).
    e.fake.faults.lock().unwrap().pending_reads = 30;
    let cfg = AutoPoolConfig {
        heartbeat_every: Some(Duration::from_millis(5)),
        ..e.cfg.clone()
    };
    let mut fleet_cfg = e.fake.client_for_tenant("alice").config().clone();
    fleet_cfg.claim_poll_interval_ms = 10;
    fleet_cfg.claim_poll_limit = 100;
    let client = cua_fleet::FleetClient::connect_with_http_client(
        fleet_cfg,
        std::sync::Arc::new(e.fake.clone()),
    )
    .unwrap();
    let mgr = PoolManager::new(client, cfg);
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    // wait_claim reads the template once the claim is Bound: count the
    // renewals sent before that last template read.
    let reqs = e.fake.requests();
    let bound_at = reqs
        .iter()
        .rposition(|r| r.method == "GET" && r.path.contains("osgymsandboxtemplates/"))
        .unwrap();
    let renewals_before_bind = reqs[..bound_at]
        .iter()
        .filter(|r| r.method == "PATCH" && r.path.contains("osgymsandboxclaims/"))
        .count();
    assert!(
        renewals_before_bind >= 2,
        "{renewals_before_bind} renewals while Pending"
    );
    c.release().await.unwrap();
}

#[tokio::test]
async fn the_heartbeat_never_shortens_a_longer_keep_alive() {
    let e = env();
    let c = e
        .manager("alice")
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap();
    let far = e.fake.now() + 6 * 3600;
    c.extend_until(far);
    sleep_ms(80).await;
    let t = e.fake.object("claim", &c.pool, &c.claim).unwrap()["spec"]["lifecycle"]["shutdownTime"]
        .as_str()
        .unwrap()
        .to_string();
    let t = humantime::parse_rfc3339_weak(&t)
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert_eq!(t, far);
    c.release().await.unwrap();
}

#[tokio::test]
async fn bind_deadline_is_at_least_900s_and_follows_the_callers_budget() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr
        .acquire(
            key(),
            AcquireOpts {
                bind_deadline: Some(Duration::from_secs(1800)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let claim = e.fake.object("claim", &c.pool, &c.claim).unwrap();
    assert_eq!(claim["spec"]["bindDeadline"], 1800);
    c.release().await.unwrap();
    let short = PoolManager::new(
        e.fake.client_for_tenant("alice"),
        AutoPoolConfig {
            bind_deadline: Duration::from_secs(60),
            ..e.cfg.clone()
        },
    );
    let c = short.acquire(key(), AcquireOpts::default()).await.unwrap();
    let claim = e.fake.object("claim", &c.pool, &c.claim).unwrap();
    assert_eq!(claim["spec"]["bindDeadline"], 900);
    c.release().await.unwrap();
}

#[tokio::test]
async fn a_named_claim_is_reattached() {
    let e = env();
    let mgr = e.manager("alice");
    let opts = AcquireOpts {
        name: Some("my-box".into()),
        ..Default::default()
    };
    let c1 = mgr.acquire(key(), opts.clone()).await.unwrap();
    assert!(!c1.reattached);
    let bound = c1.detach();
    let c2 = mgr.acquire(key(), opts).await.unwrap();
    assert!(c2.reattached);
    assert_eq!(c2.sandbox, bound);
    c2.release().await.unwrap();
}

#[tokio::test]
async fn a_failed_bind_deletes_the_claim() {
    let e = env();
    e.fake.faults.lock().unwrap().fail_bind = true;
    let mgr = e.manager("alice");
    let err = mgr
        .acquire(key(), AcquireOpts::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("BindDeadlineExceeded"), "{err}");
    let pool = e.auto_pools()[0].clone();
    assert!(e.fake.names("claim", &pool).is_empty());
}

#[tokio::test]
async fn adopt_heartbeats_an_existing_claim_and_detaches_on_drop() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    let bound = c.detach();
    let adopted = mgr.adopt(bound.clone(), None);
    sleep_ms(100).await;
    assert!(e.heartbeats(&bound.claim) >= 2);
    drop(adopted);
    sleep_ms(50).await;
    assert!(e.fake.exists("claim", &bound.namespace, &bound.claim));
}

// -------------------------------------------------------------- pool TTL

#[tokio::test]
async fn the_backstop_ttl_is_renewed_near_expiry_and_expires_when_unused() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    let pool = c.pool.clone();
    c.release().await.unwrap();
    assert_eq!(e.pool(&pool)["spec"]["ttlSecondsAfterCreated"], 7 * DAY);

    // Used on day 3: outside the renewal window, nothing changes.
    e.fake.advance(Duration::from_secs(3 * DAY));
    let (m, _h) = e.other_process("alice");
    m.acquire(key(), AcquireOpts::default())
        .await
        .unwrap()
        .release()
        .await
        .unwrap();
    assert_eq!(e.pool(&pool)["spec"]["ttlSecondsAfterCreated"], 7 * DAY);

    // Used on day 6.5: renewed to age + 7 days.
    e.fake.advance(Duration::from_secs(3 * DAY + DAY / 2));
    let (m, _h) = e.other_process("alice");
    m.acquire(key(), AcquireOpts::default())
        .await
        .unwrap()
        .release()
        .await
        .unwrap();
    let ttl = e.pool(&pool)["spec"]["ttlSecondsAfterCreated"]
        .as_u64()
        .unwrap();
    assert_eq!(ttl, 6 * DAY + DAY / 2 + 7 * DAY);
    e.fake.advance(Duration::from_secs(2 * DAY));
    assert!(e.fake.reap().is_empty(), "renewed pool survives day 8.5");

    // Unused for 7 more days: the backstop deletes it.
    e.fake.advance(Duration::from_secs(6 * DAY));
    let reaped = e.fake.reap();
    assert!(reaped.contains(&("pool".into(), pool.clone(), pool.clone())));
}

// ---------------------------------------------------------------- GC

fn seed_pool(e: &Env, name: &str, tenant: &str, labels: Value, age: Duration) {
    e.fake.add_namespace_for(name, tenant);
    let created = e.fake.now() - age.as_secs() as i64;
    let ts = humantime::format_rfc3339_seconds(
        std::time::UNIX_EPOCH + Duration::from_secs(created as u64),
    )
    .to_string();
    e.fake.put_object(
        "pool",
        name,
        name,
        json!({"apiVersion": "osgym.cua.ai/v1alpha1", "kind": "OSGymSandboxWarmPool",
            "metadata": {"namespace": name, "name": name, "labels": labels,
                "creationTimestamp": ts},
            "spec": {"replicas": 0, "sandboxTemplateRef": {"name": name}}}),
    );
}

fn seed_claim(e: &Env, ns: &str, name: &str, phase: &str, managed: bool, age: Duration) {
    let created = e.fake.now() - age.as_secs() as i64;
    let ts = humantime::format_rfc3339_seconds(
        std::time::UNIX_EPOCH + Duration::from_secs(created as u64),
    )
    .to_string();
    let labels = if managed {
        json!({LABEL_MANAGED_BY: MANAGED_BY})
    } else {
        json!({})
    };
    e.fake.put_object(
        "claim",
        ns,
        name,
        json!({"apiVersion": "osgym.cua.ai/v1alpha1", "kind": "OSGymSandboxClaim",
            "metadata": {"namespace": ns, "name": name, "labels": labels,
                "creationTimestamp": ts},
            "spec": {"sandboxTemplateRef": {"name": ns}, "ttlSecondsAfterCreated": 900},
            "status": {"phase": phase}}),
    );
}

#[tokio::test]
async fn gc_deletes_idle_pools_and_stuck_claims_only() {
    let e = env();
    let hour = Duration::from_secs(3600);
    // Leftover namespace without a pool, two hours old: deleted.
    e.fake.add_namespace_for("cua-auto-orphan", "alice");
    e.fake.advance(2 * hour);
    let old = json!({LABEL_MANAGED_BY: MANAGED_BY,
        LABEL_LAST_USED: (e.fake.now() - 5 * 3600).to_string()});
    // Idle managed pool: deleted.
    seed_pool(&e, "cua-auto-idle", "alice", old.clone(), 10 * hour);
    // Recently used: kept.
    seed_pool(
        &e,
        "cua-auto-recent",
        "alice",
        json!({LABEL_MANAGED_BY: MANAGED_BY, LABEL_LAST_USED: e.fake.now().to_string()}),
        10 * hour,
    );
    // Idle but with a Bound claim: kept.
    seed_pool(&e, "cua-auto-busy", "alice", old.clone(), 10 * hour);
    seed_claim(&e, "cua-auto-busy", "c-bound", "Bound", true, 2 * hour);
    // Idle with a stuck managed Pending claim past its TTL: claim and pool go.
    seed_pool(&e, "cua-auto-stuck", "alice", old.clone(), 10 * hour);
    seed_claim(&e, "cua-auto-stuck", "c-stuck", "Pending", true, hour);
    seed_claim(&e, "cua-auto-stuck", "c-failed", "Failed", true, hour);
    // A fresh managed Pending claim (within TTL) keeps its pool.
    seed_pool(&e, "cua-auto-binding", "alice", old.clone(), 10 * hour);
    seed_claim(
        &e,
        "cua-auto-binding",
        "c-new",
        "Pending",
        true,
        Duration::from_secs(60),
    );
    // An unmanaged Pending claim is never touched.
    seed_pool(&e, "cua-auto-user", "alice", old.clone(), 10 * hour);
    seed_claim(&e, "cua-auto-user", "c-user", "Pending", false, hour);
    // Legacy ephemeral pool without labels, created long ago: deleted.
    seed_pool(&e, "cua-eph-0123456789ab", "alice", json!({}), 10 * hour);
    // A user's explicit pool: never considered.
    seed_pool(&e, "my-pool", "alice", json!({}), 100 * hour);
    // Another tenant's idle auto pool: invisible.
    seed_pool(&e, "cua-auto-bobs", "bob", old.clone(), 10 * hour);

    let report = e.manager("alice").gc(hour).await.unwrap();
    let mut deleted = report.deleted_pools.clone();
    deleted.sort();
    assert_eq!(
        deleted,
        vec!["cua-auto-idle", "cua-auto-stuck", "cua-eph-0123456789ab"]
    );
    let mut claims = report.deleted_claims.clone();
    claims.sort();
    assert_eq!(
        claims,
        vec!["cua-auto-stuck/c-failed", "cua-auto-stuck/c-stuck"]
    );
    assert_eq!(report.deleted_namespaces, vec!["cua-auto-orphan"]);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    for kept in [
        "cua-auto-recent",
        "cua-auto-busy",
        "cua-auto-binding",
        "cua-auto-user",
    ] {
        assert!(e.fake.exists("pool", kept, kept), "{kept}");
        assert!(report.kept.contains(&kept.to_string()), "{kept}");
    }
    assert!(e.fake.exists("claim", "cua-auto-user", "c-user"));
    assert!(e.fake.exists("pool", "my-pool", "my-pool"));
    assert!(e.fake.exists("pool", "cua-auto-bobs", "cua-auto-bobs"));
    assert!(!e.fake.namespace_exists("cua-auto-idle"));
}

#[tokio::test]
async fn scoped_gc_only_touches_the_named_pools() {
    let e = env();
    let hour = Duration::from_secs(3600);
    let old = json!({LABEL_MANAGED_BY: MANAGED_BY,
        LABEL_LAST_USED: (e.fake.now() - 5 * 3600).to_string()});
    seed_pool(&e, "cua-auto-mine", "alice", old.clone(), 10 * hour);
    seed_pool(&e, "cua-auto-theirs", "alice", old, 10 * hour);
    let r = e
        .manager("alice")
        .gc_pools(hour, &["cua-auto-mine".to_string()])
        .await
        .unwrap();
    assert_eq!(r.deleted_pools, vec!["cua-auto-mine"]);
    assert!(e.fake.exists("pool", "cua-auto-theirs", "cua-auto-theirs"));
}

#[tokio::test]
async fn gc_after_release_removes_the_idle_pool_and_the_next_acquire_recreates_it() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    let pool = c.pool.clone();
    c.release().await.unwrap();
    let r = mgr.gc(Duration::from_secs(1800)).await.unwrap();
    assert!(r.deleted_pools.is_empty(), "just used");
    e.fake.advance(Duration::from_secs(1801));
    let r = mgr.gc(Duration::from_secs(1800)).await.unwrap();
    assert_eq!(r.deleted_pools, vec![pool.clone()]);
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    assert_eq!(c.pool, pool);
    assert!(c.created_pool, "the in-process cache was invalidated");
    c.release().await.unwrap();
}

#[tokio::test]
async fn automatic_gc_runs_at_most_hourly_per_machine() {
    let e = env();
    let cfg = AutoPoolConfig {
        idle_gc: Some(Duration::from_secs(3600)),
        ..e.cfg.clone()
    };
    let mgr = PoolManager::new(e.fake.client_for_tenant("alice"), cfg.clone());
    assert!(mgr.gc_if_due().await.is_some(), "first run");
    assert!(mgr.gc_if_due().await.is_none(), "stamp is fresh");
    // Another process on the same machine (same home) also skips.
    let other = PoolManager::new(e.fake.client_for_tenant("alice"), cfg.clone());
    assert!(other.gc_if_due().await.is_none());
    e.fake.advance(Duration::from_secs(3601));
    // A held lock (another process mid-GC) skips too.
    std::fs::write(cfg.home.join("fleet-gc.lock"), "1").unwrap();
    assert!(other.gc_if_due().await.is_none());
    std::fs::remove_file(cfg.home.join("fleet-gc.lock")).unwrap();
    assert!(other.gc_if_due().await.is_some());
    // Disabled.
    let off = PoolManager::new(
        e.fake.client_for_tenant("alice"),
        AutoPoolConfig {
            idle_gc: None,
            ..cfg
        },
    );
    e.fake.advance(Duration::from_secs(7200));
    assert!(off.gc_if_due().await.is_none());
}

#[tokio::test]
async fn list_skips_namespaces_it_cannot_read() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    // A namespace listed for alice but no longer readable (terminating).
    e.fake.add_namespace_for("cua-auto-terminating", "alice");
    e.fake.put_object(
        "pool",
        "cua-auto-terminating",
        "cua-auto-terminating",
        json!({"metadata": {"namespace": "cua-auto-terminating", "name": "cua-auto-terminating"},
            "spec": {"replicas": 0, "sandboxTemplateRef": {"name": "x"}}}),
    );
    e.fake.faults.lock().unwrap().forbid_reads_in = Some("cua-auto-terminating".into());
    let pools = mgr.list().await.unwrap();
    assert_eq!(
        pools.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        vec![c.pool.clone()]
    );
    c.release().await.unwrap();
}

#[tokio::test]
async fn list_reports_managed_pools() {
    let e = env();
    let mgr = e.manager("alice");
    let c = mgr.acquire(key(), AcquireOpts::default()).await.unwrap();
    let pools = mgr.list().await.unwrap();
    assert_eq!(pools.len(), 1);
    let p = &pools[0];
    assert_eq!(p.name, c.pool);
    assert!(p.managed);
    assert_eq!(p.spec_hash.as_deref(), Some(&key().spec_hash()[..32]));
    assert_eq!(p.image.as_deref(), Some(IMAGE));
    assert_eq!((p.claims, p.bound_claims), (1, 1));
    assert_eq!(p.max_pool_size, Some(10));
    assert_eq!(p.expires_at, p.created.map(|c| c + 7 * DAY as i64));
    assert!(!p.terminating);
    c.release().await.unwrap();
}

// ------------------------------------------------------------ resolver

struct Pin;

#[async_trait::async_trait]
impl cua_fleet::ImageResolver for Pin {
    async fn resolve(&self, image: &str) -> Option<String> {
        image
            .strip_suffix(":latest")
            .map(|repo| format!("{repo}@sha256:feed"))
    }
}

#[tokio::test]
async fn tags_resolve_to_digests_for_the_key_and_the_template() {
    let e = env();
    let mgr = e.manager("alice").with_resolver(Arc::new(Pin));
    let c = mgr
        .acquire(
            PoolSpecKey::new("ghcr.io/x/y:latest"),
            AcquireOpts::default(),
        )
        .await
        .unwrap();
    let t = e.fake.object("template", &c.pool, &c.pool).unwrap();
    assert_eq!(
        t["spec"]["vmTemplate"]["containerDiskImage"],
        "ghcr.io/x/y@sha256:feed"
    );
    let pinned = PoolSpecKey::new("ghcr.io/x/y@sha256:feed");
    let c2 = mgr.acquire(pinned, AcquireOpts::default()).await.unwrap();
    assert_eq!(c2.pool, c.pool, "a tag and its digest share a pool");
    c.release().await.unwrap();
    c2.release().await.unwrap();
}
