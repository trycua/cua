use super::*;
use crate::perf::CacheConfig;
use crate::perf::cachevol::plan_for;
#[cfg(unix)]
use crate::perf::cachevol::{PromoteLimits, scope_key};

fn rt(d: &Path) -> PerfRuntime {
    PerfRuntime::new(PerfConfig::default(), d.join("perf"), 1_000)
}

fn claim(event: &str, git_ref: &str) -> JobClaim {
    JobClaim {
        job_key: "1/2".into(),
        repo: "acme/app".into(),
        account: "acct".into(),
        event: Some(event.into()),
        git_ref: Some(git_ref.into()),
        default_branch: Some("main".into()),
        private: Some(true),
        ..Default::default()
    }
}

#[test]
fn a_claim_becomes_the_meta_the_rules_read() {
    let mut c = claim("push", "refs/heads/main");
    c.fork = Some(false);
    c.cache_policy = Some(CachePolicy::Ro);
    let m = c.meta();
    assert_eq!(m.repo, "acme/app");
    assert_eq!(m.policy_cache, Some(CachePolicy::Ro));
    assert!(!plan_for(&m, &CacheConfig::default()).write);
}

#[test]
fn queued_jobs_and_demand_feed_the_policy() {
    let d = tempfile::tempdir().unwrap();
    let r = rt(d.path());
    assert_eq!(r.last_demand_ms(), 1_000, "starts the scale-to-zero clock");
    r.mark_demand(5_000);
    r.mark_demand(3_000);
    assert_eq!(r.last_demand_ms(), 5_000, "never goes back");
    r.set_queued_jobs(3);
    assert_eq!(r.queued_jobs(), 3);
    let p = r.warm_policy(1, 2, 0.0);
    assert_eq!((p.warm, p.cap, p.ttl_ms), (1, 2, 1_800_000));
    assert!(p.budget.is_none() && p.golden_key.is_none());
}

#[test]
fn the_budget_reaches_the_policy() {
    let d = tempfile::tempdir().unwrap();
    let mut cfg = PerfConfig::default();
    cfg.budget = Some(crate::perf::BudgetConfig {
        monthly_usd: 50.0,
        usd_per_hour: 0.65,
    });
    let r = PerfRuntime::new(cfg, d.path().join("perf"), 0);
    let b = r.warm_policy(1, 2, 12.5).budget.unwrap();
    assert_eq!(
        (b.limit_usd, b.spent_usd, b.usd_per_hour),
        (50.0, 12.5, 0.65)
    );
}

#[test]
fn link_tree_makes_an_independent_tree() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    std::fs::create_dir_all(src.join("a/b")).unwrap();
    std::fs::write(src.join("a/b/f"), b"x").unwrap();
    std::fs::write(src.join("top"), b"y").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("top", src.join("l")).unwrap();
    let dst = d.path().join("out/deep/dst");
    link_tree(&src, &dst).unwrap();
    assert_eq!(std::fs::read(dst.join("a/b/f")).unwrap(), b"x");
    assert_eq!(std::fs::read(dst.join("top")).unwrap(), b"y");
    #[cfg(unix)]
    assert_eq!(std::fs::read_link(dst.join("l")).unwrap(), Path::new("top"));
    // Removing the destination leaves the source whole.
    std::fs::remove_dir_all(&dst).unwrap();
    assert_eq!(std::fs::read(src.join("a/b/f")).unwrap(), b"x");
    // The fallback walker too.
    let dst2 = d.path().join("walk");
    walk_link(&src, &dst2).unwrap();
    assert_eq!(std::fs::read(dst2.join("a/b/f")).unwrap(), b"x");
}

#[cfg(unix)]
#[test]
fn a_slot_folder_holds_only_this_jobs_scope() {
    use std::process::Command;
    let d = tempfile::tempdir().unwrap();
    let r = rt(d.path());
    let key = scope_key("acct", "acme/app");
    // Promote a cargo registry for this repo and another for a neighbour.
    let mk_tar = |name: &str| {
        let src = d.path().join(format!("src-{name}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("crate"), name).unwrap();
        let tar = d.path().join(format!("{name}.tar"));
        assert!(
            Command::new("tar")
                .arg("-cf")
                .arg(&tar)
                .arg("-C")
                .arg(&src)
                .arg(".")
                .status()
                .unwrap()
                .success()
        );
        tar
    };
    let mine = mk_tar("mine");
    let theirs = mk_tar("theirs");
    r.caches
        .promote(
            &key,
            "cargo",
            "any",
            &[("registry", &mine)],
            PromoteLimits::default(),
        )
        .unwrap();
    r.caches
        .promote(
            &scope_key("acct", "acme/other"),
            "cargo",
            "any",
            &[("registry", &theirs)],
            PromoteLimits::default(),
        )
        .unwrap();

    let default = claim("push", "refs/heads/main");
    let plan = plan_for(&default.meta(), &CacheConfig::default());
    let want = vec!["cargo".to_string(), "pnpm".to_string()];
    let p = r.populate_slot_dir("slot1", &plan, &want, None).unwrap();
    assert_eq!(p.stores, vec![("cargo".to_string(), "any".to_string())]);
    let dir = r.slot_dir("slot1");
    assert_eq!(
        std::fs::read(dir.join("cache/cargo/any/current/registry/crate")).unwrap(),
        b"mine"
    );
    assert!(!dir.join("cache/pnpm").exists());

    // A fork PR on a private repo gets nothing: an empty folder.
    let mut fork = claim("pull_request", "refs/pull/1/merge");
    fork.head_repo = Some("evil/app".into());
    let plan = plan_for(&fork.meta(), &CacheConfig::default());
    let p = r.populate_slot_dir("slot2", &plan, &want, None).unwrap();
    assert!(p.stores.is_empty() && !p.mirror);
    assert_eq!(
        std::fs::read_dir(r.slot_dir("slot2").join("cache"))
            .unwrap()
            .count(),
        0
    );

    // A second population of slot1 for a no-cache job clears the links.
    let none = plan_for(
        &claim("pull_request_target", "refs/heads/main").meta(),
        &CacheConfig::default(),
    );
    r.populate_slot_dir("slot1", &none, &want, None).unwrap();
    assert_eq!(
        std::fs::read_dir(r.slot_dir("slot1").join("cache"))
            .unwrap()
            .count(),
        0
    );

    // The mirror folder is linked in.
    let mirror = d.path().join("mirrors/public-x");
    std::fs::create_dir_all(mirror.join("repo.git/objects")).unwrap();
    std::fs::write(mirror.join("repo.git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let p = r
        .populate_slot_dir("slot3", &none, &want, Some(&MirrorRef { folder: mirror }))
        .unwrap();
    assert!(p.mirror);
    assert!(r.slot_dir("slot3").join("mirror/repo.git/objects").is_dir());

    r.remove_slot_dir("slot1");
    assert!(!r.slot_dir("slot1").exists());
    assert!(
        r.caches.current(&key, "cargo", "any").is_some(),
        "the cache itself stays"
    );
}

#[test]
fn read_hashes_do_not_carry_names() {
    let (k, w, rd) = read_hashes("abc", "Acme/App");
    assert_eq!(w, rd);
    assert!(!k.contains("abc") && !w.contains("acme"));
    assert_eq!(k.len(), 64);
}

#[test]
fn slot_notes_round_trip_through_json_with_defaults() {
    let mut p = SlotPerf::default();
    p.start_mode = Some(StartMode::Fork);
    p.job = Some(JobCtx {
        job_key: "1/2".into(),
        repo: "a/b".into(),
        stores: vec![("cargo".into(), "any".into())],
        ..Default::default()
    });
    let s = serde_json::to_string(&p).unwrap();
    assert_eq!(serde_json::from_str::<SlotPerf>(&s).unwrap(), p);
    // An old state file without any of it still loads.
    assert_eq!(
        serde_json::from_str::<SlotPerf>("{}").unwrap(),
        SlotPerf::default()
    );
}

#[test]
fn a_pool_without_perf_toml_has_no_runtime_and_one_with_it_lives_under_the_pool() {
    let d = tempfile::tempdir().unwrap();
    let paths = crate::paths::CiPaths::new(d.path());
    assert!(PerfRuntime::for_pool(&paths, "acme", 5).unwrap().is_none());
    std::fs::create_dir_all(paths.pool_dir("acme")).unwrap();
    std::fs::write(PerfConfig::path(&paths, "acme"), "enabled = true\n").unwrap();
    let r = PerfRuntime::for_pool(&paths, "acme", 5).unwrap().unwrap();
    assert!(r.cfg.enabled);
    assert_eq!(r.root, d.path().join("acme/perf"));
    assert_eq!(r.golden_path(), d.path().join("acme/perf/golden.json"));
    assert_eq!(r.last_demand_ms(), 5);
}
