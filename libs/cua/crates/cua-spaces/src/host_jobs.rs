//! Cluster jobs on this host: [`SpacesExecutor`] runs every job member in a
//! Space, never as a plain process of the host.
//!
//! - **A fresh Space per member attempt**, from the job's image and kind,
//!   deleted when the attempt ends: on a Mac a macOS VM (Lume, with
//!   paravirtual GPU acceleration when the job asks for an accelerator); on
//!   a Linux host a container (gVisor when the engine has it; the NVIDIA GPU
//!   through the engine, gVisor's nvproxy, when the job asks for one). With
//!   no kind and no image the host picks: a macOS VM on a Mac, a container
//!   elsewhere.
//! - **The machine's own Space** (`kind = "machine"`, [`MachineExecutor`]):
//!   only on a host set up as dedicated (a cloud instance made for this
//!   work, `cua host setup --dedicated`), and its default kind there.
//!
//! What a member needs lives outside its throwaway Space, on the host
//! ([`cua_cluster::jobdir`]): its work directory (the job's bundle, its
//! control files), the run's data directory (checkpoints across attempts),
//! the uv and Hugging Face caches and `uv` itself, mounted into the Space (a
//! container's binds, a macOS VM's shared directories). Its output streams
//! into the host's output log as the Space's cua-spacesd sends it; its
//! start barrier and heartbeat are files in the mounted work directory;
//! its result file is read from there. A gang member reaches its peers
//! over the host's Cua mesh ([`cua_cluster::peers`]).
//!
//! A job's Space is an ordinary Space (it shows in this host's list while
//! it runs) named after the job and attempt (`cj-<hash>-<attempt>`), so a
//! host that restarts mid-job finds and deletes it. It is isolated: it gets
//! nothing a Space usually gets from this machine (no Cua Volume).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_cluster::jobdir::{self, JobDirs};
use cua_cluster::machine::MachineExecutor;
use cua_cluster::member::{self, GuestPaths, MemberCommand};
use cua_cluster::model::{KIND_MACHINE, KIND_PROCESS, process_refused};
use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, Offers};
use cua_cluster::peers::{GangPeers, MemberNet, PeerRouter, route_member};
use cua_cluster::{CancellationToken, JobSpec};
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_spacesd_client::{Command, ProcessEvent, SpacesdClient};

use crate::{SpaceCreate, Spaces};

/// The host's job runner (served by the daemon as `HostJobsService`).
pub use cua_cluster::NodeJobs;
/// Whether a Space runs a cluster job. A job Space is isolated: it gets
/// nothing a Space usually gets from this machine (no Cua Volume).
pub use cua_cluster::model::is_job_space;

/// The image a member runs when its job names none, on a Mac.
pub const DEFAULT_MACOS_JOB_IMAGE: &str = "ghcr.io/trycua/macos:26";
/// The image a member runs when its job names none, elsewhere.
pub const DEFAULT_LINUX_JOB_IMAGE: &str = "ghcr.io/trycua/linux:24.04-slim";

/// Smallest macOS VM a member gets (vCPUs, MiB): a GPU member's working
/// set is about two thirds of the VM's memory.
const MACOS_MIN: (u32, u64) = (4, 8192);
/// Smallest macOS VM a member that asks for an accelerator gets.
const MACOS_GPU_MIN: (u32, u64) = (6, 16384);

/// Where a macOS member sees its directories: links without spaces to the
/// shared directories (`/Volumes/My Shared Files/<name>`), since a path with
/// a space breaks the `#!` line of every console script a Python
/// environment installs there.
const MACOS_LINKS: &str = "/private/tmp/cua";

/// Where a macOS member's Python installs go (the VM's own disk).
const MACOS_PYTHON: &str = "/private/tmp/cua-python";

/// Whether `host`'s owner set this machine up as dedicated
/// (`HostPolicy.dedicated`): read from its policy at most every 2 s (the
/// runner asks on every status).
pub fn dedicated(host: cua_host::Host) -> impl Fn() -> bool + Send + Sync + 'static {
    let cache: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    move || {
        let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = *g
            && at.elapsed() < Duration::from_secs(2)
        {
            return v;
        }
        let v = host
            .policy()
            .ok()
            .flatten()
            .is_some_and(|p| p.provide_spaces && p.dedicated);
        *g = Some((Instant::now(), v));
        v
    }
}

type Flag = Arc<dyn Fn() -> bool + Send + Sync>;

/// Runs cluster job members in Spaces.
#[derive(Clone)]
pub struct SpacesExecutor {
    spaces: Spaces,
    offers: Offers,
    dirs: JobDirs,
    router: Option<Arc<dyn PeerRouter>>,
    machine: Option<MachineExecutor>,
    dedicated: Flag,
    /// macOS VMs this Mac runs for anything but its jobs (refreshed in the
    /// background).
    macos_others: Arc<AtomicU32>,
    /// Attempts running here (their directories are never swept).
    running: Arc<Mutex<HashSet<String>>>,
    /// One download of a guest's `uv` at a time.
    tools: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for SpacesExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpacesExecutor")
            .field("offers", &self.offers)
            .field("dirs", &self.dirs)
            .finish()
    }
}

impl SpacesExecutor {
    /// Members in Spaces on this machine (offering what its runtimes
    /// create, [`detect_offers`]), with their directories under its cua
    /// home. Starts the sweep of old run data, and on a Mac the count of
    /// macOS VMs it runs for anything else.
    pub async fn local(spaces: Spaces) -> Self {
        let offers = tokio::task::spawn_blocking(detect_offers)
            .await
            .unwrap_or_default();
        let me = Self {
            dirs: JobDirs::new(spaces.home_dir()),
            spaces,
            offers,
            router: None,
            machine: None,
            dedicated: Arc::new(|| false),
            macos_others: Arc::default(),
            running: Arc::default(),
            tools: Arc::default(),
        };
        me.start_background();
        me
    }

    /// Routes gang members' peers through `router` (the host's Cua mesh).
    pub fn with_peer_router(mut self, router: Arc<dyn PeerRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// Runs `kind = "machine"` members through `machine` while `dedicated`
    /// says this host is dedicated (asked at each call).
    pub fn with_machine(
        mut self,
        machine: MachineExecutor,
        dedicated: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        self.machine = Some(machine);
        self.dedicated = Arc::new(dedicated);
        self
    }

    /// The directories of members in fresh Spaces.
    pub fn dirs(&self) -> &JobDirs {
        &self.dirs
    }

    fn start_background(&self) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let me = self.clone();
        rt.spawn(async move {
            loop {
                let m = me.clone();
                let _ = tokio::task::spawn_blocking(move || m.sweep()).await;
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
        if cfg!(target_os = "macos") {
            let others = self.macos_others.clone();
            rt.spawn(async move {
                loop {
                    others.store(macos_vms_running().await, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            });
        }
    }

    fn sweep(&self) {
        let running = self.running.lock().expect("running").clone();
        let busy = |name: &str| running.contains(name);
        self.dirs.sweep(jobdir::KEEP, &busy);
        if let Some(m) = &self.machine {
            m.dirs().sweep(jobdir::KEEP, &busy);
        }
    }

    fn is_dedicated(&self) -> bool {
        self.machine.is_some() && (self.dedicated)()
    }

    /// The directories that hold an attempt (the machine's own Space keeps
    /// its members elsewhere when each run gets its own user).
    fn dirs_of(&self, job_id: &str, attempt: u32) -> &JobDirs {
        match &self.machine {
            Some(m) if m.dirs().attempt_dir(job_id, attempt).is_dir() => m.dirs(),
            _ => &self.dirs,
        }
    }

    async fn run_space(
        &self,
        job: &JobSpec,
        space: &str,
        cancel: &CancellationToken,
        out: &mut ExecOutcome,
    ) -> Result<ExecEnd, String> {
        let t0 = Instant::now();
        let image = job_image(job);
        let macos = cua_host::provided::is_macos_image(&image);
        let guest_os = if macos { "macos" } else { "linux" };
        let target = cua_host::uv::uv_target_for(guest_os, std::env::consts::ARCH)
            .ok_or_else(|| format!("no uv for {guest_os} on {}", std::env::consts::ARCH))?;
        // A container's user is not this host's: what it writes to is open.
        let dirs = self.dirs.clone();
        let j = job.clone();
        tokio::task::spawn_blocking(move || dirs.prepare(&j, !macos))
            .await
            .map_err(|e| e.to_string())??;
        let tools = self.dirs.tools_dir(target);
        {
            let _one = self.tools.lock().await;
            cua_host::uv::ensure_uv_in(&tools, target)
                .await
                .map_err(|e| format!("uv for the member's Space: {e}"))?;
        }
        let work = self.dirs.work_dir(&job.job_id, job.attempt);
        let data = self.dirs.run_data_dir(jobdir::run_of(job));
        let cache = self.dirs.cache_dir();
        let host_dirs = [(work, false), (data, false), (cache, false), (tools, true)];
        let (mounts, guest, links) = if macos {
            let shares: Vec<String> = host_dirs
                .iter()
                .map(|(h, _)| cua_vmm::lume::macos_share_path(h))
                .collect();
            let names = ["job", "data", "cache", "tools"];
            let links: Vec<(String, String)> = names
                .iter()
                .zip(&shares)
                .map(|(n, s)| (format!("{MACOS_LINKS}/{n}"), s.clone()))
                .collect();
            let mounts = host_dirs
                .iter()
                .zip(&shares)
                .map(|((h, ro), g)| cua_vmm::Mount {
                    host: h.clone(),
                    guest: g.clone(),
                    read_only: *ro,
                })
                .collect::<Vec<_>>();
            // A shared folder cannot `fsync`, which uv does for a Python
            // install: installs go on the VM's own disk.
            let guest = GuestPaths {
                python_installs: Some(MACOS_PYTHON.into()),
                ..GuestPaths::under(MACOS_LINKS, member::MACOS_PATH)
            };
            (mounts, guest, links)
        } else {
            let guest = GuestPaths::under("/cua", member::LINUX_PATH);
            let at = [&guest.work, &guest.data, &guest.cache, &guest.tools];
            let mounts = host_dirs
                .iter()
                .zip(at)
                .map(|((h, ro), g)| cua_vmm::Mount {
                    host: h.clone(),
                    guest: g.clone(),
                    read_only: *ro,
                })
                .collect::<Vec<_>>();
            (mounts, guest, Vec::new())
        };
        // Services with slow one-time work (the compat gateway's image pull
        // and first boot) start it now, beside the member Space's own.
        if let Some(p) = crate::job_services::provider() {
            for name in &job.services {
                let (p, name, job) = (p.clone(), name.clone(), job.clone());
                tokio::spawn(async move { p.prepare(&name, &job).await });
            }
        }
        let id = self
            .create(job, space, &image, macos, mounts, cancel)
            .await
            .map_err(|e| match e {
                ExecEnd::Error(e) => e,
                other => format!("{other:?}"),
            });
        let id = match id {
            Ok(id) => id,
            Err(e) if cancel.is_cancelled() => {
                out.create_ms = t0.elapsed().as_millis() as u32;
                let _ = e;
                return Ok(ExecEnd::Canceled);
            }
            Err(e) => return Err(e),
        };
        let result = self
            .in_space(job, space, &id, macos, &guest, &links, cancel, out, t0)
            .await;
        let t2 = Instant::now();
        if let Err(e) = self.spaces.delete(&id).await {
            tracing::warn!(space = %id, "a job's Space was not deleted: {e}");
            self.remove_space(space).await;
        }
        out.delete_ms = t2.elapsed().as_millis() as u32;
        result
    }

    /// Everything between the member's Space being ready and its delete.
    #[allow(clippy::too_many_arguments)]
    async fn in_space(
        &self,
        job: &JobSpec,
        space: &str,
        id: &str,
        macos: bool,
        guest: &GuestPaths,
        links: &[(String, String)],
        cancel: &CancellationToken,
        out: &mut ExecOutcome,
        t0: Instant,
    ) -> Result<ExecEnd, String> {
        let sp = self
            .spaces
            .space(id)
            .await
            .map_err(|e| format!("the job's Space is not reachable: {e}"))?;
        let client = sp
            .spacesd()
            .map_err(|e| {
                format!("the job's Space runs no cua-spacesd, so it cannot run a command: {e}")
            })?
            .clone();
        if !links.is_empty() {
            link_shares(&client, links).await?;
        }
        let net = match GangPeers::of(job) {
            None => MemberNet::machine(),
            // A gang of one has no peer to reach it: when this host cannot
            // see its Space's address (a container in the built-in Linux
            // runtime's own VM on a Mac), it listens on every address there.
            Some(g) if g.peers.len() == 1 => match member_net(&client, space).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::debug!(space, "a gang of one listens on 0.0.0.0: {e}");
                    MemberNet {
                        listen: std::net::Ipv4Addr::UNSPECIFIED.into(),
                        gateway: std::net::Ipv4Addr::UNSPECIFIED.into(),
                    }
                }
            },
            Some(_) => member_net(&client, space).await?,
        };
        let routed = route_member(self.router.as_deref(), job, net).await?;
        if let Some(r) = &routed {
            member::note_routes(&self.dirs, job, r);
        }
        let kind = if job.kind.is_empty() {
            if macos { "vm" } else { "container" }
        } else {
            job.kind.as_str()
        };
        let mut env = member::member_env(job, guest, kind, routed.as_ref());
        // Host services (JobSpec::services): started for this member,
        // reached through its Space's hotspot; dropped when it ends.
        let (services, job) = match self.start_services(job, &sp, guest, &mut env).await {
            Ok(v) => v,
            Err(e) => return Ok(ExecEnd::Error(e)),
        };
        let job = &job;
        out.create_ms = t0.elapsed().as_millis() as u32;
        let t1 = Instant::now();
        let end = member::run(
            MemberCommand {
                client: &client,
                job,
                dirs: &self.dirs,
                env,
                cwd: guest.work.clone(),
                user: None,
                // A container's user is not the host's, and a shared
                // folder in the built-in Linux runtime shows every file as
                // root's: what the member makes stays writable to it (and
                // removable by the host).
                open_files: !macos,
                // A Linux member on a Mac runs in the built-in Linux
                // runtime, whose shared folders make every file the Mac
                // user's (mode 0644): only the Space's root can rewrite
                // what the member wrote.
                as_root: !macos && cfg!(target_os = "macos"),
            },
            space,
            cancel,
        )
        .await;
        drop(routed);
        drop(services);
        out.run_ms = t1.elapsed().as_millis() as u32;
        if !job.result_path.is_empty() && matches!(end, ExecEnd::Exited(_)) {
            match member::read_result(&self.dirs, job) {
                Ok(r) => out.result = r,
                Err(e) => out.result_error = Some(e),
            }
        }
        Ok(end)
    }

    /// Starts `job`'s host services ([`crate::job_services`]): their files
    /// in the work directory, their environment, the hotspot that reaches
    /// their ports, and the command wrapped in the port forwarder. Returns
    /// what keeps them alive and the job to run.
    async fn start_services(
        &self,
        job: &JobSpec,
        sp: &crate::Space,
        guest: &GuestPaths,
        env: &mut std::collections::BTreeMap<String, String>,
    ) -> Result<(Vec<Box<dyn std::any::Any + Send + Sync>>, JobSpec), String> {
        if job.services.is_empty() {
            return Ok((vec![], job.clone()));
        }
        #[cfg(not(feature = "spaces-hotspot"))]
        {
            let _ = (sp, guest, env);
            return Err("this host serves no job services".into());
        }
        #[cfg(feature = "spaces-hotspot")]
        {
            use crate::job_services as js;
            let provider = js::provider().ok_or_else(|| {
                format!(
                    "this host serves no job services (asked for {})",
                    job.services.join(", ")
                )
            })?;
            let work = self.dirs.work_dir(&job.job_id, job.attempt);
            let mut keep: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![];
            let mut ports = std::collections::BTreeSet::new();
            let mut files = vec![(
                js::FORWARDER_PATH.to_string(),
                js::FORWARDER.as_bytes().to_vec(),
            )];
            for name in &job.services {
                let grant = provider.start(name, job).await?;
                ports.extend(grant.ports.iter().copied());
                for (k, v) in grant.env {
                    env.insert(k, js::expand(&v, &guest.work));
                }
                files.extend(grant.files);
                keep.push(grant.guard);
            }
            for (rel, data) in files {
                if rel.split('/').any(|c| c == ".." || c.is_empty()) {
                    return Err(format!("a job service wrote a bad path: {rel}"));
                }
                let p = work.join(&rel);
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            let hotspot = sp
                .start_hotspot(crate::hotspot::HotspotOptions {
                    socks_port: 0,
                    set_system_proxy: false,
                    bypass: vec![],
                    dialer: js::loopback_dialer(ports.clone()),
                })
                .await
                .map_err(|e| format!("the job's Space cannot reach its services: {e}"))?;
            let socks = hotspot.socks_address().to_string();
            keep.push(Box::new(hotspot));
            let mut run = job.clone();
            run.argv = js::wrap(&job.argv, &guest.work, &socks, &ports);
            Ok((keep, run))
        }
    }

    async fn create(
        &self,
        job: &JobSpec,
        space: &str,
        image: &str,
        macos: bool,
        mounts: Vec<cua_vmm::Mount>,
        cancel: &CancellationToken,
    ) -> Result<String, ExecEnd> {
        let place = |e: cua_sandbox_core::placement::PlacementError| ExecEnd::Error(e.to_string());
        let kind = Kind::parse(&job.kind).map_err(place)?;
        let mut runtime = Runtime::parse(&job.runtime).map_err(place)?;
        let gpu = !job.accelerator.is_empty();
        let (cpus, memory_mb) = member_size(job, macos);
        if macos && runtime == Runtime::Auto {
            // Lume runs macOS members (paravirtual GPU acceleration).
            runtime = Runtime::parse("lume").map_err(place)?;
        }
        let opts = SpaceCreate {
            image: Some(image.to_string()),
            on: Some(On::Local),
            kind,
            runtime,
            name: Some(space.to_string()),
            cpus: Some(cpus),
            memory_mb: Some(memory_mb),
            wait: Some(true),
            // Cancel finds the create by the job's Space name.
            create_id: Some(space.to_string()),
            // The runtime's own GPU: paravirtual for a macOS VM, NVIDIA
            // (nvproxy under gVisor) for a container.
            gpu: gpu.then(|| "auto".to_string()),
            mounts,
            // A member's command runs through the Space's cua-spacesd, whatever the
            // image is called: wait for it (a custom job image is not "canonical",
            // so it would otherwise get one probe and the job would fail on a race).
            spacesd: Some(true),
            ..Default::default()
        };
        let created = tokio::select! {
            r = self.spaces.create(opts) => r,
            _ = cancel.cancelled() => {
                let _ = self.spaces.cancel_create(space).await;
                return Err(ExecEnd::Canceled);
            }
        };
        match created {
            Ok(c) => match c.ready() {
                Ok(info) => Ok(info.id),
                Err(p) => Err(ExecEnd::Error(format!("{} is still starting", p.id))),
            },
            Err(e) => Err(ExecEnd::Error(format!(
                "the job's Space did not start: {e}"
            ))),
        }
    }
}

/// The vCPUs and MiB a member's Space gets: what its job reserves, and on a
/// macOS VM at least [`MACOS_MIN`], or [`MACOS_GPU_MIN`] when the job asks
/// for an accelerator (the guest's GPU working set is about two thirds of
/// its memory).
fn member_size(job: &JobSpec, macos: bool) -> (u32, u64) {
    let cpus = job.millicpus.div_ceil(1000).max(1);
    if !macos {
        return (cpus, job.memory_mb);
    }
    let (c, m) = if job.accelerator.is_empty() {
        MACOS_MIN
    } else {
        MACOS_GPU_MIN
    };
    (cpus.max(c), job.memory_mb.max(m))
}

/// The image a member runs: its job's, else this host's default for its kind
/// (a macOS VM on a Mac, Linux elsewhere and for containers).
pub fn job_image(job: &JobSpec) -> String {
    if !job.image.trim().is_empty() {
        return job.image.trim().to_string();
    }
    // Only a VM (or no kind) is a macOS one: containers and microVMs are Linux.
    if cfg!(target_os = "macos") && !matches!(job.kind.as_str(), "container" | "microvm") {
        DEFAULT_MACOS_JOB_IMAGE.into()
    } else {
        DEFAULT_LINUX_JOB_IMAGE.into()
    }
}

/// Makes a macOS member's links to its shared directories (waiting for
/// macOS to mount them).
async fn link_shares(client: &SpacesdClient, links: &[(String, String)]) -> Result<(), String> {
    let mut script = String::from("set -e\nmkdir -p \"$1\"\nshift\n");
    script.push_str(
        "while [ $# -gt 1 ]; do\n  i=0\n  while [ ! -d \"$2\" ]; do\n    i=$((i+1))\n    \
         [ $i -gt 240 ] && { echo \"cua: $2 never appeared\" >&2; exit 1; }\n    sleep 0.5\n  \
         done\n  ln -sfn \"$2\" \"$1\"\n  shift 2\ndone\n",
    );
    let mut cmd = Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(MACOS_LINKS)
        .timeout(Duration::from_secs(150));
    for (link, target) in links {
        cmd = cmd.arg(link.clone()).arg(target.clone());
    }
    let out = client
        .run(cmd)
        .await
        .map_err(|e| format!("linking the member's directories: {e}"))?;
    if out.success() {
        Ok(())
    } else {
        Err(format!(
            "linking the member's directories: {}",
            out.stderr_str().trim()
        ))
    }
}

/// Where a member's Space sits on this host's network: its own address
/// (a VM's from its cua-spacesd endpoint, a container's from its engine)
/// and this host's address on that network.
async fn member_net(client: &SpacesdClient, space: &str) -> Result<MemberNet, String> {
    let from_endpoint = client
        .endpoint()
        .base_url()
        .host_str()
        .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
        .filter(|ip| !ip.is_loopback());
    let ip = match from_endpoint {
        Some(ip) => ip,
        None => {
            let name = space.to_string();
            tokio::task::spawn_blocking(move || container_ip(&name))
                .await
                .ok()
                .flatten()
                .ok_or_else(|| format!("could not find the address of the member's Space {space}"))?
                .into()
        }
    };
    let std::net::IpAddr::V4(v4) = ip else {
        return Err(format!(
            "the member's Space has an IPv6 address only ({ip})"
        ));
    };
    let gateway = cua_cluster::sysinfo::gateway_for(v4).ok_or_else(|| {
        format!("this computer has no address on the network of its member's Space ({v4})")
    })?;
    Ok(MemberNet {
        listen: ip,
        gateway: gateway.into(),
    })
}

/// A container's address on its engine's network.
fn container_ip(name: &str) -> Option<std::net::Ipv4Addr> {
    let out = std::process::Command::new("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}} {{end}}",
            name,
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .find_map(|w| w.parse().ok())
}

/// Starts `job`'s host services for a member in this host's own Space
/// (`kind = "machine"`): what keeps them alive, and their environment and
/// files (`None`: it asked for none). An `Err` fails the member before it
/// starts: a member that asks for services never runs without them.
async fn machine_services(
    job: &JobSpec,
) -> Result<
    (
        Vec<Box<dyn std::any::Any + Send + Sync>>,
        Option<cua_cluster::machine::MemberExtras>,
    ),
    String,
> {
    if job.services.is_empty() {
        return Ok((vec![], None));
    }
    #[cfg(not(feature = "spaces-hotspot"))]
    {
        Err(format!(
            "this host serves no job services (asked for {})",
            job.services.join(", ")
        ))
    }
    #[cfg(feature = "spaces-hotspot")]
    {
        let provider = crate::job_services::provider().ok_or_else(|| {
            format!(
                "this host serves no job services (asked for {})",
                job.services.join(", ")
            )
        })?;
        let mut keep: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![];
        let mut extras = cua_cluster::machine::MemberExtras::default();
        for name in &job.services {
            let grant = provider.start(name, job).await?;
            extras.env.extend(grant.env);
            extras.files.extend(grant.files);
            keep.push(grant.guard);
        }
        Ok((keep, Some(extras)))
    }
}

/// macOS VMs Lume runs on this Mac that are not job members (Apple's
/// license allows two per Mac, and Apple Virtualization refuses a third).
async fn macos_vms_running() -> u32 {
    let base = std::env::var("LUME_API").unwrap_or_else(|_| "http://127.0.0.1:7777".into());
    let client = cua_vmm::lume::LumeClient::new(base);
    match tokio::time::timeout(Duration::from_secs(4), client.list()).await {
        Ok(Ok(vms)) => vms
            .iter()
            .filter(|v| {
                v.status == "running"
                    && v.os.eq_ignore_ascii_case("macos")
                    && !is_job_space(&v.name)
            })
            .count() as u32,
        _ => 0,
    }
}

#[async_trait]
impl JobExecutor for SpacesExecutor {
    async fn run(&self, job: &JobSpec, space: &str, cancel: CancellationToken) -> ExecOutcome {
        if job.kind == KIND_MACHINE {
            return match &self.machine {
                Some(m) if self.is_dedicated() => {
                    self.running.lock().expect("running").insert(space.into());
                    // The member runs on this host itself: it reaches its
                    // services' loopback ports directly (no hotspot).
                    let out = match machine_services(job).await {
                        Ok((guards, extras)) => {
                            let out = m.run_with(job, space, cancel, extras).await;
                            drop(guards);
                            out
                        }
                        Err(e) => ExecOutcome::ended(ExecEnd::Error(e)),
                    };
                    self.running.lock().expect("running").remove(space);
                    out
                }
                _ => ExecOutcome::ended(ExecEnd::Error(
                    "this host is not dedicated, so it has no computer Space for jobs".into(),
                )),
            };
        }
        if job.kind == KIND_PROCESS {
            return ExecOutcome::ended(ExecEnd::Error(process_refused()));
        }
        self.running.lock().expect("running").insert(space.into());
        let mut out = ExecOutcome::ended(ExecEnd::Exited(0));
        let t0 = Instant::now();
        out.end = match self.run_space(job, space, &cancel, &mut out).await {
            Ok(end) => end,
            Err(e) => {
                jobdir::append(
                    &self.dirs.output_log(&job.job_id, job.attempt),
                    format!("cua: {e}\n").as_bytes(),
                );
                if out.create_ms == 0 {
                    out.create_ms = t0.elapsed().as_millis() as u32;
                }
                ExecEnd::Error(e)
            }
        };
        // The runner keeps the tail: read a little more than it keeps so it
        // can say whether it cut.
        let max = (job.max_output_bytes as usize).saturating_mul(2).max(1);
        out.stdout = self.dirs.output_tail(&job.job_id, job.attempt, max);
        let dirs = self.dirs.clone();
        let j = job.clone();
        let _ = tokio::task::spawn_blocking(move || dirs.finish(&j)).await;
        self.running.lock().expect("running").remove(space);
        out
    }

    async fn remove_space(&self, space: &str) {
        if let Some(m) = &self.machine
            && m.dirs().jobs_dir().join(space).is_dir()
        {
            m.remove_space(space).await;
            return;
        }
        let id = format!("local:{space}");
        if self.spaces.delete(&id).await.is_err() {
            let _ = self.spaces.sandboxes().delete_local_instance(space).await;
        }
    }

    fn offers(&self) -> Offers {
        // Checked at each call: a provider installed while the daemon runs
        // is offered without a restart.
        let mut base = self.offers.clone();
        if !cfg!(target_os = "macos") {
            // Every job member mounts the host's work, data, cache and tools
            // directories, which QEMU cannot do: a `vm` member here would
            // fail to start on every retry. Not offering it makes the
            // placement refuse it up front. (Spaces made outside a job still
            // use QEMU: this is only what the job executor offers.)
            base.kinds.retain(|k| k != "vm");
            base.runtimes.retain(|r| r != "qemu");
        }
        let mut o = with_external_runtimes(
            base,
            &cua_sandbox_core::external::Registry::global().providers(),
        );
        if self.is_dedicated() {
            o.kinds.push(KIND_MACHINE.into());
        }
        o
    }

    fn kind_of(&self, spec: &JobSpec) -> String {
        if !spec.kind.is_empty() {
            return spec.kind.clone();
        }
        if self.is_dedicated() {
            return KIND_MACHINE.into();
        }
        if cua_host::provided::is_macos_image(&job_image(spec)) {
            "vm".into()
        } else {
            "container".into()
        }
    }

    fn kind_limit(&self, kind: &str) -> Option<(u32, String)> {
        if kind != "vm" || !cfg!(target_os = "macos") {
            return None;
        }
        let others = self.macos_others.load(Ordering::Relaxed);
        let max = cua_host::provided::MACOS_VM_LICENSE_LIMIT.saturating_sub(others);
        Some((
            max,
            format!(
                "this Mac runs {} macOS VMs already ({others} for something else): {}",
                cua_host::provided::MACOS_VM_LICENSE_LIMIT,
                cua_host::provided::MACOS_LIMIT_REASON
            ),
        ))
    }

    fn output(&self, job_id: &str, attempt: u32, from: u64) -> Option<(Vec<u8>, u64)> {
        self.dirs_of(job_id, attempt).output(job_id, attempt, from)
    }

    fn progress(&self, job_id: &str, attempt: u32) -> Option<cua_proto::env::v1::HostJobProgress> {
        self.dirs_of(job_id, attempt).progress(job_id, attempt)
    }

    fn release(&self, job_id: &str, attempt: u32) {
        self.dirs_of(job_id, attempt).release(job_id, attempt);
    }

    fn stop_grace(&self) -> Duration {
        jobdir::KILL_GRACE
    }
}

/// Runs `job`'s command through a Space's cua-spacesd, keeping the tail of
/// each stream in `out`; stops it on `cancel` or after its timeout (the
/// guest's own timeout first, this one as the backstop). For elastic
/// providers' sandboxes, which have no host directories to mount.
pub async fn run_command(
    client: &SpacesdClient,
    job: &JobSpec,
    cancel: &CancellationToken,
    out: &mut ExecOutcome,
) -> ExecEnd {
    let timeout = Duration::from_secs(u64::from(job.timeout_secs));
    let mut cmd = Command::new(&job.argv[0])
        .args(job.argv[1..].iter().cloned())
        .timeout(timeout);
    for (k, v) in &job.env {
        cmd = cmd.env(k, v);
    }
    let mut handle = match client.spawn(cmd).await {
        Ok(h) => h,
        Err(e) => return ExecEnd::Error(format!("the command did not start: {e}")),
    };
    let max = job.max_output_bytes as usize;
    let keep = |buf: &mut Vec<u8>, data: &[u8]| {
        buf.extend_from_slice(data);
        if buf.len() > 2 * max {
            let cut = buf.len() - max;
            buf.drain(..cut);
        }
    };
    let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(30);
    loop {
        let event = tokio::select! {
            e = handle.next_event() => e,
            _ = cancel.cancelled() => {
                let _ = handle.kill().await;
                return ExecEnd::Canceled;
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = handle.kill().await;
                return ExecEnd::TimedOut;
            }
        };
        match event {
            Ok(Some(ProcessEvent::Stdout { data, .. })) => keep(&mut out.stdout, &data),
            Ok(Some(ProcessEvent::Stderr { data, .. })) => keep(&mut out.stderr, &data),
            Ok(Some(ProcessEvent::Pty { data, .. })) => keep(&mut out.stdout, &data),
            Ok(Some(ProcessEvent::Exit(status))) => {
                if status.timed_out {
                    return ExecEnd::TimedOut;
                }
                if let Some(code) = status.code {
                    return ExecEnd::Exited(code);
                }
                if let Some(e) = status.error {
                    // The supervisor could not run it ("executable not
                    // found"): the job's own failure, not the host's.
                    out.stderr.extend_from_slice(e.as_bytes());
                    return ExecEnd::Exited(127);
                }
                // Killed by a signal.
                return ExecEnd::Exited(128 + status.signal.map_or(9, |s| s as i32));
            }
            Ok(None) => {
                return ExecEnd::Error("the command's output ended without an exit".into());
            }
            Err(e) => return ExecEnd::Error(format!("lost the command: {e}")),
        }
    }
}

/// Reads `job`'s result file (its tail) into `out`, through the sandbox's
/// cua-spacesd (elastic providers' sandboxes).
pub async fn read_result(client: &SpacesdClient, job: &JobSpec, out: &mut ExecOutcome) {
    match client.download(&job.result_path).await {
        Ok(bytes) => {
            let max = job.max_output_bytes as usize;
            let start = bytes.len().saturating_sub(max);
            out.result = bytes[start..].to_vec();
        }
        Err(e) => out.result_error = Some(format!("result file {}: {e}", job.result_path)),
    }
}

/// `offers` plus the external runtime providers installed on this machine
/// ([`cua_sandbox_core::external`]): each one's runtime word, and the kind
/// it presents as when not offered yet. Unchanged without providers.
pub fn with_external_runtimes(
    mut offers: Offers,
    providers: &[std::sync::Arc<cua_sandbox_core::external::ExternalProvider>],
) -> Offers {
    for p in providers {
        let kind = p.kind.as_str().to_string();
        if !offers.kinds.contains(&kind) {
            offers.kinds.push(kind);
        }
        let runtime = p.name().to_string();
        if !offers.runtimes.contains(&runtime) {
            offers.runtimes.push(runtime);
        }
    }
    offers
}

/// What this machine's runtimes create: containers with Docker (`runc`,
/// and `gvisor` when Docker has `runsc`), VMs with QEMU and KVM (Linux) or
/// Lume (macOS), microVMs with Firecracker and KVM when this build knows
/// the kind.
pub fn detect_offers() -> Offers {
    let ok = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let on_path = |bin: &str| {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
    };
    let mut kinds = Vec::new();
    let mut runtimes = Vec::new();
    if let Some(info) = ok("docker", &["info", "--format", "{{json .Runtimes}}"]) {
        kinds.push("container".to_string());
        if info.contains("runsc") {
            runtimes.push("gvisor".to_string());
        }
        runtimes.push("runc".to_string());
    }
    let kvm = std::path::Path::new("/dev/kvm").exists();
    if cfg!(target_os = "macos") {
        // A Mac without its own Docker runs Linux Spaces on the built-in
        // runtime (Docker and gVisor in a VM it starts on demand).
        if !kinds.iter().any(|k| k == "container")
            && cua_vmm::managed::LinuxSource::current().allows_builtin()
        {
            kinds.push("container".into());
            runtimes.push("gvisor".into());
        }
        kinds.push("vm".into());
        runtimes.push("lume".into());
        if cua_vmm::containerization::install::supported_host() && Kind::parse("microvm").is_ok() {
            kinds.push("microvm".into());
            runtimes.push("containerization".into());
        }
    } else if kvm && (on_path("qemu-system-x86_64") || on_path("qemu-system-aarch64")) {
        kinds.push("vm".into());
        runtimes.push("qemu".into());
    }
    if kvm && on_path("firecracker") && Kind::parse("microvm").is_ok() {
        kinds.push("microvm".into());
        runtimes.push("firecracker".into());
    }
    kinds.dedup();
    Offers { kinds, runtimes }
}

#[cfg(test)]
mod external_offers_tests {
    use super::*;
    use cua_sandbox_core::external::ExternalProvider;
    use std::sync::Arc;

    fn provider(name: &str, kind: Kind) -> Arc<ExternalProvider> {
        Arc::new(ExternalProvider {
            manifest: serde_json::from_value(serde_json::json!({
                "protocol": 1, "name": name, "kind": kind.as_str()
            }))
            .unwrap(),
            path: format!("/x/cua-runtime-{name}").into(),
            kind,
        })
    }

    #[test]
    fn a_node_advertises_its_external_runtimes_and_nothing_more_without() {
        let base = Offers {
            kinds: vec!["container".into()],
            runtimes: vec!["gvisor".into(), "runc".into()],
        };
        assert_eq!(with_external_runtimes(base.clone(), &[]), base);
        let offers = with_external_runtimes(
            base,
            &[
                provider("acme", Kind::Vm),
                provider("beta", Kind::Container),
            ],
        );
        assert_eq!(offers.kinds, ["container", "vm"]);
        assert_eq!(offers.runtimes, ["gvisor", "runc", "acme", "beta"]);
    }

    #[test]
    fn a_job_without_an_image_runs_this_hosts_default() {
        let mut job = JobSpec::default();
        let want = if cfg!(target_os = "macos") {
            DEFAULT_MACOS_JOB_IMAGE
        } else {
            DEFAULT_LINUX_JOB_IMAGE
        };
        assert_eq!(job_image(&job), want);
        job.image = " ghcr.io/trycua/linux:24.04 ".into();
        assert_eq!(job_image(&job), "ghcr.io/trycua/linux:24.04");
        // A container is Linux wherever it runs.
        let job = JobSpec {
            kind: "container".into(),
            ..Default::default()
        };
        assert_eq!(job_image(&job), DEFAULT_LINUX_JOB_IMAGE);
    }
}

#[cfg(test)]
mod member_size_tests {
    use super::*;

    #[test]
    fn a_gpu_member_on_a_mac_gets_the_gpu_vm_size() {
        let job = JobSpec {
            millicpus: 2000,
            memory_mb: 4096,
            ..Default::default()
        };
        assert_eq!(member_size(&job, true), MACOS_MIN);
        let gpu = JobSpec {
            accelerator: "gpu".into(),
            ..job.clone()
        };
        assert_eq!(member_size(&gpu, true), MACOS_GPU_MIN);
        // From the wire too: the host learns the accelerator from the spec.
        let wire = JobSpec::from_pb(&gpu.to_pb());
        assert_eq!(member_size(&wire, true), (6, 16384));
        // Elsewhere the job's own reservation; a bigger one wins on a Mac.
        assert_eq!(member_size(&gpu, false), (2, 4096));
        let big = JobSpec {
            millicpus: 8000,
            memory_mb: 32768,
            ..gpu
        };
        assert_eq!(member_size(&big, true), (8, 32768));
    }
}
