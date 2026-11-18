use super::*;

const T0: u64 = 1_800_000_000_000;

fn timers() -> (tempfile::TempDir, Timers, EventLog) {
    let d = tempfile::tempdir().unwrap();
    let log = EventLog::new(d.path().join("p/events.jsonl"));
    (d, Timers::new(log.clone()), log)
}

fn kinds(log: &EventLog) -> Vec<EventKind> {
    log.read().iter().map(|e| e.kind).collect()
}

#[test]
fn a_whole_job_gives_every_phase() {
    let (_d, t, log) = timers();
    t.job_queued(T0, "11/22", "acme/app", Some(T0 - 500));
    t.vm_claimed(
        T0 + 100,
        "s1",
        Some("11/22"),
        GuestOs::Macos,
        "arm64",
        StartMode::WarmPool,
        Some(T0),
        Some(T0 - 500),
    );
    t.vm_ready(T0 + 100, "s1", Some("2.338.2"));
    t.runner_registered(
        Event::new(T0 + 1_100, EventKind::RunnerRegistered).runner(Some("r"), Some(7)),
        "s1",
    );
    t.job_assigned(
        Event::new(T0 + 1_800, EventKind::JobAssigned).ms(700),
        "s1",
        Some(false),
    );
    let marks = GuestMarks {
        job_start_ms: Some(T0 + 2_400),
        job_end_ms: Some(T0 + 62_400),
        restores: vec![RestoreMark {
            at_ms: T0 + 2_500,
            store: "swiftpm".into(),
            ms: 900,
            bytes: 10,
            hit: true,
            scope: "default_branch".into(),
        }],
        ..Default::default()
    };
    t.guest_marks("s1", &marks);
    t.checkout_done(T0 + 3_000, "s1", 400, true);
    let mut fin = Event::new(T0 + 63_000, EventKind::JobFinished).reason("job_done");
    fin.exit_code = Some(0);
    fin.job = Some("build".into());
    t.job_finished(fin, "s1", Some("Succeeded"));
    let done = t.teardown_done(T0 + 64_000, "s1", "job_done");
    let p = done.phases();
    assert_eq!(p.queue_wait, Some(2_300), "from GitHub's created_at");
    assert_eq!(p.vm_claim, Some(100));
    assert_eq!(p.vm_boot, Some(0));
    assert_eq!(p.runner_register, Some(1_000));
    assert_eq!(p.assign, Some(700));
    assert_eq!(p.job_start, Some(600));
    assert_eq!(p.start_latency, Some(2_900));
    assert_eq!(p.checkout, Some(400));
    assert_eq!(p.cache_restore_total, Some(900));
    assert_eq!(
        (done.cache_hits, done.cache_misses, done.cache_bytes),
        (1, 0, 10)
    );
    assert_eq!(p.job, Some(60_000));
    assert_eq!(
        p.teardown,
        Some(1_600),
        "from the job-completed hook, not the exit"
    );
    assert_eq!(p.wall, Some(64_500));
    assert!(t.timeline("s1").is_none(), "forgotten after teardown");

    let ev = log.read();
    use EventKind as K;
    assert_eq!(
        kinds(&log),
        vec![
            K::JobQueued,
            K::VmClaimed,
            K::VmReady,
            K::RunnerRegistered,
            K::JobAssigned,
            K::JobStarted,
            K::CacheRestore,
            K::CheckoutDone,
            K::JobFinished,
            K::TeardownDone
        ]
    );
    let claimed = ev.iter().find(|e| e.kind == K::VmClaimed).unwrap();
    assert_eq!(claimed.extra["start_mode"], "warm_pool");
    assert_eq!(claimed.extra["class"], "macos");
    assert_eq!(claimed.extra["arch"], "arm64");
    assert_eq!(claimed.extra["job_key"], "11/22");
    let fin = ev.iter().find(|e| e.kind == K::JobFinished).unwrap();
    assert_eq!(fin.result.as_deref(), Some("success"));
    assert_eq!(fin.extra["runner_result"], "Succeeded");
    assert_eq!(
        fin.ts_ms,
        T0 + 62_400,
        "the job-completed hook's mark, not the runner's exit"
    );
    assert_eq!(fin.ms, Some(60_000));
    assert_eq!(fin.job.as_deref(), Some("build"));
    let co = ev.iter().find(|e| e.kind == K::CheckoutDone).unwrap();
    assert_eq!(co.extra["mirror"], true);
    let cr = ev.iter().find(|e| e.kind == K::CacheRestore).unwrap();
    assert_eq!(cr.extra["store"], "swiftpm");
    assert_eq!(cr.extra["scope"], "default_branch");
    assert_eq!(cr.extra["hit"], true);
    let a = ev.iter().find(|e| e.kind == K::JobAssigned).unwrap();
    assert_eq!(a.extra["fork_pr"], false);
}

#[test]
fn missing_endpoints_are_none_not_zero() {
    let t = JobTimeline {
        claimed_ms: Some(10),
        ..Default::default()
    };
    let p = t.phases();
    assert_eq!(p, Phases::default());
    let t = JobTimeline {
        queued_ms: Some(100),
        assigned_ms: Some(160),
        ..Default::default()
    };
    assert_eq!(
        t.phases().queue_wait,
        Some(60),
        "falls back to controller receipt"
    );
}

#[test]
fn guest_marks_are_shifted_onto_the_host_clock() {
    let text = r#"{"k":"job_start","at":1000500,"event":"push","repo":"o/r","ref":"refs/heads/main","bench":"b1"}
not json
{"k":"cache_restore","at":1000900,"store":"pnpm","ms":120,"bytes":4096,"hit":true,"scope":"repo"}
{"k":"mirror","used":true}
{"k":"job_end","at":1060500}
{"k":"unknown"}
"#;
    // The guest's clock runs 500 ms ahead of the host's.
    let m = parse_guest_marks(text, 500);
    assert_eq!(m.job_start_ms, Some(1_000_000));
    assert_eq!(m.job_end_ms, Some(1_060_000));
    assert_eq!(m.event.as_deref(), Some("push"));
    assert_eq!(m.repo.as_deref(), Some("o/r"));
    assert_eq!(m.git_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(m.bench_run_id.as_deref(), Some("b1"));
    assert_eq!(m.mirror_ready, Some(true));
    assert_eq!(m.restores.len(), 1);
    assert_eq!(m.restores[0].at_ms, 1_000_400);
    assert_eq!(m.restores[0].bytes, 4096);
    // A negative shift never goes below zero.
    assert_eq!(
        parse_guest_marks(r#"{"k":"job_end","at":5}"#, 100).job_end_ms,
        Some(0)
    );
}

#[test]
fn iso_stamps() {
    assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso_ms("1970-01-02T00:00:01.5Z"), Some(86_401_500));
    assert_eq!(
        parse_iso_ms("2026-10-28T12:00:01.123456Z"),
        Some(1_793_188_801_123)
    );
    assert_eq!(parse_iso_ms("2000-03-01 00:00:00"), Some(951_868_800_000));
    assert_eq!(parse_iso_ms("nonsense"), None);
}

#[test]
fn step_telemetry_gives_the_checkout_duration() {
    let log = r#"[2026-10-28 12:00:00Z INFO Worker] something else
[2026-10-28 12:00:03Z INFO ExecutionContext] Publish step telemetry for current step {"action":"actions/checkout","displayName":"Run actions/checkout@v4","startTime":"2026-10-28T12:00:01.000Z","finishTime":"2026-10-28T12:00:02.500Z","result":"Succeeded"}
[2026-10-28 12:00:09Z INFO ExecutionContext] Publish step telemetry for current step {"displayName":"Build","startTime":"2026-10-28T12:00:03Z","finishTime":"2026-10-28T12:00:09Z","result":"Succeeded"}
[2026-10-28 12:00:09Z INFO ExecutionContext] Publish step telemetry for current step {broken
"#;
    let steps = parse_step_telemetry(log);
    assert_eq!(steps.len(), 2);
    assert!(steps[0].is_checkout());
    assert_eq!(steps[0].ms(), 1_500);
    assert!(!steps[1].is_checkout());
    assert_eq!(steps[1].ms(), 6_000);
}

#[test]
fn results_are_the_bench_words() {
    assert_eq!(normalize_result("Succeeded"), "success");
    assert_eq!(normalize_result("Failed"), "failure");
    assert_eq!(normalize_result("Canceled"), "cancelled");
    assert_eq!(normalize_result("whatever"), "failure");
}

#[test]
fn infra_failures_and_sweeps_are_events() {
    let (_d, t, log) = timers();
    t.infra_failure(T0, None, "create failed: boom");
    t.resource_sweep(T0 + 1, 0, 0, "");
    let ev = log.read();
    assert_eq!(ev[0].kind, EventKind::InfraFailure);
    assert_eq!(ev[1].extra["leaked_vms"], 0);
    assert_eq!(ev[1].extra["leaked_cloud"], 0);
}
