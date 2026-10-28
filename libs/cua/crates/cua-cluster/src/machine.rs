//! `kind = "machine"`: a dedicated host's own Space. A machine set up as
//! dedicated (`cua host setup --dedicated`, which `cua cloud add aws` passes; a
//! cloud instance that exists only for this work) offers itself: its job
//! members run through its own cua-spacesd, like a command in any Space,
//! with the host's GPU and drivers as they are. The instance is the
//! isolation boundary; a user's own computer is never dedicated (its
//! members run in macOS VMs or containers).
//!
//! On a long-lived dedicated host, runs stay apart from each other: when
//! the host runs as root on Linux, each run gets its own user
//! (`cuaj-<hash of the run>`), which owns the run's work and data
//! directories (under `/var/lib/cua-jobs`, which other users can pass
//! through but not list); when an attempt ends, its user's processes are
//! killed, its files in `/tmp`, `/var/tmp` and `/dev/shm` removed and the
//! user deleted (the run's data becomes root's again, so whoever gets that
//! user id next cannot read it), and the work directory goes. What stays on
//! purpose: the run's data directory (checkpoints, for its next attempt,
//! whose new user gets it back), and the uv and Hugging Face caches every
//! run on the host shares (so a second run does not download the same
//! Python packages and models again).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use cua_spacesd_client::SpacesdClient;
use tokio_util::sync::CancellationToken;

use crate::jobdir::{self, JobDirs};
use crate::member::{self, GuestPaths, MemberCommand};
use crate::model::{JobSpec, KIND_MACHINE};
use crate::node::{ExecEnd, ExecOutcome, JobExecutor, Offers};
use crate::peers::{MemberNet, PeerRouter, route_member};

/// How to reach this host's own cua-spacesd.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostSpacesd {
    /// Its URL (`http://127.0.0.1:3211`).
    pub url: String,
    /// Its token.
    pub token: String,
}

type SpacesdFn = Arc<dyn Fn() -> Result<HostSpacesd, String> + Send + Sync>;

/// Where a dedicated host keeps its members' directories when each run
/// gets its own user.
pub const ISOLATED_ROOT: &str = "/var/lib/cua-jobs";

/// What a member's host services (`JobSpec::services`) give it when it
/// runs in this host's own Space: the host's runner started them (the
/// member reaches their loopback ports directly, it runs on this host).
#[derive(Clone, Debug, Default)]
pub struct MemberExtras {
    /// Environment for its command (`{work}`: its work directory).
    pub env: std::collections::BTreeMap<String, String>,
    /// Files written into its work directory (relative paths) before it
    /// starts.
    pub files: Vec<(String, Vec<u8>)>,
}

/// Runs job members in this host's own Space.
#[derive(Clone)]
pub struct MachineExecutor {
    dirs: JobDirs,
    spacesd: SpacesdFn,
    router: Option<Arc<dyn PeerRouter>>,
    /// A user per run (Linux, as root).
    per_run_users: bool,
    /// Members running per run user: a user goes when its run's last
    /// member here ends.
    users: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

impl std::fmt::Debug for MachineExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachineExecutor")
            .field("dirs", &self.dirs)
            .field("per_run_users", &self.per_run_users)
            .finish()
    }
}

/// Whether this process can give each run its own user (Linux, as root).
pub fn can_isolate_runs() -> bool {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

impl MachineExecutor {
    /// Members in this host's own Space under the cua home `home`, through
    /// the cua-spacesd `spacesd` says (asked at each start), with the pinned
    /// `uv` first on their `PATH` (installed into the members' tools
    /// directory on first use). As root on Linux each run gets its own
    /// user, and the members' directories move to [`ISOLATED_ROOT`] (the
    /// cua home is root's own).
    pub fn new(
        home: impl Into<PathBuf>,
        spacesd: impl Fn() -> Result<HostSpacesd, String> + Send + Sync + 'static,
    ) -> Self {
        let per_run_users = can_isolate_runs();
        let dirs = if per_run_users {
            JobDirs::for_other_users(ISOLATED_ROOT)
        } else {
            JobDirs::new(home)
        };
        Self {
            dirs,
            spacesd: Arc::new(spacesd),
            router: None,
            per_run_users,
            users: Arc::default(),
        }
    }

    /// Routes gang members' peers through `router` before they start.
    pub fn with_peer_router(mut self, router: Arc<dyn PeerRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// The members' directories.
    pub fn dirs(&self) -> &JobDirs {
        &self.dirs
    }

    /// Whether each run gets its own user here.
    pub fn per_run_users(&self) -> bool {
        self.per_run_users
    }

    async fn client(&self) -> Result<SpacesdClient, String> {
        let at = (self.spacesd)()?;
        SpacesdClient::connect_url(&at.url, Some(at.token))
            .await
            .map_err(|e| format!("this computer's cua-spacesd at {}: {e}", at.url))
    }

    async fn run_inner(
        &self,
        job: &JobSpec,
        space: &str,
        cancel: &CancellationToken,
        out: &mut ExecOutcome,
        extras: Option<MemberExtras>,
    ) -> Result<ExecEnd, String> {
        // A member that asks for host services runs only with them: never
        // without (its credentials and endpoints would be missing).
        if !job.services.is_empty() && extras.is_none() {
            return Err(format!(
                "this computer started none of the job services the member needs ({})",
                job.services.join(", ")
            ));
        }
        let extras = extras.unwrap_or_default();
        let files = extras.files;
        if let Some((bad, _)) = files.iter().find(|(rel, _)| {
            rel.starts_with('/') || rel.split('/').any(|c| c == ".." || c.is_empty())
        }) {
            return Err(format!("a job service wrote a bad path: {bad}"));
        }
        let t0 = Instant::now();
        let dirs = self.dirs.clone();
        let isolate = self.per_run_users;
        let j = job.clone();
        let space_owned = space.to_string();
        if isolate {
            *self
                .users
                .lock()
                .expect("users")
                .entry(run_user::name(jobdir::run_of(job)))
                .or_default() += 1;
        }
        let user = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
            dirs.prepare(&j, isolate)?;
            let work = dirs.work_dir(&j.job_id, j.attempt);
            for (rel, data) in &files {
                let p = work.join(rel);
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            if !isolate {
                return Ok(None);
            }
            let user = run_user::name(jobdir::run_of(&j));
            let data = dirs.run_data_dir(jobdir::run_of(&j));
            run_user::ensure(&user, &work)?;
            run_user::own(&user, &[&work, &data])?;
            // A host that restarts mid-attempt finds whose it was.
            let _ = std::fs::write(
                dirs.attempt_dir(&j.job_id, j.attempt)
                    .join(run_user::RECORD),
                format!("{user}\n"),
            );
            let _ = space_owned;
            Ok(Some(user))
        })
        .await
        .map_err(|e| e.to_string())??;
        let client = self.client().await?;
        let target = cua_host::uv::uv_target()
            .ok_or_else(|| format!("no uv is pinned for {}", std::env::consts::OS))?;
        let tools = self.dirs.tools_dir(target);
        cua_host::uv::ensure_uv_in(&tools, target)
            .await
            .map_err(|e| format!("uv for this computer's members: {e}"))?;
        let routed = route_member(self.router.as_deref(), job, MemberNet::machine()).await?;
        if let Some(r) = &routed {
            member::note_routes(&self.dirs, job, r);
        }
        let work = self.dirs.work_dir(&job.job_id, job.attempt);
        let guest = GuestPaths {
            work: work.display().to_string(),
            data: self
                .dirs
                .run_data_dir(jobdir::run_of(job))
                .display()
                .to_string(),
            cache: self.dirs.cache_dir().display().to_string(),
            tools: tools.display().to_string(),
            path: std::env::var("PATH").unwrap_or_else(|_| member::LINUX_PATH.into()),
            python_installs: None,
        };
        let mut env = member::member_env(job, &guest, KIND_MACHINE, routed.as_ref());
        // The cache and its environments are on the same file system here.
        env.remove("UV_LINK_MODE");
        for (k, v) in &extras.env {
            env.insert(k.clone(), v.replace("{work}", &guest.work));
        }
        if let Some(u) = &user {
            env.insert("HOME".into(), guest.work.clone());
            env.insert("USER".into(), u.clone());
            env.insert("LOGNAME".into(), u.clone());
        }
        out.create_ms = t0.elapsed().as_millis() as u32;
        let t1 = Instant::now();
        let end = member::run(
            MemberCommand {
                client: &client,
                job,
                dirs: &self.dirs,
                env,
                cwd: guest.work.clone(),
                user: user.clone(),
                open_files: user.is_some(),
                as_root: false,
            },
            space,
            cancel,
        )
        .await;
        drop(routed);
        out.run_ms = t1.elapsed().as_millis() as u32;
        if !job.result_path.is_empty() && matches!(end, ExecEnd::Exited(_)) {
            match member::read_result(&self.dirs, job) {
                Ok(r) => out.result = r,
                Err(e) => out.result_error = Some(e),
            }
        }
        Ok(end)
    }

    fn end_attempt(&self, job: &JobSpec) {
        if self.per_run_users {
            let user = run_user::name(jobdir::run_of(job));
            let last = {
                let mut g = self.users.lock().expect("users");
                let n = g.entry(user.clone()).or_default();
                *n = n.saturating_sub(1);
                let last = *n == 0;
                if last {
                    g.remove(&user);
                }
                last
            };
            // Another member of the same run still runs here as this user.
            if last {
                run_user::scrub(&user);
                // The user's id goes back to the pool of free ids: what it
                // owned (the run's data) becomes root's again, so the next
                // user that gets that id cannot read it.
                run_user::disown(&self.dirs.run_data_dir(jobdir::run_of(job)));
            }
        }
        self.dirs.finish(job);
    }
}

impl MachineExecutor {
    /// [`JobExecutor::run`] with what the member's host services give it
    /// (`None`: it asked for none; one that did is refused).
    pub async fn run_with(
        &self,
        job: &JobSpec,
        space: &str,
        cancel: CancellationToken,
        extras: Option<MemberExtras>,
    ) -> ExecOutcome {
        let mut out = ExecOutcome::ended(ExecEnd::Exited(0));
        let t0 = Instant::now();
        out.end = match self.run_inner(job, space, &cancel, &mut out, extras).await {
            Ok(end) => end,
            Err(e) => {
                jobdir::append(
                    &self.dirs.output_log(&job.job_id, job.attempt),
                    format!("cua: {e}\n").as_bytes(),
                );
                if out.create_ms == 0 {
                    out.create_ms = t0.elapsed().as_millis() as u32;
                }
                ExecEnd::Error(format!("the member did not start: {e}"))
            }
        };
        // The runner keeps the tail: read a little more than it keeps so
        // it can say whether it cut.
        let max = (job.max_output_bytes as usize).saturating_mul(2).max(1);
        out.stdout = self.dirs.output_tail(&job.job_id, job.attempt, max);
        let t2 = Instant::now();
        let me = self.clone();
        let j = job.clone();
        let _ = tokio::task::spawn_blocking(move || me.end_attempt(&j)).await;
        out.delete_ms = t2.elapsed().as_millis() as u32;
        out
    }
}

#[async_trait]
impl JobExecutor for MachineExecutor {
    async fn run(&self, job: &JobSpec, space: &str, cancel: CancellationToken) -> ExecOutcome {
        self.run_with(job, space, cancel, None).await
    }

    async fn remove_space(&self, space: &str) {
        // A member a crash left: stop what still runs of it.
        let attempt = self.dirs.jobs_dir().join(space);
        if !attempt.is_dir() {
            return;
        }
        if let Ok(client) = self.client().await {
            let _ = client
                .signal_group(
                    cua_spacesd_client::ProcessRef::Tag(member::tag_of(space)),
                    cua_spacesd_client::pb::Signal::Kill,
                )
                .await;
        }
        if let Ok(user) = std::fs::read_to_string(attempt.join(run_user::RECORD)) {
            let user = user.trim().to_string();
            if user.starts_with(run_user::PREFIX) {
                let _ = tokio::task::spawn_blocking(move || run_user::scrub(&user)).await;
            }
        }
        let _ = jobdir::open_tree(&attempt.join(jobdir::WORK));
        let _ = std::fs::remove_dir_all(attempt.join(jobdir::WORK));
    }

    fn offers(&self) -> Offers {
        Offers {
            kinds: vec![KIND_MACHINE.into()],
            runtimes: vec![],
        }
    }

    fn output(&self, job_id: &str, attempt: u32, from: u64) -> Option<(Vec<u8>, u64)> {
        self.dirs.output(job_id, attempt, from)
    }

    fn progress(&self, job_id: &str, attempt: u32) -> Option<cua_proto::env::v1::HostJobProgress> {
        self.dirs.progress(job_id, attempt)
    }

    fn release(&self, job_id: &str, attempt: u32) {
        self.dirs.release(job_id, attempt);
    }

    fn stop_grace(&self) -> std::time::Duration {
        jobdir::KILL_GRACE
    }
}

/// A run's own user on a dedicated Linux host (as root).
pub mod run_user {
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// Every run user's name starts with this.
    pub const PREFIX: &str = "cuaj-";
    /// The file in an attempt directory naming its run user.
    pub const RECORD: &str = ".cua-user";

    /// The user of `run`: `cuaj-` and 12 hex digits of its hash (system
    /// user names are at most 32 characters).
    pub fn name(run: &str) -> String {
        use sha2::Digest as _;
        let digest = hex::encode(sha2::Sha256::digest(run.as_bytes()));
        format!("{PREFIX}{}", &digest[..12])
    }

    fn run(program: &str, args: &[&str]) -> Result<(), String> {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("{program}: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{program} {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// Held while a run user is added or removed. Members of one run start
    /// side by side; two `useradd`s of one name at once leave the name, its
    /// group and the ids out of step (the run's directories end up owned by
    /// an id its member does not run as).
    static ACCOUNTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn exists(user: &str) -> bool {
        Command::new("id")
            .args(["-u", user])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Makes `user` (a system user with its own group, no login shell,
    /// `home` as its home) unless it exists.
    pub fn ensure(user: &str, home: &Path) -> Result<(), String> {
        let _one_at_a_time = ACCOUNTS.lock().unwrap_or_else(|e| e.into_inner());
        if exists(user) {
            return Ok(());
        }
        let home = home.display().to_string();
        run(
            "useradd",
            &[
                "--system",
                "--user-group",
                "--no-create-home",
                "--home-dir",
                &home,
                "--shell",
                "/usr/sbin/nologin",
                user,
            ],
        )
    }

    /// Gives `user` its run's directories (and only it: 0700).
    pub fn own(user: &str, dirs: &[&Path]) -> Result<(), String> {
        for d in dirs {
            let d = d.display().to_string();
            run("chown", &["-R", &format!("{user}:{user}"), &d])?;
            run("chmod", &["0700", &d])?;
        }
        Ok(())
    }

    /// Gives `dir` (a run's data) back to root, owner-only, once its run's
    /// user is gone.
    pub fn disown(dir: &Path) {
        if dir.is_dir() {
            let d = dir.display().to_string();
            let _ = run("chown", &["-R", "0:0", &d]);
            let _ = run("chmod", &["0700", &d]);
        }
    }

    /// Ends `user`: its processes, its files in the shared temporary
    /// directories, the user itself.
    pub fn scrub(user: &str) {
        let _one_at_a_time = ACCOUNTS.lock().unwrap_or_else(|e| e.into_inner());
        if !user.starts_with(PREFIX) || !exists(user) {
            return;
        }
        let _ = run("pkill", &["-KILL", "-u", user]);
        std::thread::sleep(std::time::Duration::from_millis(300));
        for tmp in ["/tmp", "/var/tmp", "/dev/shm"] {
            let _ = run("find", &[tmp, "-xdev", "-user", user, "-delete"]);
        }
        let _ = run("userdel", &[user]);
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn a_run_users_name_is_short_stable_and_its_own() {
            let a = super::name("acct/run-1");
            assert_eq!(a, super::name("acct/run-1"));
            assert_ne!(a, super::name("acct/run-2"));
            assert!(a.starts_with("cuaj-") && a.len() == 17, "{a}");
        }
    }
}
