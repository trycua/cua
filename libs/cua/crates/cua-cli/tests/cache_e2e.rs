//! Opt-in end to end (`CUA_E2E_DISK=1`, needs a container engine): pull
//! three small images through the SDK, start a sandbox on one, prune with a
//! zero budget and check the usage drops, the two unused images are gone
//! from the engine and the running sandbox (and its image) survive; then
//! remove the sandbox and prune the rest.
//!
//! Only images this test pulls are removed: images already in the engine are
//! skipped as candidates (the SDK records only its own pulls), the cua home
//! is a temp dir (its GC ignores other homes' labelled objects), and the
//! sandbox is named `cua-e2e-disk-*` and deleted in the end.

mod common;

use common::Home;

/// Small public images; the first three not already present are used.
const CANDIDATES: &[&str] = &[
    "docker.io/library/busybox:1.36.1-musl",
    "docker.io/library/alpine:3.19.4",
    "docker.io/library/busybox:1.35.0-musl",
    "docker.io/library/alpine:3.18.9",
    "docker.io/library/busybox:1.34.1-musl",
    "docker.io/library/alpine:3.17.10",
];

fn docker_has(reference: &str) -> bool {
    std::process::Command::new("docker")
        .args(["image", "inspect", reference])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn docker_images_bytes(h: &Home, rt: &tokio::runtime::Handle) -> u64 {
    let v = rt.block_on(h.run(&["cache", "du", "--json"])).ok().json();
    v["categories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["category"] == "docker-images")
        .and_then(|c| c["bytes"].as_u64())
        .unwrap_or(0)
}

#[test]
fn pull_three_prune_with_budget_running_sandbox_survives() {
    if std::env::var("CUA_E2E_DISK").as_deref() != Ok("1") {
        eprintln!("skipping: set CUA_E2E_DISK=1 (needs a container engine)");
        return;
    }
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut h = Home::new();
    // The real engine (HOME is a temp dir, so point the docker config back
    // at the engine this user runs), a temp cua home, nothing automatic.
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        h.set("DOCKER_HOST", host);
    } else if let Some(sock) = [
        format!(
            "{}/.colima/default/docker.sock",
            std::env::var("HOME").unwrap()
        ),
        format!("{}/.docker/run/docker.sock", std::env::var("HOME").unwrap()),
        "/var/run/docker.sock".to_string(),
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).exists())
    {
        h.set("DOCKER_HOST", format!("unix://{sock}"));
    }
    h.set("CUA_CACHE_AUTO_GC", "0")
        .set("CUA_DAEMON_MAINTENANCE", "0")
        .set("CUA_NO_DAEMON_AUTOSTART", "1")
        .set("LUME_API", "http://127.0.0.1:9");
    let name = format!("cua-e2e-disk-{:08x}", rand_u32());

    let ledger = cua_vmm::container::ledger::PullLedger::new(h.cua_home().join("docker/pulls"));
    // What the engine holds for this test: the pinned references the SDK
    // recorded (a pull resolves the tag to a digest; an image already in
    // the engine is never recorded, so it is never a candidate).
    let pulled = |ledger: &cua_vmm::container::ledger::PullLedger| -> Vec<String> {
        let mut v: Vec<String> = ledger.records().into_iter().map(|r| r.reference).collect();
        v.sort();
        v
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut images: Vec<String> = Vec::new();
        for r in CANDIDATES {
            if images.len() == 3 {
                break;
            }
            let before = pulled(&ledger);
            rt.block_on(h.run(&["--embedded", "image", "pull", &format!("container:{r}")]))
                .ok();
            if let Some(new) = pulled(&ledger).into_iter().find(|p| !before.contains(p)) {
                assert!(docker_has(&new), "{new} was not pulled");
                images.push(new);
            }
        }
        assert_eq!(
            images.len(),
            3,
            "three candidates were already in the engine"
        );
        // The pulls are older than the prune grace period (2 minutes).
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for r in ledger.records() {
            cua_vmm::disk::mark_used_at(&ledger.file(&r.reference), past);
        }
        let before = docker_images_bytes(&h, rt.handle());
        assert!(before > 0);

        // A running sandbox on the first image.
        let out = rt.block_on(h.run(&[
            "--embedded",
            "sb",
            "create",
            &format!("container:{}", images[0]),
            "--name",
            &name,
            "--runtime",
            "runc",
            "--",
            "sleep",
            "600",
        ]));
        out.ok();

        let p = rt
            .block_on(h.run(&["cache", "prune", "--budget", "0", "--json"]))
            .ok()
            .json();
        let removed: Vec<String> = p["gc"]["removed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["item"]["name"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(removed.len(), 2, "{p}");
        assert!(!docker_has(&images[1]) && !docker_has(&images[2]));
        assert!(docker_has(&images[0]), "the running sandbox's image stays");
        let after = docker_images_bytes(&h, rt.handle());
        assert!(after < before, "usage drops: {before} -> {after}");
        let info = rt
            .block_on(h.run(&["--embedded", "sb", "info", &name, "--json"]))
            .ok()
            .json();
        assert_eq!(info["state"], "running", "{info}");
    }));

    // Cleanup, whatever happened: the sandbox, then every image this test
    // pulled (the ledger names only those).
    let _ = rt.block_on(h.run(&["--embedded", "sb", "rm", &name, "-f"]));
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for r in ledger.records() {
        cua_vmm::disk::mark_used_at(&ledger.file(&r.reference), past);
    }
    let recorded = pulled(&ledger);
    let fin = rt.block_on(h.run(&["cache", "prune", "--all", "--json"]));
    let left: Vec<&String> = recorded.iter().filter(|r| docker_has(r)).collect();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    assert!(
        left.is_empty(),
        "{left:?} still in the engine: {}",
        fin.stdout
    );
}

fn rand_u32() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ std::process::id())
        .unwrap_or(7)
}
