// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Opt-in: `on="auto"` on a loaded machine with a real container runtime.
//! While this process keeps twice as many threads busy as the machine has
//! CPUs (the 1-minute load average passes the CPU count), a Space is still
//! created: admission is by what Spaces reserve, never by load. Then Spaces
//! reserve every CPU and the next create fails with the typed
//! `host_capacity` error (not "not found"). Everything it creates is
//! deleted.
//!
//! ```text
//! CUA_HOME=/tmp/some-throwaway-home CUA_E2E_AUTO_LOAD=1 \
//!   cargo test -p cua-daemon --test auto_load_live -- --ignored --nocapture
//! ```
//!
//! Needs Docker (or Podman) and network for the image, and a throwaway
//! `CUA_HOME` (never `~/.cua`), and a machine with no other Spaces on its
//! container engine (they count as reserved). Image `CUA_E2E_AUTO_LOAD_IMAGE` (default
//! `ghcr.io/trycua/linux:24.04-slim`).

#![cfg(all(unix, feature = "spaces"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cua_daemon::{Runtime, RuntimeConfig, local::VmmLocal};
use cua_sandbox_core::placement::{Kind, On};
use cua_spaces::SpaceCreate;

fn create(image: &str, name: &str, cpus: u32) -> SpaceCreate {
    SpaceCreate {
        on: Some(On::Auto),
        image: Some(image.into()),
        name: Some(name.into()),
        kind: Kind::Container,
        cpus: Some(cpus),
        memory_mb: Some(512),
        timeout: Some(Duration::from_secs(600)),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "needs a container engine; set CUA_E2E_AUTO_LOAD=1"]
async fn auto_creates_on_a_loaded_machine_and_reports_capacity_when_full() {
    if std::env::var("CUA_E2E_AUTO_LOAD").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_E2E_AUTO_LOAD=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_E2E_AUTO_LOAD_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        vmm: Some(Arc::new(VmmLocal::default())),
        spaces_home: Some(home.clone()),
        ..Default::default()
    })
    .unwrap();
    let spaces = rt.spaces().clone();

    // Load the machine: two busy threads per CPU, bounded by `stop` and a
    // deadline, until the load average passes the CPU count.
    let cpus = cua_cluster::sysinfo::cpu_count();
    let stop = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_secs(600);
    let spinners: Vec<_> = (0..cpus * 2)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 1u64;
                while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
                    for _ in 0..100_000 {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    }
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    let started = Instant::now();
    while cua_cluster::sysinfo::load_average().unwrap_or(0.0) <= f64::from(cpus) {
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "the load average never passed {cpus}"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    eprintln!(
        "load {:?} over {cpus} CPUs",
        cua_cluster::sysinfo::load_average()
    );

    let outcome = async {
        // 1. Loaded, nothing reserved: admitted.
        spaces
            .create(create(&image, "auto-load-1", 2))
            .await
            .expect("a loaded machine with no Spaces admits a 2-CPU Space")
            .ready()
            .unwrap();
        // 2. Reserve every CPU, then one more is a capacity error.
        // The first Space books 2 vCPUs: this one books all the rest, so
        // nothing is left for a third.
        let all = cpus * cua_cluster::sysinfo::container_overcommit() - 2;
        spaces
            .create(create(&image, "auto-load-2", all))
            .await
            .expect("a Space that books the rest of the machine")
            .ready()
            .unwrap();
        let err = match spaces.create(create(&image, "auto-load-3", 2)).await {
            Err(e) => e,
            Ok(_) => panic!("a full machine took another Space"),
        };
        assert_eq!(err.tag(), "host_capacity", "{err}");
        eprintln!("full machine: {err}");
        let daemon_err = cua_daemon::Error::from(err);
        assert!(matches!(daemon_err, cua_daemon::Error::HostCapacity(_)));
    }
    .await;

    stop.store(true, Ordering::Relaxed);
    for s in spinners {
        let _ = s.join();
    }
    for name in ["auto-load-1", "auto-load-2", "auto-load-3"] {
        let _ = spaces.delete(&format!("local:{name}")).await;
    }
    outcome
}

/// Idle warm-pool members hold CPU until `on="auto"` is short of it: then
/// they are given back before the create is refused. With the pool already
/// reclaimed and the machine truly full, the create is `host_capacity`.
///
/// ```text
/// CUA_HOME=/tmp/some-throwaway-home CUA_E2E_AUTO_LOAD=1 \
///   cargo test -p cua-daemon --features spaces --test auto_load_live -- \
///   --ignored --nocapture auto_reclaims
/// ```
#[tokio::test]
#[ignore = "needs a container engine; set CUA_E2E_AUTO_LOAD=1"]
async fn auto_reclaims_idle_warm_members_before_refusing() {
    if std::env::var("CUA_E2E_AUTO_LOAD").as_deref() != Ok("1") {
        eprintln!("skipped: set CUA_E2E_AUTO_LOAD=1");
        return;
    }
    let home = std::path::PathBuf::from(
        std::env::var_os("CUA_HOME").expect("set CUA_HOME to a throwaway home"),
    );
    let real = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cua"));
    assert!(Some(&home) != real.as_ref(), "never the user's ~/.cua");
    let image = std::env::var("CUA_E2E_AUTO_LOAD_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/trycua/linux:24.04-slim".into());
    // Two ready members of the default size (2 vCPUs each), kept regardless
    // of use.
    // SAFETY: set before any thread of this test reads the environment.
    unsafe {
        std::env::set_var("CUA_POOL_WARM", format!("{image}=2"));
        std::env::set_var("CUA_CONTAINER_CPU_OVERCOMMIT", "1");
    }
    let vmm = Arc::new(VmmLocal::default());
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        vmm: Some(vmm.clone()),
        spaces_home: Some(home.clone()),
        ..Default::default()
    })
    .unwrap();
    let spaces = rt.spaces().clone();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let task = vmm
        .enable_pool(rt.sandboxes().state().clone(), shutdown)
        .expect("the pool starts");
    let pool = vmm.pool().expect("a pool").clone();
    let started = Instant::now();
    while pool.held().len() < 2 {
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "two warm members never came up: {:?}",
            pool.status()
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let cpus = cua_cluster::sysinfo::cpu_count();

    let outcome = async {
        // The warm members hold 4 CPUs; this Space (a shape no member
        // answers) takes the rest of the machine.
        spaces
            .create(create(&image, "auto-warm-1", cpus - 4))
            .await
            .expect("fits beside the warm members")
            .ready()
            .unwrap();
        // Nothing is free but the warm members' CPUs: the create gives them
        // back, then succeeds.
        spaces
            .create(create(&image, "auto-warm-2", 3))
            .await
            .expect("reclaiming the idle members made room")
            .ready()
            .unwrap();
        assert!(pool.held().is_empty(), "the warm members were given back");
        // The pool is reclaimed and the machine is full: capacity error.
        let err = match spaces.create(create(&image, "auto-warm-3", 3)).await {
            Err(e) => e,
            Ok(_) => panic!("a full machine took another Space"),
        };
        assert_eq!(err.tag(), "host_capacity", "{err}");
        eprintln!("full machine: {err}");
    }
    .await;

    let _ = stop.send(true);
    let _ = task.await;
    for name in ["auto-warm-1", "auto-warm-2", "auto-warm-3"] {
        let _ = spaces.delete(&format!("local:{name}")).await;
    }
    outcome
}
