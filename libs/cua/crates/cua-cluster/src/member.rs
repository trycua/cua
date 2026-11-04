//! Running a job member's command in its Space, through the Space's
//! cua-spacesd: the part every kind shares (a fresh VM or container, or a
//! dedicated machine's own Space).
//!
//! - **Environment** ([`member_env`]): the job's own variables, the run's
//!   (`CUA_RUN_ID`, `CUA_ATTEMPT`, `CUA_RANK` 0, `CUA_WORLD_SIZE` 1,
//!   `CUA_PEERS` empty unless the coordinator set them), where things are as
//!   the member sees them (`CUA_JOB_DIR`, `CUA_JOB_DATA`, `CUA_HEARTBEAT`,
//!   `CUA_BARRIER`, `CUA_RESULT_PATH`), its kind (`CUA_SPACE_KIND`), and the
//!   caches (`UV_CACHE_DIR`, `UV_PYTHON_INSTALL_DIR`, `HF_HOME`) with `uv`
//!   first on `PATH`.
//! - **Output**: stdout and stderr, merged in arrival order, go to the
//!   attempt's `output.log` on the host as they arrive.
//! - **Stopping**: a cancel or the job's timeout sends the command's whole
//!   process group SIGTERM, then SIGKILL after
//!   [`crate::jobdir::KILL_GRACE`].
//! - **Result**: read on the host from the mounted work directory.

use std::collections::BTreeMap;
use std::time::Duration;

use cua_spacesd_client::{Command, ProcessEvent, ProcessRef, Replay, SpacesdClient, pb};
use tokio_util::sync::CancellationToken;

use crate::jobdir::{self, BARRIER, CONTROL, HEARTBEAT, JobDirs, has_barrier, run_of};
use crate::model::JobSpec;
use crate::node::ExecEnd;
use crate::peers::RoutedPeers;

/// Where a member's directories are as it sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestPaths {
    /// The work directory (the bundle, its cwd).
    pub work: String,
    /// The run's data directory.
    pub data: String,
    /// The caches (`uv/`, `python/`, `hf/` in it).
    pub cache: String,
    /// The directory with `uv` (first on `PATH`).
    pub tools: String,
    /// The rest of `PATH`.
    pub path: String,
    /// Where Python installs go when the cache's folder cannot hold them
    /// (a macOS guest's shared folders cannot `fsync`, which uv does for
    /// an install): on the Space's own disk, with the downloaded archives
    /// still cached in `<cache>/python-downloads`. `None`: in the cache.
    pub python_installs: Option<String>,
}

impl GuestPaths {
    /// Under one root, as a Linux container mounts them (`/cua/job`, ...).
    pub fn under(root: &str, path: &str) -> Self {
        Self {
            work: format!("{root}/job"),
            data: format!("{root}/data"),
            cache: format!("{root}/cache"),
            tools: format!("{root}/tools"),
            path: path.into(),
            python_installs: None,
        }
    }
}

/// `PATH` after the tools in a Linux guest.
pub const LINUX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// `PATH` after the tools in a macOS guest.
pub const MACOS_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// The job environment variable naming the member's kind.
pub const SPACE_KIND_ENV: &str = "CUA_SPACE_KIND";

/// A member's environment: `job`'s with the run's defaults, where its
/// directories are (`guest`), its kind, the caches, and its routed peers.
pub fn member_env(
    job: &JobSpec,
    guest: &GuestPaths,
    kind: &str,
    routed: Option<&RoutedPeers>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let run = run_of(job);
    // The run id without the account prefix the host keeps it under.
    let run = run.rsplit('/').next().unwrap_or(run).to_string();
    let mut defaults = vec![
        (
            "CUA_JOB_ID",
            job.job_id.rsplit('/').next().unwrap_or("").to_string(),
        ),
        ("CUA_RUN_ID", run),
        ("CUA_ATTEMPT", job.attempt.to_string()),
        ("CUA_RANK", "0".into()),
        ("CUA_WORLD_SIZE", "1".into()),
        ("CUA_PEERS", String::new()),
        ("UV_CACHE_DIR", format!("{}/uv", guest.cache)),
        (
            "UV_PYTHON_INSTALL_DIR",
            guest
                .python_installs
                .clone()
                .unwrap_or_else(|| format!("{}/python", guest.cache)),
        ),
        ("HF_HOME", format!("{}/hf", guest.cache)),
        // The cache is on another file system than the environments uv
        // makes from it (a mount): copy instead of trying to link.
        ("UV_LINK_MODE", "copy".into()),
        ("PYTHONUNBUFFERED", "1".into()),
    ];
    if guest.python_installs.is_some() {
        defaults.push((
            "UV_PYTHON_CACHE_DIR",
            format!("{}/python-downloads", guest.cache),
        ));
    }
    if !job.result_path.is_empty() {
        defaults.push((
            "CUA_RESULT_PATH",
            format!("{}/{}", guest.work, job.result_path),
        ));
    }
    for (k, v) in defaults {
        env.insert(k.to_string(), v);
    }
    for (k, v) in &job.env {
        env.insert(k.clone(), v.clone());
    }
    let path = match job.env.get("PATH") {
        Some(p) if !p.is_empty() => format!("{}:{p}", guest.tools),
        _ => format!("{}:{}", guest.tools, guest.path),
    };
    env.insert("PATH".into(), path);
    // Always the host's: where this member, its run's data and its control
    // files are, and what it runs in.
    let control = format!("{}/{CONTROL}", guest.work);
    env.insert("CUA_JOB_DIR".into(), guest.work.clone());
    env.insert("CUA_JOB_DATA".into(), guest.data.clone());
    env.insert("CUA_HEARTBEAT".into(), format!("{control}/{HEARTBEAT}"));
    env.remove("CUA_BARRIER");
    if has_barrier(job) {
        env.insert("CUA_BARRIER".into(), format!("{control}/{BARRIER}"));
    }
    env.insert(SPACE_KIND_ENV.into(), kind.into());
    if let Some(r) = routed {
        if let Some(placed) = job.env.get("CUA_PEERS") {
            env.insert("CUA_PEERS_PLACED".into(), placed.clone());
        }
        env.insert("CUA_PEERS".into(), r.peers.join(","));
    }
    env
}

/// How a member's command runs in its Space.
pub struct MemberCommand<'a> {
    /// The Space's cua-spacesd.
    pub client: &'a SpacesdClient,
    /// The job.
    pub job: &'a JobSpec,
    /// Its directories on the host.
    pub dirs: &'a JobDirs,
    /// Its environment ([`member_env`]).
    pub env: BTreeMap<String, String>,
    /// Its working directory as it sees it.
    pub cwd: String,
    /// The user it runs as (`None`: the Space's cua-spacesd's).
    pub user: Option<String>,
    /// Make what it creates open to every user (it runs as another user
    /// than the host's, and the host removes what it leaves).
    pub open_files: bool,
    /// Run as the Space's root when its user may (`sudo -n`): a shared
    /// folder whose files all belong to one owner it cannot be (a Mac's
    /// folder in the built-in Linux runtime: created files are the Mac
    /// user's, mode 0644) is writable only to root. The Space is still the
    /// isolation boundary (gVisor).
    pub as_root: bool,
}

/// The shell that sets a member's umask (and, with `as_root`, becomes the
/// Space's root when it may) before its command; the command follows as
/// `"$@"`.
pub fn wrapper(as_root: bool) -> &'static str {
    if as_root {
        "if [ \"$(id -u)\" != 0 ] && command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then \
         exec sudo -n -E env \"PATH=$PATH\" /bin/sh -c 'umask 000; exec \"$@\"' sh \"$@\"; fi; \
         umask 000; exec \"$@\""
    } else {
        "umask 000; exec \"$@\""
    }
}

/// The process tag of a member's command (its Space name): a host that
/// restarted finds it by this.
pub fn tag_of(space: &str) -> String {
    format!("cua-job-{space}")
}

/// Output waiting for the log's writer task: bounded, so a disk that cannot
/// keep up slows the command's reader (which then backs the command up)
/// instead of growing memory.
const LOG_QUEUE: usize = 64;

/// The guest scrollback a member's command gets (the Space's default is
/// 8 MiB): a follower that must re-attach after losing its stream resumes
/// from this ring, so it is what a command may write meanwhile without
/// losing output.
const SCROLLBACK_BYTES: u64 = 128 * 1024 * 1024;

/// How many times a lost output stream is re-attached to before the Space
/// is called gone, and the longest wait between tries.
const RECOVER_ATTEMPTS: u32 = 8;
const RECOVER_MAX_WAIT: Duration = Duration::from_secs(15);

/// A member's output log, written by its own task so that a slow disk never
/// stops the async worker that reads the command's output: the worker only
/// hands each chunk to a bounded queue.
struct LogWriter {
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
}

impl LogWriter {
    fn open(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(LOG_QUEUE);
        let task = tokio::task::spawn_blocking(move || {
            use std::io::Write as _;
            let mut file = std::io::BufWriter::with_capacity(256 * 1024, file);
            while let Some(data) = rx.blocking_recv() {
                let _ = file.write_all(&data);
                // Whatever else is already queued goes out in the same
                // write; the log is flushed when the queue runs dry.
                while let Ok(more) = rx.try_recv() {
                    let _ = file.write_all(&more);
                }
                let _ = file.flush();
            }
            let _ = file.flush();
        });
        Ok(Self { tx, task })
    }

    async fn write(&self, data: &[u8]) {
        let _ = self.tx.send(data.to_vec()).await;
    }

    /// Everything handed over is on disk when this returns.
    async fn finish(self) {
        drop(self.tx);
        let _ = self.task.await;
    }
}

/// Runs a member's command to its end, its output going to the attempt's
/// output log as it arrives.
///
/// A lost output stream is not the command's end: the command is found
/// again by its tag and followed from the last byte seen, so a job that
/// finished (or is still running) is never reported as an infrastructure
/// failure because its follower lost a connection. Only a Space that is
/// really gone is [`ExecEnd::Error`].
pub async fn run(cmd: MemberCommand<'_>, space: &str, cancel: &CancellationToken) -> ExecEnd {
    let job = cmd.job;
    let log_path = cmd.dirs.output_log(&job.job_id, job.attempt);
    let log = match LogWriter::open(&log_path) {
        Ok(w) => w,
        Err(e) => return ExecEnd::Error(format!("the member's output log: {e}")),
    };
    let end = run_following(cmd, space, cancel, &log).await;
    log.finish().await;
    end
}

/// Finds the member's command again after its output stream was lost, and
/// follows it from `offset`. `Err` says why the Space cannot be reached
/// (or that the process is gone).
async fn reattach(
    client: &SpacesdClient,
    space: &str,
    offset: u64,
    cancel: &CancellationToken,
    cause: &cua_spacesd_client::Error,
) -> Result<cua_spacesd_client::ProcessHandle, Option<String>> {
    let mut last = cause.to_string();
    for attempt in 0..RECOVER_ATTEMPTS {
        if attempt > 0 {
            let wait = Duration::from_secs(1u64 << attempt.min(4)).min(RECOVER_MAX_WAIT);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = cancel.cancelled() => return Err(None),
            }
        }
        match client
            .attach(ProcessRef::Tag(tag_of(space)), Replay::FromOffset(offset))
            .await
        {
            Ok(h) => return Ok(h),
            // The Space answered and the process is not there: it is gone
            // (restarted, or its record aged out), nothing to follow.
            Err(cua_spacesd_client::Error::ProcessNotFound(d)) => {
                return Err(Some(format!("the process is gone: {}", d.message)));
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(Some(last))
}

async fn run_following(
    cmd: MemberCommand<'_>,
    space: &str,
    cancel: &CancellationToken,
    log: &LogWriter,
) -> ExecEnd {
    let job = cmd.job;
    let timeout = Duration::from_secs(u64::from(job.timeout_secs.max(1)));
    // A shell sets the umask (and nothing else) before the job's command.
    let mut argv: Vec<String> = Vec::new();
    if cmd.open_files || cmd.as_root {
        argv.extend([
            "/bin/sh".into(),
            "-c".into(),
            wrapper(cmd.as_root).into(),
            "sh".into(),
        ]);
    }
    argv.extend(job.argv.iter().cloned());
    let mut c = Command::new(&argv[0])
        .args(argv[1..].iter().cloned())
        .cwd(cmd.cwd.clone())
        .timeout(timeout)
        .scrollback(SCROLLBACK_BYTES)
        .tag(tag_of(space));
    for (k, v) in &cmd.env {
        c = c.env(k, v);
    }
    if let Some(u) = &cmd.user {
        c = c.user(u.clone());
    }
    let mut handle = match cmd.client.spawn(c).await {
        Ok(h) => h,
        Err(e) => return ExecEnd::Error(format!("the command did not start: {e}")),
    };
    let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(30);
    let mut touch_at = tokio::time::Instant::now() + Duration::from_secs(3600);
    let mut stopping: Option<(ExecEnd, tokio::time::Instant)> = None;
    // Re-attaches since the last output (a stream that keeps dying without
    // delivering anything is given up on).
    let mut recoveries = 0u32;
    // Output lost so far that the log already says.
    let mut noted_dropped = 0u64;
    loop {
        let event = tokio::select! {
            e = handle.next_event() => e,
            _ = cancel.cancelled(), if stopping.is_none() => {
                let _ = handle.signal_group(pb::Signal::Term).await;
                stopping = Some((ExecEnd::Canceled, tokio::time::Instant::now() + jobdir::KILL_GRACE));
                continue;
            }
            _ = tokio::time::sleep_until(deadline), if stopping.is_none() => {
                log.write(format!("cua: the job ran out of time ({} s); stopping it\n", job.timeout_secs).as_bytes()).await;
                let _ = handle.signal_group(pb::Signal::Term).await;
                stopping = Some((ExecEnd::TimedOut, tokio::time::Instant::now() + jobdir::KILL_GRACE));
                continue;
            }
            _ = sleep_until_opt(stopping.as_ref().map(|s| s.1)) => {
                let _ = handle.signal_group(pb::Signal::Kill).await;
                // The Space's spacesd says when it is gone; do not wait
                // forever for a machine that stopped answering.
                let _ = tokio::time::timeout(Duration::from_secs(10), handle.next_event()).await;
                return stopping.map(|s| s.0).unwrap_or(ExecEnd::Canceled);
            }
            _ = tokio::time::sleep(Duration::from_secs(600)) => {
                if tokio::time::Instant::now() >= touch_at {
                    // A long run keeps its data from the sweep.
                    cmd.dirs.touch_run(job);
                    touch_at = tokio::time::Instant::now() + Duration::from_secs(3600);
                }
                continue;
            }
        };
        match event {
            Ok(Some(ProcessEvent::Stdout { data, .. }))
            | Ok(Some(ProcessEvent::Stderr { data, .. }))
            | Ok(Some(ProcessEvent::Pty { data, .. })) => {
                recoveries = 0;
                if handle.dropped_bytes() > noted_dropped {
                    // Output the Space no longer held when the stream was
                    // resumed: said in the log, never silent.
                    log.write(
                        format!(
                            "\ncua: {} bytes of output were lost here (the stream was cut while the job wrote faster than it could be re-read)\n",
                            handle.dropped_bytes() - noted_dropped
                        )
                        .as_bytes(),
                    )
                    .await;
                    noted_dropped = handle.dropped_bytes();
                }
                log.write(&data).await;
            }
            Ok(Some(ProcessEvent::Exit(status))) => {
                if let Some((end, _)) = stopping {
                    // Stopped by us: its own exit is not how it ended.
                    let _ = handle.signal_group(pb::Signal::Kill).await;
                    return end;
                }
                // Whatever the command left running in its group goes too.
                let _ = handle.signal_group(pb::Signal::Kill).await;
                if status.timed_out {
                    log.write(
                        format!("cua: the job ran out of time ({} s)\n", job.timeout_secs)
                            .as_bytes(),
                    )
                    .await;
                    return ExecEnd::TimedOut;
                }
                if let Some(code) = status.code {
                    return ExecEnd::Exited(code);
                }
                if let Some(e) = status.error {
                    // The supervisor could not run it ("executable not
                    // found"): the job's own failure, not the host's.
                    log.write(format!("cua: could not start {}: {e}\n", job.argv[0]).as_bytes())
                        .await;
                    return ExecEnd::Exited(127);
                }
                // Killed by a signal.
                return ExecEnd::Exited(128 + status.signal.map_or(9, |s| s as i32));
            }
            Ok(None) => {
                return stopping.map(|s| s.0).unwrap_or_else(|| {
                    ExecEnd::Error("the command's output ended without an exit".into())
                });
            }
            Err(e) => {
                if let Some((end, _)) = stopping {
                    return end;
                }
                // The stream could not be resumed (the client already
                // tried). The command may have finished meanwhile: find it
                // again and read the rest from where it left off.
                recoveries += 1;
                if recoveries > RECOVER_ATTEMPTS {
                    return ExecEnd::Error(format!("lost the command: {e}"));
                }
                match reattach(cmd.client, space, handle.output_offset(), cancel, &e).await {
                    Ok(h) => {
                        // A new handle counts from zero.
                        handle = h;
                        noted_dropped = 0;
                    }
                    Err(None) => return ExecEnd::Canceled,
                    Err(Some(why)) => return ExecEnd::Error(format!("lost the command: {why}")),
                }
            }
        }
    }
}

async fn sleep_until_opt(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Writes a routed member's notes (how it reaches each peer) to its output
/// log, before its own output.
pub fn note_routes(dirs: &JobDirs, job: &JobSpec, routed: &RoutedPeers) {
    let notes: String = routed.notes.iter().map(|n| format!("cua: {n}\n")).collect();
    jobdir::append(&dirs.output_log(&job.job_id, job.attempt), notes.as_bytes());
}

/// The member's result file (its tail, at most `max_output_bytes`), read
/// on the host from its work directory.
pub fn read_result(dirs: &JobDirs, job: &JobSpec) -> Result<Vec<u8>, String> {
    let p = dirs
        .work_dir(&job.job_id, job.attempt)
        .join(&job.result_path);
    match std::fs::read(&p) {
        Ok(bytes) => {
            let keep = job.max_output_bytes as usize;
            Ok(bytes[bytes.len().saturating_sub(keep)..].to_vec())
        }
        // A gang's result is rank 0's: another rank need not write one.
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && job.env.get("CUA_RANK").is_some_and(|r| r.trim() != "0") =>
        {
            Ok(Vec::new())
        }
        Err(e) => Err(format!("result file {}: {e}", job.result_path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_wrapper_sets_the_umask_and_runs_the_command_with_its_arguments() {
        for as_root in [false, true] {
            let out = std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    wrapper(as_root),
                    "sh",
                    "sh",
                    "-c",
                    "umask; echo \"$0|$1\"",
                    "a b",
                    "c",
                ])
                // No sudo on this PATH: the test never becomes root.
                .env("PATH", "/bin")
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success(), "{as_root}: {text}");
            assert!(text.ends_with("a b|c\n"), "{as_root}: {text}");
            assert!(
                text.starts_with("0000\n") || text.starts_with("000\n"),
                "{text}"
            );
        }
    }

    #[test]
    fn only_rank_0_of_a_gang_owes_a_result_file() {
        let home = tempfile::tempdir().unwrap();
        let dirs = JobDirs::new(home.path());
        let mut job = JobSpec {
            job_id: "j".into(),
            attempt: 1,
            argv: vec!["true".into()],
            result_path: "out.bin".into(),
            max_output_bytes: 1024,
            ..Default::default()
        };
        dirs.prepare(&job, false).unwrap();
        assert!(read_result(&dirs, &job).unwrap_err().contains("out.bin"));
        job.env.insert("CUA_RANK".into(), "0".into());
        assert!(read_result(&dirs, &job).is_err());
        job.env.insert("CUA_RANK".into(), "1".into());
        assert_eq!(read_result(&dirs, &job).unwrap(), b"");
        std::fs::write(dirs.work_dir("j", 1).join("out.bin"), b"r").unwrap();
        assert_eq!(read_result(&dirs, &job).unwrap(), b"r");
    }

    /// The stream is cut while the command runs; by the time the member
    /// looks again the command has exited. Its real exit and all of its
    /// output are reported, not an infrastructure error (which would run the
    /// whole job again).
    #[tokio::test]
    async fn a_lost_stream_after_the_process_exited_keeps_its_exit_and_output() {
        use cua_spacesd_client::{
            ConnectOptions, RetryPolicy,
            testing::{CutProxy, MockAuth, MockServer},
        };
        let server = MockServer::start(MockAuth::default()).await;
        let proxy = CutProxy::start(server.addr).await;
        // No resume inside the client: the member's own recovery is tested.
        let client = SpacesdClient::connect(ConnectOptions::parse(&proxy.url()).unwrap().retry(
            RetryPolicy {
                max_attempts: 1,
                initial_backoff: Duration::from_millis(10),
                max_backoff: Duration::from_millis(10),
            },
        ))
        .await
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let dirs = JobDirs::new(home.path());
        let job = JobSpec {
            job_id: "j".into(),
            attempt: 1,
            // 20 ticks, 50 ms apart.
            argv: vec!["ticker".into(), "20".into(), "50".into()],
            timeout_secs: 60,
            result_path: "out.bin".into(),
            max_output_bytes: 1024,
            ..Default::default()
        };
        dirs.prepare(&job, false).unwrap();
        let cancel = CancellationToken::new();
        let cut = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            proxy.cut_now();
        };
        let run = run(
            MemberCommand {
                client: &client,
                job: &job,
                dirs: &dirs,
                env: BTreeMap::new(),
                cwd: "/".into(),
                user: None,
                open_files: false,
                as_root: false,
            },
            "space-1",
            &cancel,
        );
        let (end, ()) = tokio::join!(run, cut);
        assert_eq!(end, ExecEnd::Exited(0));
        let log = std::fs::read_to_string(dirs.output_log("j", 1)).unwrap();
        let want: String = (0..20).map(|i| format!("tick {i}\n")).collect();
        assert_eq!(log, want, "every byte once, none lost");
        assert!(proxy.cuts.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    }

    /// A process that really is gone (the Space restarted) is the only
    /// thing that is an infrastructure error.
    #[tokio::test]
    async fn a_process_that_is_gone_is_an_infrastructure_error() {
        use cua_spacesd_client::{
            ConnectOptions, RetryPolicy,
            testing::{CutProxy, MockAuth, MockServer},
        };
        let server = MockServer::start(MockAuth::default()).await;
        let proxy = CutProxy::start(server.addr).await;
        let client = SpacesdClient::connect(ConnectOptions::parse(&proxy.url()).unwrap().retry(
            RetryPolicy {
                max_attempts: 1,
                initial_backoff: Duration::from_millis(10),
                max_backoff: Duration::from_millis(10),
            },
        ))
        .await
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let dirs = JobDirs::new(home.path());
        let job = JobSpec {
            job_id: "j".into(),
            attempt: 1,
            argv: vec!["ticker".into(), "100".into(), "50".into()],
            timeout_secs: 60,
            result_path: "out.bin".into(),
            max_output_bytes: 1024,
            ..Default::default()
        };
        dirs.prepare(&job, false).unwrap();
        let cancel = CancellationToken::new();
        let wipe = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            proxy.cut_now();
            // The Space's spacesd lost its process table.
            server.state.forget_processes();
        };
        let run = run(
            MemberCommand {
                client: &client,
                job: &job,
                dirs: &dirs,
                env: BTreeMap::new(),
                cwd: "/".into(),
                user: None,
                open_files: false,
                as_root: false,
            },
            "space-2",
            &cancel,
        );
        let (end, ()) = tokio::join!(run, wipe);
        match end {
            ExecEnd::Error(e) => assert!(e.contains("gone"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_members_environment_says_where_everything_is_as_it_sees_it() {
        let mut job = JobSpec {
            job_id: "acct/j-1".into(),
            attempt: 2,
            run_id: "acct/run-1".into(),
            argv: vec!["python".into()],
            result_path: "out/result.bin".into(),
            ..Default::default()
        };
        job.env.insert("CUA_BARRIER_TIMEOUT".into(), "900".into());
        job.env.insert("CUA_RANK".into(), "1".into());
        job.env
            .insert("CUA_PEERS".into(), "10.0.0.1:47010,10.0.0.2:47010".into());
        // A job cannot move the host's own paths.
        job.env.insert("CUA_JOB_DIR".into(), "/elsewhere".into());
        let guest = GuestPaths::under("/cua", LINUX_PATH);
        let routed = RoutedPeers::new(vec!["172.17.0.1:50001".into(), "172.17.0.3:47010".into()]);
        let env = member_env(&job, &guest, "container", Some(&routed));
        let get = |k: &str| env.get(k).map(String::as_str).unwrap_or("");
        assert_eq!(get("CUA_JOB_ID"), "j-1");
        assert_eq!(get("CUA_RUN_ID"), "run-1");
        assert_eq!(get("CUA_ATTEMPT"), "2");
        assert_eq!(get("CUA_RANK"), "1");
        assert_eq!(get("CUA_WORLD_SIZE"), "1");
        assert_eq!(get("CUA_JOB_DIR"), "/cua/job");
        assert_eq!(get("CUA_JOB_DATA"), "/cua/data");
        assert_eq!(get("CUA_HEARTBEAT"), "/cua/job/.cua/heartbeat");
        assert_eq!(get("CUA_BARRIER"), "/cua/job/.cua/barrier");
        assert_eq!(get("CUA_RESULT_PATH"), "/cua/job/out/result.bin");
        assert_eq!(get("CUA_SPACE_KIND"), "container");
        assert_eq!(get("UV_CACHE_DIR"), "/cua/cache/uv");
        assert_eq!(get("UV_PYTHON_INSTALL_DIR"), "/cua/cache/python");
        assert_eq!(get("HF_HOME"), "/cua/cache/hf");
        assert!(get("PATH").starts_with("/cua/tools:/usr/local/sbin"));
        assert_eq!(get("CUA_PEERS"), "172.17.0.1:50001,172.17.0.3:47010");
        assert_eq!(get("CUA_PEERS_PLACED"), "10.0.0.1:47010,10.0.0.2:47010");
        // No barrier asked for: no barrier directory named.
        job.env.remove("CUA_BARRIER_TIMEOUT");
        let env = member_env(&job, &guest, "machine", None);
        assert!(!env.contains_key("CUA_BARRIER"));
        assert!(!env.contains_key("CUA_PEERS_PLACED"));
        assert!(!env.contains_key("UV_PYTHON_CACHE_DIR"));
        // A macOS guest installs Python on its own disk and caches the
        // downloads in the shared cache.
        let mac = GuestPaths {
            python_installs: Some("/private/tmp/cua-python".into()),
            ..GuestPaths::under("/private/tmp/cua", MACOS_PATH)
        };
        let env = member_env(&job, &mac, "vm", None);
        assert_eq!(env["UV_PYTHON_INSTALL_DIR"], "/private/tmp/cua-python");
        assert_eq!(
            env["UV_PYTHON_CACHE_DIR"],
            "/private/tmp/cua/cache/python-downloads"
        );
        assert_eq!(env["CUA_PEERS"], "10.0.0.1:47010,10.0.0.2:47010");
    }
}
