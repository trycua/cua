use super::*;

fn meta(event: &str, git_ref: &str, head: Option<&str>, private: bool) -> JobMeta {
    JobMeta {
        account: "acct-1".into(),
        repo: "acme/app".into(),
        private: Some(private),
        event: Some(event.into()),
        git_ref: Some(git_ref.into()),
        head_repo: head.map(str::to_string),
        default_branch: Some("main".into()),
        fork: None,
        policy_cache: None,
    }
}

fn cfg() -> CacheConfig {
    CacheConfig::default()
}

#[test]
fn only_a_default_branch_job_may_write() {
    let p = plan_for(&meta("push", "refs/heads/main", None, true), &cfg());
    assert_eq!(p.trust, Trust::DefaultBranch);
    assert!(p.write && p.read.is_some());
    assert_eq!(p.scope, "default_branch");

    for (event, r, head) in [
        ("push", "refs/heads/feature", None),
        ("pull_request", "refs/pull/7/merge", Some("acme/app")),
        ("schedule", "refs/heads/dev", None),
    ] {
        let p = plan_for(&meta(event, r, head, true), &cfg());
        assert_eq!(p.trust, Trust::SameRepo, "{event} {r}");
        assert!(!p.write, "{event} {r}");
        assert!(p.read.is_some());
        assert_eq!(p.scope, "repo");
    }
    // Scheduled and manual runs on the default branch are reviewed code too.
    assert!(plan_for(&meta("schedule", "refs/heads/main", None, true), &cfg()).write);
    assert!(
        plan_for(
            &meta("workflow_dispatch", "refs/heads/main", None, true),
            &cfg()
        )
        .write
    );
}

#[test]
fn a_fork_pull_request_reads_public_caches_only_and_never_writes() {
    let public = plan_for(
        &meta("pull_request", "refs/pull/9/merge", Some("evil/app"), false),
        &cfg(),
    );
    assert_eq!(public.trust, Trust::Fork);
    assert!(public.read.is_some() && !public.write);
    assert_eq!(public.scope, "fork_readonly");
    assert!(public.report_denied_writes);

    let private = plan_for(
        &meta("pull_request", "refs/pull/9/merge", Some("evil/app"), true),
        &cfg(),
    );
    assert!(
        private.read.is_none() && !private.write,
        "a private repo's cache is never given to a fork"
    );
    assert!(private.report_denied_writes);

    // Unknown privacy counts as private.
    let mut m = meta("pull_request", "refs/pull/9/merge", Some("evil/app"), false);
    m.private = None;
    assert!(plan_for(&m, &cfg()).read.is_none());

    // A push whose head lives elsewhere is a fork too.
    let p = plan_for(
        &meta("push", "refs/heads/main", Some("evil/app"), false),
        &cfg(),
    );
    assert_eq!(p.trust, Trust::Fork);
    assert!(!p.write);

    let mut off = cfg();
    off.fork_read_public = false;
    assert!(
        plan_for(
            &meta("pull_request", "refs/pull/9/merge", Some("evil/app"), false),
            &off
        )
        .read
        .is_none()
    );
}

#[test]
fn the_services_word_on_forks_and_its_policy_only_take_rights_away() {
    // The service says fork although the head repo field is missing.
    let mut m = meta("pull_request", "refs/pull/9/merge", None, false);
    m.fork = Some(true);
    assert_eq!(trust_of(&m), Trust::Fork);
    let mut m = meta("push", "refs/heads/main", None, true);
    m.fork = Some(true);
    assert!(!plan_for(&m, &cfg()).write);
    // `ro` removes the write, `off` removes everything, `rw` adds nothing.
    let mut m = meta("push", "refs/heads/main", None, true);
    m.policy_cache = Some(CachePolicy::Ro);
    let p = plan_for(&m, &cfg());
    assert!(p.read.is_some() && !p.write);
    m.policy_cache = Some(CachePolicy::Off);
    let p = plan_for(&m, &cfg());
    assert!(p.read.is_none() && !p.write);
    let mut m = meta("push", "refs/heads/feature", None, true);
    m.policy_cache = Some(CachePolicy::Rw);
    assert!(
        !plan_for(&m, &cfg()).write,
        "the service saying rw never grants a write on its own"
    );
    assert_eq!(CachePolicy::parse("rw"), Some(CachePolicy::Rw));
    assert_eq!(CachePolicy::parse("x"), None);
}

#[test]
fn dangerous_events_and_missing_metadata_get_nothing() {
    for e in [
        "pull_request_target",
        "issue_comment",
        "workflow_run",
        "issues",
    ] {
        let p = plan_for(&meta(e, "refs/heads/main", None, false), &cfg());
        assert_eq!(p.trust, Trust::Untrusted, "{e}");
        assert!(p.read.is_none() && !p.write, "{e}");
    }
    let mut m = meta("push", "refs/heads/main", None, false);
    m.event = None;
    assert_eq!(plan_for(&m, &cfg()).trust, Trust::Unknown);
    let mut m = meta("push", "refs/heads/main", None, false);
    m.default_branch = None;
    let p = plan_for(&m, &cfg());
    assert!(p.read.is_none() && !p.write);
    let mut m = meta("push", "refs/heads/main", None, false);
    m.account.clear();
    assert!(plan_for(&m, &cfg()).read.is_none());
    let mut off = cfg();
    off.enabled = false;
    assert!(
        plan_for(&meta("push", "refs/heads/main", None, false), &off)
            .read
            .is_none()
    );
}

#[test]
fn scopes_never_share() {
    let a = scope_key("acct-1", "acme/app");
    assert_eq!(a, scope_key(" ACCT-1 ", "Acme/App"), "case does not matter");
    assert_ne!(a, scope_key("acct-2", "acme/app"), "another account");
    assert_ne!(a, scope_key("acct-1", "acme/other"), "another repository");
    assert_ne!(
        scope_key("a", "b/c"),
        scope_key("ab", "/c"),
        "the parts are separated"
    );
    assert_eq!(a.as_str().len(), 32);
    assert_ne!(hash_for_event("acme/app"), hash_for_event("acme/other"));
}

#[test]
fn toolchain_keys_follow_the_tools_the_store_depends_on() {
    let mut v = BTreeMap::new();
    v.insert("node".to_string(), "22.1".to_string());
    v.insert("pnpm".to_string(), "9.0".to_string());
    v.insert("xcode".to_string(), "26.0".to_string());
    let pnpm = store("pnpm").unwrap();
    let k1 = toolchain_key(pnpm, &v);
    v.insert("node".to_string(), "22.2".to_string());
    assert_ne!(k1, toolchain_key(pnpm, &v));
    assert_eq!(toolchain_key(store("cargo").unwrap(), &v), "any");
    let sp1 = toolchain_key(store("swiftpm").unwrap(), &v);
    v.insert("xcode".to_string(), "26.1".to_string());
    assert_ne!(sp1, toolchain_key(store("swiftpm").unwrap(), &v));
    assert!(store("npm").is_none());
}

#[test]
fn job_env_is_shell_safe_and_lists_the_stores() {
    let plan = plan_for(&meta("push", "refs/heads/main", None, true), &cfg());
    let env = job_env(
        &plan,
        "/Volumes/My Shared Files/abc",
        &[("pnpm".into(), "t1".into()), ("cargo".into(), "any".into())],
        Some("/Volumes/My Shared Files/m"),
    );
    assert!(env.contains("CUA_CI_CACHE_SCOPE='default_branch'"));
    assert!(env.contains("CUA_CI_CACHE_ROOT='/Volumes/My Shared Files/abc'"));
    assert!(env.contains("CUA_CI_CACHE_STORES='pnpm/t1 cargo/any'"));
    assert!(env.contains("CUA_CI_CACHE_WRITE=1"));
    assert!(env.contains("CUA_CI_MIRROR='/Volumes/My Shared Files/m'"));
    let none = job_env(&CachePlan::none(Trust::Unknown), "/x", &[], None);
    assert!(none.contains("CUA_CI_CACHE_WRITE=0"));
    assert!(!none.contains("CUA_CI_MIRROR"));
}

#[test]
fn guest_mounts_by_os() {
    assert_eq!(
        guest_mount_path(GuestOs::Macos, "k"),
        "/Volumes/My Shared Files/k"
    );
    assert_eq!(guest_mount_path(GuestOs::Linux, "k"), "/cua/ci-cache/k");
}

#[cfg(unix)]
mod fs {
    use super::*;
    use std::process::Command;

    fn tar_of(dir: &Path, build: impl FnOnce(&Path)) -> PathBuf {
        let src = dir.join("delta-src");
        std::fs::create_dir_all(&src).unwrap();
        build(&src);
        let tar = dir.join("delta.tar");
        let st = Command::new("tar")
            .arg("-cf")
            .arg(&tar)
            .arg("-C")
            .arg(&src)
            .arg(".")
            .status()
            .unwrap();
        assert!(st.success());
        std::fs::remove_dir_all(&src).unwrap();
        tar
    }

    #[test]
    fn promote_builds_the_next_version_beside_the_current_one() {
        let d = tempfile::tempdir().unwrap();
        let root = CacheRoot::new(d.path().join("cache"));
        let key = scope_key("a", "o/r");
        let t1 = tar_of(d.path(), |s| {
            std::fs::write(s.join("a.crate"), b"AAAA").unwrap()
        });
        let p = root
            .promote(
                &key,
                "cargo",
                "any",
                &[("registry", &t1)],
                PromoteLimits::default(),
            )
            .unwrap();
        assert_eq!((p.files, p.bytes), (1, 4));
        let v1 = root.current(&key, "cargo", "any").unwrap();
        assert!(v1.join("registry/a.crate").is_file());

        // A reader holds v1 open; the next promotion keeps its files.
        std::thread::sleep(Duration::from_millis(5));
        let t2 = tar_of(d.path(), |s| {
            std::fs::write(s.join("b.crate"), b"BB").unwrap()
        });
        root.promote(
            &key,
            "cargo",
            "any",
            &[("registry", &t2)],
            PromoteLimits::default(),
        )
        .unwrap();
        let v2 = root.current(&key, "cargo", "any").unwrap();
        assert_ne!(v1, v2);
        assert!(
            v2.join("registry/a.crate").is_file(),
            "the new version holds the old files"
        );
        assert!(v2.join("registry/b.crate").is_file());
        assert!(
            v1.join("registry/a.crate").is_file(),
            "the old version is untouched"
        );
        assert!(!v1.join("registry/b.crate").exists());
        assert_eq!(
            root.available(&key, &["cargo".into(), "pnpm".into()], &BTreeMap::new()),
            vec![("cargo".to_string(), "any".to_string())]
        );
        // Another repository sees nothing.
        assert!(
            root.current(&scope_key("a", "o/other"), "cargo", "any")
                .is_none()
        );
    }

    #[test]
    fn a_poisoned_delta_is_refused_and_the_cache_is_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let root = CacheRoot::new(d.path().join("cache"));
        let key = scope_key("a", "o/r");
        let good = tar_of(d.path(), |s| std::fs::write(s.join("f"), b"ok").unwrap());
        root.promote(
            &key,
            "sccache",
            "any",
            &[("objects", &good)],
            PromoteLimits::default(),
        )
        .unwrap();
        let before = root.current(&key, "sccache", "any").unwrap();

        // An absolute link.
        let abs = tar_of(d.path(), |s| {
            std::os::unix::fs::symlink("/etc/passwd", s.join("l")).unwrap()
        });
        let e = root
            .promote(
                &key,
                "sccache",
                "any",
                &[("objects", &abs)],
                PromoteLimits::default(),
            )
            .unwrap_err();
        assert!(matches!(e, PromoteError::Unsafe(_)), "{e}");
        // A link that climbs out.
        let up = tar_of(d.path(), |s| {
            std::fs::create_dir_all(s.join("a/b")).unwrap();
            std::os::unix::fs::symlink("../../../../x", s.join("a/b/l")).unwrap();
        });
        let e = root
            .promote(
                &key,
                "sccache",
                "any",
                &[("objects", &up)],
                PromoteLimits::default(),
            )
            .unwrap_err();
        assert!(matches!(e, PromoteError::Unsafe(_)), "{e}");
        // A link that stays inside is fine.
        let ok = tar_of(d.path(), |s| {
            std::fs::create_dir_all(s.join("a")).unwrap();
            std::fs::write(s.join("a/t"), b"t").unwrap();
            std::os::unix::fs::symlink("../a/t", s.join("a/l")).unwrap();
        });
        root.promote(
            &key,
            "sccache",
            "any",
            &[("objects", &ok)],
            PromoteLimits::default(),
        )
        .unwrap();
        // Over the limits.
        let big = tar_of(d.path(), |s| {
            std::fs::write(s.join("big"), vec![0u8; 4096]).unwrap()
        });
        let e = root
            .promote(
                &key,
                "sccache",
                "any",
                &[("objects", &big)],
                PromoteLimits {
                    max_bytes: 100,
                    max_files: 10,
                },
            )
            .unwrap_err();
        assert!(matches!(e, PromoteError::TooLarge(_)), "{e}");
        let e = root
            .promote(
                &key,
                "sccache",
                "any",
                &[("objects", &big)],
                PromoteLimits {
                    max_bytes: 1 << 20,
                    max_files: 0,
                },
            )
            .unwrap_err();
        assert!(matches!(e, PromoteError::TooLarge(_)), "{e}");
        // A file that is not a tar.
        let junk = d.path().join("junk");
        std::fs::write(&junk, b"not a tar").unwrap();
        assert!(
            root.promote(
                &key,
                "sccache",
                "any",
                &[("objects", &junk)],
                PromoteLimits::default()
            )
            .is_err()
        );
        // No pending folder is left, and an unknown store is refused.
        let base = root.scope_dir(&key).join("sccache/any");
        assert!(
            std::fs::read_dir(&base)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(".incoming"))
        );
        assert!(
            root.promote(
                &key,
                "npm",
                "any",
                &[("objects", &good)],
                PromoteLimits::default()
            )
            .is_err()
        );
        // A folder the store does not have is refused before anything is written.
        let e = root
            .promote(
                &key,
                "sccache",
                "any",
                &[("../escape", &good)],
                PromoteLimits::default(),
            )
            .unwrap_err();
        assert!(matches!(e, PromoteError::Unsafe(_)), "{e}");
        assert!(before.exists());
    }

    #[test]
    fn old_versions_are_collected() {
        let d = tempfile::tempdir().unwrap();
        let root = CacheRoot::new(d.path().join("cache"));
        let key = scope_key("a", "o/r");
        for i in 0..4u8 {
            let t = tar_of(d.path(), |s| {
                std::fs::write(s.join(format!("f{i}")), [i]).unwrap()
            });
            root.promote(
                &key,
                "pnpm",
                "tc",
                &[("store", &t)],
                PromoteLimits::default(),
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(3));
        }
        let versions = std::fs::read_dir(root.scope_dir(&key).join("pnpm/tc"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("v-"))
            .count();
        assert_eq!(versions, 2);
    }

    #[test]
    fn eviction_is_least_recently_used_and_respects_the_lease() {
        let d = tempfile::tempdir().unwrap();
        let root = CacheRoot::new(d.path().join("cache"));
        let mk = |name: &str, bytes: usize, age_s: u64| {
            let p = d.path().join("cache").join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("data"), vec![1u8; bytes]).unwrap();
            let stamp = p.join(".last-used");
            std::fs::write(&stamp, b"").unwrap();
            let t = SystemTime::now() - Duration::from_secs(age_s);
            std::fs::File::options()
                .write(true)
                .open(&stamp)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        mk("old", 1000, 3600);
        mk("mid", 1000, 1800);
        mk("new", 1000, 1);
        // Cap 1500: "old" goes, then "mid"; "new" is within its lease.
        let gone = root.evict(1500, Duration::from_secs(600));
        assert_eq!(gone, vec!["old".to_string(), "mid".to_string()]);
        assert!(d.path().join("cache/new").exists());
        // Under the cap nothing more goes, and a leased scope survives a zero cap.
        assert!(root.evict(10_000, Duration::from_secs(600)).is_empty());
        assert!(root.evict(0, Duration::from_secs(600)).is_empty());
    }
}
