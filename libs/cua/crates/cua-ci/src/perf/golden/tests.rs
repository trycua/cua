use super::*;
use crate::testing::{FakeSpaces, FakeTarballs, ManualClock};

const T0: u64 = 1_800_000_000_000;
const H: u64 = 3_600_000;

fn spec() -> GoldenSpec {
    let mut s = GoldenSpec::basic(
        GuestOs::Macos,
        "arm64",
        "ghcr.io/trycua/macos:26",
        "2.338.2",
        true,
    );
    s.xcode = Some("26.0.1".into());
    s.tools.insert("node".into(), "22".into());
    s.tools.insert("pnpm".into(), "10.4.1".into());
    s.tools.insert("rust".into(), "1.90.0".into());
    s.lockfiles.insert("pnpm-lock.yaml".into(), "aa".into());
    s
}

fn record(spec: &GoldenSpec, built: u64) -> GoldenRecord {
    GoldenRecord {
        spec: spec.clone(),
        digest: spec.digest(),
        checkpoints: (0..spec.variants())
            .map(|i| spec.checkpoint_name(i))
            .collect(),
        built_ms: built,
        verified_ms: Some(built),
        build_ms: 600_000,
        fork_ready_ms: Some(2_000),
        versions: BTreeMap::new(),
    }
}

fn state(rec: GoldenRecord) -> GoldenState {
    GoldenState {
        current: Some(rec),
        ..Default::default()
    }
}

#[test]
fn the_digest_names_everything_in_the_image() {
    let a = spec();
    assert_eq!(a.digest(), spec().digest(), "stable");
    assert_eq!(a.short().len(), 12);
    let mut b = spec();
    b.xcode = Some("26.1".into());
    assert_ne!(a.digest(), b.digest());
    let mut c = spec();
    c.lockfiles.insert("Cargo.lock".into(), "bb".into());
    assert_ne!(a.digest(), c.digest());
    let mut d = spec();
    d.base_digest = Some("sha256:1".into());
    assert_ne!(a.digest(), d.digest());
    // The checkpoint names carry the os, arch, digest and variant.
    let n = a.checkpoint_name(1);
    assert!(n.starts_with("cua-ci-golden-macos-arm64-"));
    assert!(n.ends_with("-v1") && n.contains(&a.short()));
    assert_eq!(
        a.fork_image(0),
        format!("checkpoint:{}", a.checkpoint_name(0))
    );
    // macOS gets a checkpoint per job slot, Linux one.
    assert_eq!(a.variants(), 2);
    let mut l = spec();
    l.os = GuestOs::Linux;
    assert_eq!(l.variants(), 1);
}

#[test]
fn changes_are_named_and_only_lockfiles_wait() {
    let a = spec();
    let mut b = spec();
    b.xcode = Some("26.1".into());
    b.tools.insert("node".into(), "24".into());
    b.lockfiles.insert("pnpm-lock.yaml".into(), "cc".into());
    b.runner_version = "2.339.0".into();
    let ch = b.changes_from(&a);
    assert!(ch.contains(&Change::Xcode));
    assert!(ch.contains(&Change::Tool("node".into())));
    assert!(ch.contains(&Change::Lockfile("pnpm-lock.yaml".into())));
    assert!(ch.contains(&Change::Runner));
    assert!(ch.iter().filter(|c| c.is_urgent()).count() == 3);
    assert!(!Change::Lockfile("x".into()).is_urgent());
    assert!(Change::Xcode.describe().contains("Xcode"));
    assert!(spec().changes_from(&spec()).is_empty());
    // A tool that appears or disappears is a change too.
    let mut c = spec();
    c.tools.remove("rust");
    assert_eq!(c.changes_from(&a), vec![Change::Tool("rust".into())]);
    // The VM size is part of the golden image (a golden VM forks into jobs of
    // its own size), so a new size is urgent and says so.
    let mut d = spec();
    d.tools.insert("vm_memory_mb".into(), "8192".into());
    let ch = d.changes_from(&a);
    assert_eq!(ch, vec![Change::Tool("vm_memory_mb".into())]);
    assert!(ch[0].is_urgent());
    assert_eq!(ch[0].describe(), "the VM size changed");
    assert_ne!(d.digest(), a.digest());
}

#[test]
fn a_current_image_is_kept() {
    let s = spec();
    let st = state(record(&s, T0));
    let existing = st.current.as_ref().unwrap().checkpoints.clone();
    assert_eq!(
        assess(&s, &st, T0 + H, Cadence::default(), &existing),
        Verdict::Fresh
    );
}

#[test]
fn a_missing_image_is_built_and_a_failed_build_waits() {
    let s = spec();
    let c = Cadence::default();
    assert_eq!(
        assess(&s, &GoldenState::default(), T0, c, &[]),
        Verdict::Rebuild(Why::Missing)
    );
    let failed = GoldenState {
        last_attempt_ms: T0,
        ..Default::default()
    };
    assert_eq!(
        assess(&s, &failed, T0 + 60_000, c, &[]),
        Verdict::Wait,
        "inside the retry pause"
    );
    assert_eq!(
        assess(&s, &failed, T0 + 31 * 60_000, c, &[]),
        Verdict::Rebuild(Why::Missing)
    );
}

#[test]
fn toolchain_changes_rebuild_at_once_lockfile_changes_after_a_day() {
    let old = spec();
    let st = state(record(&old, T0));
    let existing = st.current.as_ref().unwrap().checkpoints.clone();
    let c = Cadence::default();

    let mut xcode = spec();
    xcode.xcode = Some("26.1".into());
    match assess(&xcode, &st, T0 + 60_000, c, &existing) {
        Verdict::Rebuild(Why::Changed(ch)) => assert_eq!(ch, vec![Change::Xcode]),
        v => panic!("{v:?}"),
    }
    let mut lock = spec();
    lock.lockfiles.insert("pnpm-lock.yaml".into(), "new".into());
    assert_eq!(assess(&lock, &st, T0 + H, c, &existing), Verdict::Wait);
    assert!(matches!(
        assess(&lock, &st, T0 + 25 * H, c, &existing),
        Verdict::Rebuild(Why::Changed(_))
    ));
}

#[test]
fn age_a_gone_checkpoint_and_repeated_fork_failures_rebuild() {
    let s = spec();
    let c = Cadence::default();
    let st = state(record(&s, T0));
    let existing = st.current.as_ref().unwrap().checkpoints.clone();
    assert_eq!(
        assess(&s, &st, T0 + 8 * 24 * H, c, &existing),
        Verdict::Rebuild(Why::Age)
    );
    // One variant's checkpoint was pruned.
    let gone = vec![existing[0].clone()];
    assert_eq!(
        assess(&s, &st, T0 + H, c, &gone),
        Verdict::Rebuild(Why::CheckpointGone(existing[1].clone()))
    );
    // Forks failing twice is not enough, three times is.
    let mut st2 = st.clone();
    st2.fork_failures = FORK_FAILURES_TO_REBUILD - 1;
    assert_eq!(assess(&s, &st2, T0 + H, c, &existing), Verdict::Fresh);
    st2.fork_failures = FORK_FAILURES_TO_REBUILD;
    assert_eq!(
        assess(&s, &st2, T0 + H, c, &existing),
        Verdict::Rebuild(Why::ForkFailed)
    );
}

#[test]
fn variants_are_spread_over_the_slots() {
    assert_eq!(pick_variant(&[], 2), 0);
    assert_eq!(pick_variant(&[0], 2), 1);
    assert_eq!(pick_variant(&[1], 2), 0);
    assert_eq!(pick_variant(&[0, 1], 2), 0, "all taken: the first");
    assert_eq!(pick_variant(&[3], 1), 0);
}

#[test]
fn fixup_sets_the_clock_and_clears_the_markers() {
    let m = fork_fixup_script(1_800_000_000, GuestOs::Macos);
    assert!(m.starts_with("# cua-ci:fixup\n"));
    assert!(m.contains("date -u -f %s 1800000000"));
    assert!(m.contains("ipconfig set en0 DHCP"));
    assert!(m.contains(".cua-exit") && m.trim_end().ends_with("exit 0"));
    let l = fork_fixup_script(1_800_000_000, GuestOs::Linux);
    assert!(l.contains("date -u -s @1800000000"));
}

#[test]
fn versions_and_pins() {
    let v = parse_versions("xcode=26.0.1\nnode=22.11.0\n\nbad\npnpm=10.4.1\nempty=\n");
    assert_eq!(v["xcode"], "26.0.1");
    assert_eq!(v.len(), 3);
    check_versions(&spec(), &v).unwrap();
    let mut wrong = v.clone();
    wrong.insert("xcode".into(), "26.0".into());
    let e = check_versions(&spec(), &wrong).unwrap_err().to_string();
    assert!(e.contains("xcode 26.0 (wanted 26.0.1)"), "{e}");
    wrong.insert("node".into(), "20.1".into());
    assert!(
        check_versions(&spec(), &wrong)
            .unwrap_err()
            .to_string()
            .contains("node 20.1")
    );
    // A version the image did not report is not a mismatch.
    check_versions(&spec(), &BTreeMap::new()).unwrap();
}

#[test]
fn the_recipe_gets_its_pins_as_exports() {
    let env = spec().recipe_env();
    assert!(env.contains("export CUA_CI_XCODE='26.0.1'"));
    assert!(env.contains("export CUA_CI_NODE='22'"));
    assert!(env.contains("export CUA_CI_PNPM='10.4.1'"));
    assert!(env.contains("export CUA_CI_RUST='1.90.0'"));
    assert!(MACOS_RECIPE.contains("set -euo pipefail"));
}

#[test]
fn lockfile_digests_hash_the_files_that_exist() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("pnpm-lock.yaml");
    std::fs::write(&a, b"lock").unwrap();
    let m = lockfile_digests(&[a.as_path(), d.path().join("missing").as_path()]);
    assert_eq!(m.len(), 1);
    let h = m.values().next().unwrap();
    assert_eq!(h.len(), 64);
    std::fs::write(&a, b"lock2").unwrap();
    assert_ne!(
        &lockfile_digests(&[a.as_path()]).values().next().unwrap(),
        &h
    );
}

#[test]
fn the_state_file_round_trips_and_a_bad_one_is_empty() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("perf/golden.json");
    let st = state(record(&spec(), T0));
    st.save(&p).unwrap();
    assert_eq!(GoldenState::load(&p), st);
    assert_eq!(st.key(), Some(spec().digest()));
    std::fs::write(&p, "{nonsense").unwrap();
    assert_eq!(GoldenState::load(&p), GoldenState::default());
    assert_eq!(
        GoldenState::load(&d.path().join("none")),
        GoldenState::default()
    );
}

struct H2 {
    _dir: tempfile::TempDir,
    spaces: Arc<FakeSpaces>,
    clock: Arc<ManualClock>,
    refresh: Refresh,
}

fn harness() -> H2 {
    let dir = tempfile::tempdir().unwrap();
    let spaces = Arc::new(FakeSpaces::new());
    let clock = Arc::new(ManualClock::new(T0));
    let refresh = Refresh {
        spaces: spaces.clone(),
        tarballs: Arc::new(FakeTarballs::new(dir.path())),
        clock: clock.clone(),
        on: "local".into(),
        size: (Some(5), Some(24_576)),
    };
    H2 {
        _dir: dir,
        spaces,
        clock,
        refresh,
    }
}

#[tokio::test]
async fn a_refresh_builds_checkpoints_verifies_a_fork_and_cleans_up() {
    let h = harness();
    let s = spec();
    let done = refresh(&h.refresh, &s, None, &|_| false).await.unwrap();
    assert_eq!(done.record.digest, s.digest());
    assert_eq!(
        done.record.checkpoints,
        vec![s.checkpoint_name(0), s.checkpoint_name(1)]
    );
    assert!(done.record.verified_ms.is_some() && done.record.fork_ready_ms.is_some());
    assert!(done.retire.is_empty());
    // Both checkpoints exist; no build or verification Space is left.
    assert_eq!(h.spaces.checkpoints.lock().unwrap().len(), 2);
    assert!(h.spaces.ids().is_empty(), "{:?}", h.spaces.ids());
    // The build VM was sized like the job VMs; the recipe ran with the pins.
    let calls = h.spaces.calls();
    assert!(calls.iter().any(|c| matches!(c, crate::testing::Call::Sh(_, sc) if sc.starts_with("# cua-ci:recipe") && sc.contains("CUA_CI_XCODE"))));
    assert!(
        calls.iter().any(
            |c| matches!(c, crate::testing::Call::Sh(_, sc) if sc.starts_with("# cua-ci:trim"))
        )
    );
    // The hooks and the runner went into the image before the checkpoint.
    assert!(calls.iter().any(
        |c| matches!(c, crate::testing::Call::Write(_, p, 0o755) if p.ends_with("job-started.sh"))
    ));
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, crate::testing::Call::Upload(..)))
    );
    // The fork ran the fixup.
    assert!(
        calls.iter().any(
            |c| matches!(c, crate::testing::Call::Sh(_, sc) if sc.starts_with("# cua-ci:fixup"))
        )
    );
}

#[tokio::test]
async fn the_old_image_is_retired_only_when_unused() {
    let h = harness();
    let old = spec();
    let rec = record(&old, T0 - 10 * H);
    h.spaces
        .checkpoints
        .lock()
        .unwrap()
        .extend(rec.checkpoints.clone());
    let mut new = spec();
    new.xcode = Some("26.1".into());
    let busy = rec.checkpoints[0].clone();
    let done = refresh(&h.refresh, &new, Some(&rec), &move |n| n == busy)
        .await
        .unwrap();
    assert_eq!(
        done.retire,
        vec![rec.checkpoints[1].clone()],
        "the one a VM still forks from stays"
    );
    assert_ne!(done.record.digest, rec.digest);
    h.clock.advance(Duration::from_secs(1));
}

#[tokio::test]
async fn a_failed_recipe_leaves_nothing_behind() {
    let h = harness();
    *h.spaces.recipe_error.lock().unwrap() =
        Some("the base image has Xcode 26.0, the spec wants 26.0.1".into());
    let e = refresh(&h.refresh, &spec(), None, &|_| false)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("the golden recipe failed") && e.contains("wants 26.0.1"),
        "{e}"
    );
    assert!(h.spaces.ids().is_empty(), "the build VM is deleted");
    assert!(h.spaces.checkpoints.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_checkpoint_that_cannot_be_taken_fails_the_refresh_cleanly() {
    let h = harness();
    *h.spaces.checkpoint_error.lock().unwrap() = Some("this runtime cannot checkpoint".into());
    let e = refresh(&h.refresh, &spec(), None, &|_| false)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("cannot checkpoint"), "{e}");
    assert!(h.spaces.ids().is_empty());
}

#[tokio::test]
async fn a_fork_that_fails_verification_deletes_the_new_checkpoints() {
    let h = harness();
    // The fork of the new checkpoint cannot start: pretend the engine lost it
    // between the checkpoint and the verification fork.
    let s = spec();
    let first = s.checkpoint_name(0);
    struct Lossy(Arc<FakeSpaces>, String);
    #[async_trait::async_trait]
    impl Spaces for Lossy {
        async fn create(&self, spec: &SpaceSpec) -> Result<crate::spaces::SpaceRow, SpacesError> {
            if spec.image == format!("checkpoint:{}", self.1) {
                return Err(SpacesError::Other("restore failed".into()));
            }
            self.0.create(spec).await
        }
        async fn delete(&self, id: &str) -> Result<(), SpacesError> {
            self.0.delete(id).await
        }
        async fn list(&self) -> Result<Vec<crate::spaces::SpaceRow>, SpacesError> {
            self.0.list().await
        }
        async fn sh(
            &self,
            id: &str,
            s: &str,
            t: Duration,
        ) -> Result<crate::spaces::ShOutput, SpacesError> {
            self.0.sh(id, s, t).await
        }
        async fn write_file(&self, id: &str, p: &str, d: &[u8], m: u32) -> Result<(), SpacesError> {
            self.0.write_file(id, p, d, m).await
        }
        async fn upload_file(&self, id: &str, l: &Path, p: &str) -> Result<(), SpacesError> {
            self.0.upload_file(id, l, p).await
        }
        async fn download(&self, id: &str, r: &str, l: &Path) -> Result<(), SpacesError> {
            self.0.download(id, r, l).await
        }
        async fn spawn(
            &self,
            id: &str,
            p: &str,
            a: &[String],
            c: &str,
            t: &str,
        ) -> Result<(), SpacesError> {
            self.0.spawn(id, p, a, c, t).await
        }
        async fn set_egress(
            &self,
            id: &str,
            p: &crate::egress::EgressPolicy,
        ) -> Result<(), SpacesError> {
            self.0.set_egress(id, p).await
        }
        async fn checkpoint(&self, id: &str, n: &str) -> Result<(), SpacesError> {
            self.0.checkpoint(id, n).await
        }
        async fn checkpoints(&self) -> Result<Vec<String>, SpacesError> {
            self.0.checkpoints().await
        }
        async fn delete_checkpoint(&self, n: &str) -> Result<(), SpacesError> {
            self.0.delete_checkpoint(n).await
        }
    }
    let r = Refresh {
        spaces: Arc::new(Lossy(h.spaces.clone(), first)),
        tarballs: h.refresh.tarballs.clone(),
        clock: h.clock.clone(),
        on: "local".into(),
        size: (None, None),
    };
    let e = refresh(&r, &s, None, &|_| false)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("forking the new golden image failed"), "{e}");
    assert!(
        h.spaces.checkpoints.lock().unwrap().is_empty(),
        "no half-verified image stays"
    );
    assert!(h.spaces.ids().is_empty());
}

#[tokio::test]
async fn a_linux_image_makes_one_checkpoint() {
    let h = harness();
    let mut s = spec();
    s.os = GuestOs::Linux;
    s.base_image = "ghcr.io/trycua/linux:24.04".into();
    let done = refresh(&h.refresh, &s, None, &|_| false).await.unwrap();
    assert_eq!(done.record.checkpoints.len(), 1);
}
