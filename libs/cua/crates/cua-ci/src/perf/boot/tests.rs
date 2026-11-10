use super::*;
#[cfg(unix)]
use crate::perf::timers::parse_guest_marks;

#[cfg(unix)]
fn bash_n(script: &str) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.sh");
    std::fs::write(&p, script).unwrap();
    let st = std::process::Command::new("bash")
        .arg("-n")
        .arg(&p)
        .status()
        .unwrap();
    assert!(st.success(), "bash -n failed for:\n{script}");
}

#[cfg(unix)]
#[test]
fn every_script_is_valid_bash() {
    bash_n(&job_started_hook());
    bash_n(&job_completed_hook());
    bash_n(&delta_script("/Users/lume", true));
    bash_n(&delta_script("/Users/lume", false));
    bash_n(&collect_script("/Users/lume"));
    bash_n(&secret_scan_script());
    bash_n(&trim_script(GuestOs::Macos));
    bash_n(&trim_script(GuestOs::Linux));
    bash_n(&wrapper_exports());
}

#[test]
fn hooks_never_fail_a_job_and_the_table_is_the_one_source() {
    for h in [job_started_hook(), job_completed_hook()] {
        assert!(h.trim_end().ends_with("exit 0"));
        assert!(h.starts_with("#!/bin/bash"));
        assert!(h.contains("set +e"));
    }
    let started = job_started_hook();
    for st in STORES.iter() {
        assert!(started.contains(&format!("{})", st.name)), "{}", st.name);
        for (sub, dest) in st.dirs {
            assert!(started.contains(&format!("{sub}:{dest}")), "{sub}");
        }
    }
    let w = wrapper_exports();
    assert!(w.contains("ACTIONS_RUNNER_HOOK_JOB_STARTED"));
    assert!(w.contains("ACTIONS_RUNNER_HOOK_JOB_COMPLETED"));
    assert!(w.contains("npm_config_store_dir"));
    assert!(w.contains("GIT_ALTERNATE_OBJECT_DIRECTORIES"));
    assert!(
        w.contains("[ -d \"$CUA_CI_MIRROR/repo.git/objects\" ]"),
        "only when the mirror is there"
    );
    assert_eq!(baked_files("/h", true).len(), 2);
    assert_eq!(
        job_started_path("/Users/lume/"),
        "/Users/lume/.cua-ci/hooks/job-started.sh"
    );
}

#[test]
fn delta_output_only_names_known_store_folders() {
    let out = "dirty=cargo:registry\ndirty=pnpm:store\ndirty=cargo:../../etc\ndirty=evil:store\nnoise\ndirty=swiftpm:cache\n";
    assert_eq!(
        parse_delta(out),
        vec![
            ("cargo".to_string(), "registry".to_string()),
            ("pnpm".to_string(), "store".to_string()),
            ("swiftpm".to_string(), "cache".to_string()),
        ]
    );
}

#[test]
fn collect_output_splits_at_the_worker_marker() {
    let (marks, worker) = split_collect("{\"k\":\"job_end\",\"at\":1}\n---worker\nline1\nline2\n");
    assert_eq!(marks, "{\"k\":\"job_end\",\"at\":1}\n");
    assert_eq!(worker, "line1\nline2\n");
    assert_eq!(split_collect("only marks"), ("only marks", ""));
}

/// The hooks, run for real with a fake HOME: the started hook restores a
/// store from a read-only folder, records its timing and the job start;
/// the completed hook records the end; the delta script tars only files
/// written after the restore.
#[cfg(unix)]
#[test]
fn the_hooks_restore_record_and_find_what_changed() {
    use std::process::Command;
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let cache = d.path().join("cache");
    std::fs::create_dir_all(home.join(".cua-ci")).unwrap();
    // A cache with a cargo registry in it.
    let reg = cache.join("cargo/any/v-1/registry");
    std::fs::create_dir_all(&reg).unwrap();
    std::fs::write(reg.join("a.crate"), b"AAAA").unwrap();
    std::os::unix::fs::symlink("v-1", cache.join("cargo/any/current")).unwrap();
    std::fs::write(
        home.join(".cua-ci/job.env"),
        format!(
            "CUA_CI_CACHE_SCOPE='default_branch'\nCUA_CI_CACHE_ROOT='{}'\nCUA_CI_CACHE_STORES='cargo/any pnpm/missing'\nCUA_CI_CACHE_WRITE=1\n",
            cache.display()
        ),
    )
    .unwrap();
    let write = |name: &str, body: String| {
        let p = d.path().join(name);
        std::fs::write(&p, body).unwrap();
        p
    };
    let started = write("started.sh", job_started_hook());
    let completed = write("completed.sh", job_completed_hook());
    let delta = write("delta.sh", delta_script(home.to_str().unwrap(), true));
    let dirty_only = write("dirty.sh", delta_script(home.to_str().unwrap(), false));
    let run = |p: &Path, extra_env: &[(&str, &str)]| {
        let mut c = Command::new("bash");
        c.arg(p)
            .env("HOME", &home)
            .env("GITHUB_EVENT_NAME", "push")
            .env("GITHUB_REPOSITORY", "acme/app")
            .env("GITHUB_REF", "refs/heads/main")
            .env("CUA_CI_BENCH_RUN_ID", "bench-7");
        for (k, v) in extra_env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    run(
        &started,
        &[(
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "/mirror/repo.git/objects",
        )],
    );
    assert_eq!(
        std::fs::read(home.join(".cargo/registry/a.crate")).unwrap(),
        b"AAAA"
    );

    // A job writes a new crate, then ends.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(home.join(".cargo/registry/b.crate"), b"BB").unwrap();
    run(&completed, &[]);
    let dry = run(&dirty_only, &[]);
    assert_eq!(
        parse_delta(&dry),
        vec![("cargo".to_string(), "registry".to_string())],
        "{dry}"
    );
    assert!(
        std::fs::read_dir(home.join(".cua-ci/delta"))
            .unwrap()
            .next()
            .is_none(),
        "a dry run writes no tar"
    );
    let out = run(&delta, &[]);
    assert_eq!(
        parse_delta(&out),
        vec![("cargo".to_string(), "registry".to_string())],
        "{out}"
    );
    let tar = home.join(".cua-ci/delta/cargo--registry.tar");
    let listing = Command::new("tar").arg("-tf").arg(&tar).output().unwrap();
    let names = String::from_utf8_lossy(&listing.stdout);
    assert!(names.contains("b.crate"), "{names}");
    assert!(
        !names.contains("a.crate"),
        "restored files are not written back: {names}"
    );

    // The marks parse.
    let text = std::fs::read_to_string(home.join(".cua-ci/timers.jsonl")).unwrap();
    let m = parse_guest_marks(&text, 0);
    assert!(m.job_start_ms.is_some() && m.job_end_ms.is_some());
    assert!(m.job_end_ms >= m.job_start_ms);
    assert_eq!(m.event.as_deref(), Some("push"));
    assert_eq!(m.repo.as_deref(), Some("acme/app"));
    assert_eq!(m.bench_run_id.as_deref(), Some("bench-7"));
    assert_eq!(m.mirror_ready, Some(true));
    assert_eq!(m.restores.len(), 2);
    let cargo = m.restores.iter().find(|r| r.store == "cargo").unwrap();
    assert!(cargo.hit && cargo.bytes >= 4 && cargo.scope == "default_branch");
    let pnpm = m.restores.iter().find(|r| r.store == "pnpm").unwrap();
    assert!(!pnpm.hit, "a store with nothing to restore is a miss");
}

#[cfg(unix)]
#[test]
fn a_hook_without_a_job_env_still_exits_zero() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.sh");
    std::fs::write(&p, job_started_hook()).unwrap();
    let o = std::process::Command::new("bash")
        .arg(&p)
        .env("HOME", d.path())
        .output()
        .unwrap();
    assert!(o.status.success());
    let text = std::fs::read_to_string(d.path().join(".cua-ci/timers.jsonl")).unwrap();
    assert!(text.contains("job_start"));
}

#[cfg(unix)]
use std::path::Path;

#[test]
fn the_secret_scan_output_is_counts_and_names_only() {
    assert_eq!(
        parse_secret_scan("noise\nfound=2\ndetail=file:.aws/credentials,env:HCLOUD_TOKEN\n"),
        Some((2, "file:.aws/credentials,env:HCLOUD_TOKEN".to_string()))
    );
    assert_eq!(
        parse_secret_scan("found=0\ndetail=\n"),
        Some((0, String::new()))
    );
    assert_eq!(parse_secret_scan("nothing"), None);
    // Anything that is not a name character is dropped from the detail.
    assert_eq!(
        parse_secret_scan("found=1\ndetail=env:A B;rm -rf !\n")
            .unwrap()
            .1,
        "env:ABrm-rf"
    );
}

#[cfg(unix)]
#[test]
fn the_secret_scan_finds_a_credential_file_in_a_fake_home_and_nothing_in_a_clean_one() {
    let run = |home: &std::path::Path| {
        let o = std::process::Command::new("bash")
            .arg("-c")
            .arg(secret_scan_script())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home)
            .output()
            .unwrap();
        parse_secret_scan(&String::from_utf8_lossy(&o.stdout)).unwrap()
    };
    let clean = tempfile::tempdir().unwrap();
    // The test process's own environment is cleared, but /proc may show other
    // processes of this user on Linux: only the file checks are asserted.
    let (_, detail) = run(clean.path());
    assert!(!detail.contains("file:"), "{detail}");
    let dirty = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dirty.path().join(".aws")).unwrap();
    std::fs::write(dirty.path().join(".aws/credentials"), "x").unwrap();
    std::fs::write(dirty.path().join("key.pem"), "x").unwrap();
    let (found, detail) = run(dirty.path());
    assert!(found >= 2, "{found} {detail}");
    assert!(
        detail.contains("file:.aws/credentials") && detail.contains("pem:key.pem"),
        "{detail}"
    );
}

#[cfg(unix)]
#[test]
fn the_wrapper_puts_the_golden_tools_on_the_jobs_path() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".cargo/bin")).unwrap();
    let script = format!("{}\necho \"$PATH\"\n", wrapper_exports());
    let o = std::process::Command::new("bash")
        .arg("-c")
        .arg(script)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home.path())
        .output()
        .unwrap();
    let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let cargo = format!("{}/.cargo/bin", home.path().display());
    let parts: Vec<&str> = path.split(':').collect();
    let at = parts.iter().position(|p| *p == cargo);
    assert!(at.is_some(), "{path}");
    // In front of the system folders, once.
    assert!(
        at.unwrap() < parts.iter().position(|p| *p == "/usr/bin").unwrap(),
        "{path}"
    );
    assert_eq!(path.matches(".cargo/bin").count(), 1, "{path}");
}
