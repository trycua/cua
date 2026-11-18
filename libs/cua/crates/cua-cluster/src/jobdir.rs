//! A job member's directories on its host, which the member's Space sees
//! through mounts (a container's binds, a macOS VM's shared directories) or,
//! on a dedicated machine's own Space, directly:
//!
//! - **Attempt directory** `<cua home>/jobs/<space>/` ([`JobDirs::attempt_dir`]),
//!   host only: `output.log` (the member's stdout and stderr, merged in
//!   order, read live from any offset: [`JobDirs::output`],
//!   `GetHostJobRequest.output_from`).
//! - **Work directory** `<attempt>/job/` ([`JobDirs::work_dir`]): the job's
//!   bundle (`HostJobSpec.bundle`, a gzip tar) unpacked, the command's
//!   working directory, and its control files in `.cua/`: `heartbeat` (a file
//!   the job rewrites as it makes progress, `{"step": n}`) and, for a gang
//!   member with a start barrier, `barrier/` (it writes `ready`, the host
//!   writes `go`: [`JobDirs::release`]). Removed when the attempt ends
//!   ([`JobDirs::finish`]).
//! - **Run data** `<cua home>/job-data/<run>/` ([`JobDirs::run_data_dir`],
//!   `CUA_JOB_DATA`): shared by every attempt of the run on this host
//!   (checkpoints), removed 7 days after the run's last attempt here ended.
//! - **Caches** `<cua home>/job-cache/` ([`JobDirs::cache_dir`]): uv's cache
//!   and Python installs, and the Hugging Face cache, shared by the jobs of
//!   this host's account on purpose (a second run starts fast).
//! - **Tools** `<cua home>/job-tools/<target>/` ([`JobDirs::tools_dir`]):
//!   `uv` for the guest's OS and CPU, mounted read-only.
//!
//! Liveness ([`JobDirs::progress`]): a heartbeat that names a step, else,
//! for a job that never writes one, the last time it wrote output (once it
//! has written any): a member that goes silent for the gang's stall timeout
//! is stalled either way.

use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::model::{JobSpec, MAX_BUNDLE, space_name};

/// How long a member gets between SIGTERM and SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(15);
/// How long a run's data and an attempt's output stay after they were
/// last used.
pub const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
/// Most bytes of live output one read returns.
pub const MAX_OUTPUT_CHUNK: usize = 256 * 1024;
/// Most bytes a bundle unpacks to.
const MAX_UNPACKED: u64 = 256 * 1024 * 1024;
/// Most files in a bundle.
const MAX_ENTRIES: usize = 20_000;
/// The file in a run's data directory whose time says when it was last used.
const LAST_USED: &str = ".cua-last-used";
/// The merged output of a member.
pub const OUTPUT_LOG: &str = "output.log";
/// The work directory's name in an attempt directory.
pub const WORK: &str = "job";
/// The work directory's control files (heartbeat, start barrier).
pub const CONTROL: &str = ".cua";
/// The heartbeat file in [`CONTROL`].
pub const HEARTBEAT: &str = "heartbeat";
/// The start barrier directory in [`CONTROL`].
pub const BARRIER: &str = "barrier";

/// Whether a job waits at a start barrier.
pub fn has_barrier(job: &JobSpec) -> bool {
    job.env
        .get("CUA_BARRIER_TIMEOUT")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|t| t > 0)
}

/// A run's data directory name: readable, and unique per run (the job id's
/// owner included).
pub fn run_dir_name(run: &str) -> String {
    use sha2::Digest as _;
    let digest = hex::encode(sha2::Sha256::digest(run.as_bytes()));
    let readable: String = run
        .rsplit('/')
        .next()
        .unwrap_or(run)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .take(60)
        .collect();
    let readable = readable.trim_matches(|c| c == '-' || c == '.');
    if readable.is_empty() {
        digest[..16].to_string()
    } else {
        format!("{readable}-{}", &digest[..12])
    }
}

/// The run a job belongs to (its own id for a plain job).
pub fn run_of(job: &JobSpec) -> &str {
    if job.run_id.is_empty() {
        &job.job_id
    } else {
        &job.run_id
    }
}

/// The directories of the jobs a host runs, under its cua home.
#[derive(Clone, Debug)]
pub struct JobDirs {
    home: PathBuf,
    /// Mode of the parents (`jobs/`, `job-data/`, each attempt's): 0700,
    /// or 0711 when members run as other users of this machine and must
    /// pass through them (they still cannot list them).
    parents: u32,
}

impl JobDirs {
    /// Under the cua home `home`.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            parents: 0o700,
        }
    }

    /// Under `base`, for members that run as other users of this machine
    /// (a dedicated machine's per-run users): the parents let them pass
    /// through, not list.
    pub fn for_other_users(base: impl Into<PathBuf>) -> Self {
        Self {
            home: base.into(),
            parents: 0o711,
        }
    }

    /// The cua home.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Where attempt directories live.
    pub fn jobs_dir(&self) -> PathBuf {
        self.home.join("jobs")
    }

    /// Where runs' data lives.
    pub fn data_root(&self) -> PathBuf {
        self.home.join("job-data")
    }

    /// The directory of a job attempt (host only).
    pub fn attempt_dir(&self, job_id: &str, attempt: u32) -> PathBuf {
        self.jobs_dir().join(space_name(job_id, attempt))
    }

    /// The attempt's work directory (the member's).
    pub fn work_dir(&self, job_id: &str, attempt: u32) -> PathBuf {
        self.attempt_dir(job_id, attempt).join(WORK)
    }

    /// The attempt's output log.
    pub fn output_log(&self, job_id: &str, attempt: u32) -> PathBuf {
        self.attempt_dir(job_id, attempt).join(OUTPUT_LOG)
    }

    /// A run's data directory (`CUA_JOB_DATA`).
    pub fn run_data_dir(&self, run: &str) -> PathBuf {
        self.data_root().join(run_dir_name(run))
    }

    /// The caches jobs share (uv, Python, Hugging Face).
    pub fn cache_dir(&self) -> PathBuf {
        self.home.join("job-cache")
    }

    /// Tools for guests of `target` (a uv release target such as
    /// `aarch64-apple-darwin`).
    pub fn tools_dir(&self, target: &str) -> PathBuf {
        self.home.join("job-tools").join(target)
    }

    /// Makes a fresh attempt directory for `job` (its bundle unpacked, its
    /// control files, its run's data directory and the caches). `shared`:
    /// the member runs as another user than this host's (a container's
    /// user, a per-run user), so what it writes to is open to every user;
    /// the parents stay this user's own.
    pub fn prepare(&self, job: &JobSpec, shared: bool) -> Result<(), String> {
        let at = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
        if self.parents != 0o700 {
            create_with(&self.home, self.parents).map_err(|e| at(&self.home, e))?;
        }
        for d in [
            self.jobs_dir(),
            self.data_root(),
            self.home.join("job-tools"),
        ] {
            create_with(&d, self.parents).map_err(|e| at(&d, e))?;
        }
        let attempt = self.attempt_dir(&job.job_id, job.attempt);
        if attempt.exists() {
            let _ = open_tree(&attempt);
            std::fs::remove_dir_all(&attempt).map_err(|e| at(&attempt, e))?;
        }
        create_with(&attempt, self.parents).map_err(|e| at(&attempt, e))?;
        let work = attempt.join(WORK);
        std::fs::create_dir_all(&work).map_err(|e| at(&work, e))?;
        let control = work.join(CONTROL);
        let barrier = control.join(BARRIER);
        std::fs::create_dir_all(if has_barrier(job) { &barrier } else { &control })
            .map_err(|e| at(&control, e))?;
        if !job.bundle.is_empty() {
            unpack(&job.bundle, &work)?;
        }
        let data = self.run_data_dir(run_of(job));
        std::fs::create_dir_all(&data).map_err(|e| at(&data, e))?;
        touch(&data.join(LAST_USED));
        let cache = self.cache_dir();
        for sub in ["uv", "python", "hf"] {
            let d = cache.join(sub);
            std::fs::create_dir_all(&d).map_err(|e| at(&d, e))?;
        }
        if shared {
            // The bundle's files, and the top of the run's data and the
            // caches (what the member makes there it makes open: it runs
            // with umask 000).
            open_tree(&work).map_err(|e| at(&work, e))?;
            for d in [data.clone(), cache.clone()]
                .into_iter()
                .chain(["uv", "python", "hf"].map(|s| cache.join(s)))
            {
                open_dir(&d).map_err(|e| at(&d, e))?;
            }
        }
        Ok(())
    }

    /// The attempt ended: its work directory goes (what the job left there
    /// is not the run's data), its output log stays (read after the end),
    /// and the run's data is marked used.
    pub fn finish(&self, job: &JobSpec) {
        let work = self.work_dir(&job.job_id, job.attempt);
        if std::fs::remove_dir_all(&work).is_err() {
            // Files a member made read-only: open them, then try again.
            let _ = open_tree(&work);
            let _ = std::fs::remove_dir_all(&work);
        }
        touch(&self.run_data_dir(run_of(job)).join(LAST_USED));
    }

    /// Keeps a long run's data from the sweep.
    pub fn touch_run(&self, job: &JobSpec) {
        touch(&self.run_data_dir(run_of(job)).join(LAST_USED));
    }

    /// Removes run data and attempt directories last used more than `keep`
    /// ago, except `running` attempts. Returns how many went.
    pub fn sweep(&self, keep: Duration, running: &dyn Fn(&str) -> bool) -> usize {
        let now = SystemTime::now();
        let old = |p: &Path, marker: &str| {
            let t = std::fs::metadata(p.join(marker))
                .or_else(|_| std::fs::metadata(p))
                .ok()
                .and_then(|m| m.modified().ok());
            t.is_some_and(|t| now.duration_since(t).unwrap_or_default() > keep)
        };
        let mut removed = 0;
        for (dir, marker) in [(self.data_root(), LAST_USED), (self.jobs_dir(), OUTPUT_LOG)] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() && !running(&name) && old(&p, marker) {
                    let _ = open_tree(&p);
                    if std::fs::remove_dir_all(&p).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
        removed
    }

    /// A member's live output from byte `from` (at most
    /// [`MAX_OUTPUT_CHUNK`]) and its size so far.
    pub fn output(&self, job_id: &str, attempt: u32, from: u64) -> Option<(Vec<u8>, u64)> {
        read_from(&self.output_log(job_id, attempt), from, MAX_OUTPUT_CHUNK)
    }

    /// The tail of a member's output (at most `max` bytes).
    pub fn output_tail(&self, job_id: &str, attempt: u32, max: usize) -> Vec<u8> {
        tail_of(&self.output_log(job_id, attempt), max)
    }

    /// A running member's start barrier and progress.
    pub fn progress(
        &self,
        job_id: &str,
        attempt: u32,
    ) -> Option<cua_proto::env::v1::HostJobProgress> {
        let work = self.work_dir(job_id, attempt);
        control_progress(&work.join(CONTROL), &self.output_log(job_id, attempt))
    }

    /// Releases a member's start barrier (idempotent).
    pub fn release(&self, job_id: &str, attempt: u32) {
        let barrier = self.work_dir(job_id, attempt).join(CONTROL).join(BARRIER);
        if barrier.is_dir() && !barrier.join("go").exists() {
            let tmp = barrier.join(".go.tmp");
            if std::fs::write(&tmp, b"go\n").is_ok() {
                let _ = std::fs::rename(&tmp, barrier.join("go"));
            }
        }
    }
}

fn create_with(dir: &Path, mode: u32) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(())
}

fn open_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o777))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Opens `dir` and what is in it to every user (directories 0777, files
/// 0666 plus their execute bits), without following links. Only for
/// directories under this user's own private parents.
pub fn open_tree(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let meta = std::fs::symlink_metadata(dir)?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }
        let mode = meta.permissions().mode();
        if meta.is_dir() {
            if mode & 0o777 != 0o777 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o777))?;
            }
            for e in std::fs::read_dir(dir)?.flatten() {
                let _ = open_tree(&e.path());
            }
        } else if meta.is_file() {
            let want = (mode & 0o777) | 0o666;
            if mode & 0o777 != want {
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(want));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

pub(crate) fn touch(p: &Path) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(p)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// Appends `data` to the file `p`.
pub fn append(p: &Path, data: &[u8]) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = f.write_all(data);
    }
}

/// Unpacks a gzip tar into `dir`: regular files and directories only, no
/// path out of `dir`, at most 256 MiB and 20,000 entries.
pub fn unpack(bundle: &[u8], dir: &Path) -> Result<(), String> {
    if bundle.len() > MAX_BUNDLE {
        return Err(format!("the bundle is over {MAX_BUNDLE} bytes"));
    }
    let gz = flate2::read::GzDecoder::new(bundle);
    let mut ar = tar::Archive::new(gz.take(MAX_UNPACKED + 1));
    let mut total = 0u64;
    let entries = ar.entries().map_err(|e| format!("the bundle: {e}"))?;
    for (n, entry) in entries.enumerate() {
        if n >= MAX_ENTRIES {
            return Err(format!("the bundle has over {MAX_ENTRIES} files"));
        }
        let mut entry = entry.map_err(|e| format!("the bundle: {e}"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        total += entry.header().size().unwrap_or(0);
        if total > MAX_UNPACKED {
            return Err(format!("the bundle unpacks to over {MAX_UNPACKED} bytes"));
        }
        // `unpack_in` refuses paths out of `dir` (`..`, absolute).
        let ok = entry
            .unpack_in(dir)
            .map_err(|e| format!("the bundle: {e}"))?;
        if !ok {
            return Err("the bundle has a path outside its directory".into());
        }
    }
    Ok(())
}

/// The bytes of `path` from `from`, at most `max`, and its size.
pub fn read_from(path: &Path, from: u64, max: usize) -> Option<(Vec<u8>, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    if from >= size {
        return Some((vec![], size));
    }
    f.seek(std::io::SeekFrom::Start(from)).ok()?;
    let n = ((size - from) as usize).min(max);
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf).ok()?;
    Some((buf, size))
}

/// The last `max` bytes of `path`.
pub fn tail_of(path: &Path, max: usize) -> Vec<u8> {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    read_from(path, size.saturating_sub(max as u64), max)
        .map(|(b, _)| b)
        .unwrap_or_default()
}

fn mtime_ms(p: &Path) -> Option<i64> {
    let at = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(at.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// A member's start barrier and progress from its control directory and
/// output log: `None` when it reports neither (no barrier, no heartbeat
/// with a step, no output yet). A member that never heartbeats is as live
/// as its output: the last time it wrote any (step -1).
pub fn control_progress(
    control: &Path,
    output: &Path,
) -> Option<cua_proto::env::v1::HostJobProgress> {
    let barrier = control.join(BARRIER);
    let has_barrier = barrier.is_dir();
    let released = barrier.join("go").exists();
    let beat = control.join(HEARTBEAT);
    // Progress is a heartbeat that names a step: setup (imports, a model
    // download) before the first step never counts as stalled.
    let from_beat = std::fs::read(&beat)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("step").and_then(serde_json::Value::as_i64))
        .and_then(|step| Some((mtime_ms(&beat)?, step)));
    // Without one, its output is its progress, once it is past its
    // barrier (a member waiting there is not stalled) and has written some.
    let (last_progress_ms, step) = from_beat
        .or_else(|| {
            let size = std::fs::metadata(output).ok()?.len();
            (size > 0 && (!has_barrier || released)).then(|| Some((mtime_ms(output)?, -1)))?
        })
        .unwrap_or((0, -1));
    if !has_barrier && last_progress_ms == 0 {
        return None;
    }
    Some(cua_proto::env::v1::HostJobProgress {
        job_id: String::new(),
        attempt: 0,
        ready: barrier.join("ready").exists(),
        released,
        last_progress_ms,
        step,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, attempt: u32) -> JobSpec {
        JobSpec {
            job_id: id.into(),
            attempt,
            run_id: "acct/run-1".into(),
            argv: vec!["true".into()],
            ..Default::default()
        }
    }

    fn bundle(files: &[(&str, &str)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, body.as_bytes()).unwrap();
        }
        let raw = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn an_attempt_gets_its_bundle_control_files_run_data_and_caches() {
        let home = tempfile::tempdir().unwrap();
        let dirs = JobDirs::new(home.path());
        let mut j = job("acct/j-1", 2);
        j.bundle = bundle(&[("train.py", "print(1)\n"), ("data/a.jsonl", "{}\n")]);
        j.env.insert("CUA_BARRIER_TIMEOUT".into(), "60".into());
        dirs.prepare(&j, true).unwrap();
        let work = dirs.work_dir(&j.job_id, 2);
        assert_eq!(
            std::fs::read_to_string(work.join("train.py")).unwrap(),
            "print(1)\n"
        );
        assert!(work.join("data/a.jsonl").is_file());
        assert!(work.join(".cua/barrier").is_dir());
        let data = dirs.run_data_dir("acct/run-1");
        assert!(data.is_dir() && data.ends_with(run_dir_name("acct/run-1")));
        for sub in ["uv", "python", "hf"] {
            assert!(dirs.cache_dir().join(sub).is_dir());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            // What the member writes to is open to its user; the parents
            // stay this user's own.
            assert_eq!(mode(&work), 0o777);
            assert_eq!(mode(&data), 0o777);
            assert_eq!(mode(&dirs.jobs_dir()), 0o700);
            assert_eq!(mode(&dirs.attempt_dir(&j.job_id, 2)), 0o700);
            assert_eq!(mode(&dirs.data_root()), 0o700);
        }
        // A new attempt of the same job starts from a fresh directory.
        std::fs::write(work.join("left-over"), b"x").unwrap();
        dirs.prepare(&j, false).unwrap();
        assert!(!work.join("left-over").exists());
        // The end removes the work directory, keeps the output and data.
        append(&dirs.output_log(&j.job_id, 2), b"hello\n");
        dirs.finish(&j);
        assert!(!work.exists());
        assert_eq!(dirs.output(&j.job_id, 2, 0).unwrap().0, b"hello\n");
        assert!(data.is_dir());
    }

    #[test]
    fn a_bundle_cannot_write_outside_its_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut tar = tar::Builder::new(Vec::new());
        let body = b"x";
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        // `append_data` refuses `..`; write the raw name.
        h.as_gnu_mut().unwrap().name[..9].copy_from_slice(b"../escape");
        h.set_cksum();
        tar.append(&h, &body[..]).unwrap();
        let raw = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &raw).unwrap();
        let bundle = gz.finish().unwrap();
        let inner = dir.path().join("in");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(unpack(&bundle, &inner).is_err());
        assert!(!dir.path().join("escape").exists());
    }

    #[test]
    fn old_run_data_and_attempts_are_swept_but_not_running_ones() {
        let home = tempfile::tempdir().unwrap();
        let dirs = JobDirs::new(home.path());
        let a = job("j-a", 1);
        let b = job("j-b", 1);
        dirs.prepare(&a, false).unwrap();
        dirs.prepare(&b, false).unwrap();
        append(&dirs.output_log("j-a", 1), b"x");
        append(&dirs.output_log("j-b", 1), b"x");
        let running_b = space_name("j-b", 1);
        // Nothing is old yet.
        assert_eq!(dirs.sweep(KEEP, &|_| false), 0);
        // Everything counts as old: all but the running attempt goes.
        let n = dirs.sweep(Duration::ZERO, &|name| name == running_b);
        assert_eq!(n, 2, "a's attempt and the run's data");
        assert!(!dirs.attempt_dir("j-a", 1).exists());
        assert!(dirs.attempt_dir("j-b", 1).exists());
    }

    #[test]
    fn progress_is_the_heartbeat_else_the_output_once_past_the_barrier() {
        let home = tempfile::tempdir().unwrap();
        let dirs = JobDirs::new(home.path());
        let mut j = job("j-p", 1);
        // No barrier, no heartbeat, no output: nothing to report.
        dirs.prepare(&j, false).unwrap();
        assert_eq!(dirs.progress("j-p", 1), None);
        // Output only: live as of its last write, no step.
        append(&dirs.output_log("j-p", 1), b"loading\n");
        let p = dirs.progress("j-p", 1).unwrap();
        assert!(p.last_progress_ms > 0);
        assert_eq!(p.step, -1);
        // A heartbeat with a step wins.
        let beat = dirs.work_dir("j-p", 1).join(".cua/heartbeat");
        std::fs::write(&beat, br#"{"step": 7}"#).unwrap();
        assert_eq!(dirs.progress("j-p", 1).unwrap().step, 7);
        // A member waiting at its barrier is not making progress by its
        // output; once released it is.
        j.env.insert("CUA_BARRIER_TIMEOUT".into(), "60".into());
        j.attempt = 2;
        dirs.prepare(&j, false).unwrap();
        append(&dirs.output_log("j-p", 2), b"installing\n");
        let p = dirs.progress("j-p", 2).unwrap();
        assert!(!p.ready && !p.released);
        assert_eq!(p.last_progress_ms, 0);
        std::fs::write(dirs.work_dir("j-p", 2).join(".cua/barrier/ready"), b"").unwrap();
        dirs.release("j-p", 2);
        dirs.release("j-p", 2);
        let p = dirs.progress("j-p", 2).unwrap();
        assert!(p.ready && p.released);
        assert!(p.last_progress_ms > 0);
    }
}
