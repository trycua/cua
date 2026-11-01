//! Sandboxes: records, provisioning (a Space from the image's layer), the
//! entrypoint and its result, timeouts, termination, tunnels and snapshots.

use crate::backend::{BackendError, CreateExtras, CreateSpec, Identity, Placement, Source};
use crate::modal::db::{Tags, now_s};
use crate::modal::exec::{self, Exit, Session};
use crate::modal::pb::api;
use crate::modal::{Caller, Modal, errors};
use base64::Engine;
use cua_spacesd_client::{Command, SpacesdClient};
use prost::Message;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub const ENTRY_TAG: &str = "modal-entry";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SbState {
    /// Created; the Space is being made.
    Scheduling,
    Running,
    Finished,
}

/// Why a GPU request is refused, and what to do instead.
pub const GPU_HINT: &str = "Cua Spaces take the runtime's GPU option, not a Modal GPU model: set experimental_options={\"cua.gpu\": \"nvidia\" (or auto, virgl, ...), \"cua.on\": \"host:<machine with the GPU>\"} and the gpu= argument is then only documentation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResultRec {
    pub status: i32,
    pub exitcode: i32,
    pub exception: String,
}

impl ResultRec {
    pub fn to_pb(&self) -> api::GenericResult {
        api::GenericResult {
            status: self.status,
            exitcode: self.exitcode,
            exception: self.exception.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TunnelRec {
    pub container_port: u32,
    pub host: String,
    pub port: u32,
    pub unencrypted_host: Option<String>,
    pub unencrypted_port: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRec {
    pub id: String,
    pub key_id: String,
    pub identity_daemon: Option<String>,
    pub identity_token: Option<String>,
    pub app_id: String,
    pub name: Option<String>,
    pub tags: Tags,
    pub created_at: f64,
    pub image_id: String,
    pub task_id: String,
    pub cua_name: Option<String>,
    pub state: SbState,
    pub started_at: f64,
    pub finished_at: f64,
    pub result: Option<ResultRec>,
    /// The client's `Sandbox` message, prost-encoded then base64.
    pub definition: String,
    /// Environment resolved at create (secrets, image config).
    pub env: BTreeMap<String, String>,
    pub error: Option<String>,
    pub tunnels: Vec<TunnelRec>,
    /// This sandbox was restored from a memory snapshot.
    #[serde(default)]
    pub restored: bool,
    /// Made through the V2 RPCs (`SandboxCreateV2`).
    #[serde(default)]
    pub v2: bool,
    /// The image of the exit snapshot (`enable_exit_snapshot`), once taken.
    #[serde(default)]
    pub exit_snapshot: Option<String>,
    #[serde(default)]
    pub exit_snapshot_error: Option<String>,
    /// Credentials given with a V2 create for cloud bucket mounts, by mount
    /// path (`cloud_bucket_mount_credentials`).
    #[serde(default)]
    pub bucket_credentials: BTreeMap<String, BTreeMap<String, String>>,
}

impl SandboxRec {
    pub fn identity(&self) -> Identity {
        Identity {
            daemon: self.identity_daemon.clone(),
            token: self.identity_token.clone(),
        }
    }

    pub fn def(&self) -> api::Sandbox {
        base64::engine::general_purpose::STANDARD
            .decode(&self.definition)
            .ok()
            .and_then(|b| api::Sandbox::decode(b.as_slice()).ok())
            .unwrap_or_default()
    }

    pub fn running(&self) -> bool {
        self.state != SbState::Finished
    }

    pub fn handle_metadata(&self) -> api::SandboxHandleMetadata {
        api::SandboxHandleMetadata {
            result: self.result.as_ref().map(ResultRec::to_pb),
            app_id: self.app_id.clone(),
        }
    }

    pub fn info(&self) -> api::SandboxInfo {
        let def = self.def();
        api::SandboxInfo {
            id: self.id.clone(),
            created_at: self.created_at,
            task_info: Some(api::TaskInfo {
                id: self.task_id.clone(),
                started_at: self.started_at,
                finished_at: self.finished_at,
                result: self.result.as_ref().map(ResultRec::to_pb),
                sandbox_id: self.id.clone(),
                ..Default::default()
            }),
            app_id: self.app_id.clone(),
            tags: self
                .tags
                .iter()
                .map(|(k, v)| api::SandboxTag {
                    tag_name: k.clone(),
                    tag_value: v.clone(),
                })
                .collect(),
            name: self.name.clone().unwrap_or_default(),
            image_id: self.image_id.clone(),
            timeout_secs: def.timeout_secs,
            idle_timeout_secs: def.idle_timeout_secs,
            tunnels: self.tunnels.iter().map(tunnel_pb).collect(),
            metadata: Some(self.handle_metadata()),
            ..Default::default()
        }
    }
}

pub fn tunnel_pb(t: &TunnelRec) -> api::TunnelData {
    api::TunnelData {
        host: t.host.clone(),
        port: t.port,
        unencrypted_host: t.unencrypted_host.clone(),
        unencrypted_port: t.unencrypted_port,
        container_port: t.container_port,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRec {
    pub id: String,
    pub key_id: String,
    pub sandbox_id: String,
    pub checkpoint: String,
    pub app_id: String,
    pub image_id: String,
    pub definition: String,
    pub env: BTreeMap<String, String>,
    pub tags: Tags,
    pub created_at: f64,
}

/// Generic statuses (`GenericResult.GenericStatus`).
pub mod status {
    pub const SUCCESS: i32 = 1;
    pub const FAILURE: i32 = 2;
    pub const TERMINATED: i32 = 3;
    pub const TIMEOUT: i32 = 4;
    pub const IDLE_TIMEOUT: i32 = 7;
}

/// What to run.
pub struct ExecReq {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub user: Option<String>,
    pub timeout: Option<Duration>,
    pub pty: Option<(u32, u32)>,
    pub stdin: bool,
    pub tag: Option<String>,
}

/// The sandbox's entrypoint argv: the image's ENTRYPOINT then the
/// definition's args, else the image's CMD.
pub fn entrypoint_argv(def: &api::Sandbox, built: &crate::modal::image::Built) -> Vec<String> {
    let mut v = built.entrypoint.clone().unwrap_or_default();
    if !def.entrypoint_args.is_empty() {
        v.extend(def.entrypoint_args.iter().cloned());
    } else if let Some(cmd) = &built.cmd {
        v.extend(cmd.iter().cloned());
    }
    v
}

impl Modal {
    pub fn sandbox(&self, id: &str) -> Option<SandboxRec> {
        self.db.read(|d| d.sandboxes.get(id).cloned())
    }

    /// A sandbox the caller owns.
    pub fn sandbox_of(&self, caller: &Caller, id: &str) -> Result<SandboxRec, tonic::Status> {
        match self.sandbox(id) {
            Some(r) if r.key_id == caller.key_id() => Ok(r),
            _ => Err(errors::not_found(format!("Sandbox {id} not found"))),
        }
    }

    pub fn update_sandbox<R>(&self, id: &str, f: impl FnOnce(&mut SandboxRec) -> R) -> Option<R> {
        self.db.write(|d| d.sandboxes.get_mut(id).map(f))
    }

    /// A receiver that fires when the sandbox's state changes.
    pub async fn sb_events(&self, id: &str) -> watch::Receiver<u64> {
        let mut m = self.live.events.lock().await;
        m.entry(id.to_string())
            .or_insert_with(|| watch::channel(0).0)
            .subscribe()
    }

    pub async fn notify_sb(&self, id: &str) {
        let m = self.live.events.lock().await;
        if let Some(tx) = m.get(id) {
            tx.send_modify(|v| *v += 1);
        }
    }

    /// Records activity (for `idle_timeout`).
    pub async fn touch(&self, id: &str) {
        self.live
            .activity
            .lock()
            .await
            .insert(id.to_string(), tokio::time::Instant::now());
    }

    /// Creates the record and starts provisioning.
    pub async fn create_sandbox(
        self: &Arc<Self>,
        caller: &Caller,
        app_id: &str,
        def: api::Sandbox,
        tags: Tags,
        v2: bool,
        extra_env: BTreeMap<String, String>,
    ) -> Result<SandboxRec, tonic::Status> {
        self.create_sandbox_from(
            caller,
            app_id,
            def,
            tags,
            v2,
            extra_env,
            Default::default(),
            None,
        )
        .await
    }

    /// [`Modal::create_sandbox`], optionally restoring a memory snapshot.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_sandbox_from(
        self: &Arc<Self>,
        caller: &Caller,
        app_id: &str,
        def: api::Sandbox,
        tags: Tags,
        v2: bool,
        extra_env: BTreeMap<String, String>,
        bucket_credentials: BTreeMap<String, BTreeMap<String, String>>,
        from: Option<SnapshotRec>,
    ) -> Result<SandboxRec, tonic::Status> {
        let id = if v2 {
            crate::modal::ids::new_id_v2("sb")
        } else {
            crate::modal::ids::new_id("sb")
        };
        let name = def.name.clone().filter(|n| !n.is_empty());
        if let Some(n) = &name {
            let taken = self.db.read(|d| {
                d.sandboxes.values().any(|s| {
                    s.key_id == caller.key_id()
                        && s.app_id == app_id
                        && s.running()
                        && s.name.as_deref() == Some(n)
                })
            });
            if taken {
                return Err(errors::exists(format!(
                    "Sandbox with name '{n}' already exists in this app"
                )));
            }
        }
        let rec = SandboxRec {
            id: id.clone(),
            key_id: caller.key_id(),
            identity_daemon: caller.0.daemon.clone(),
            identity_token: caller.0.token.clone(),
            app_id: app_id.to_string(),
            name,
            tags,
            created_at: now_s(),
            image_id: def.image_id.clone(),
            task_id: if v2 {
                crate::modal::ids::new_task_id_v2()
            } else {
                crate::modal::ids::new_task_id()
            },
            cua_name: None,
            state: SbState::Scheduling,
            started_at: 0.0,
            finished_at: 0.0,
            result: None,
            definition: base64::engine::general_purpose::STANDARD.encode(def.encode_to_vec()),
            env: BTreeMap::new(),
            error: None,
            tunnels: Vec::new(),
            restored: false,
            v2,
            exit_snapshot: None,
            exit_snapshot_error: None,
            bucket_credentials: BTreeMap::new(),
        };
        let mut rec = rec;
        rec.env = extra_env;
        rec.bucket_credentials = bucket_credentials;
        self.db.write(|d| {
            d.sandboxes.insert(id.clone(), rec.clone());
        });
        let me = self.clone();
        let sid = id.clone();
        tokio::spawn(async move {
            me.provision(&sid, from).await;
        });
        Ok(rec)
    }

    /// What a create asks for beyond placement: the runtime's GPU option and
    /// disk size (`cua.gpu`, `cua.disk_gb`).
    pub fn extras_of(options: &std::collections::HashMap<String, String>) -> CreateExtras {
        CreateExtras {
            gpu: options.get("cua.gpu").filter(|g| !g.is_empty()).cloned(),
            disk_gb: options.get("cua.disk_gb").and_then(|v| v.parse().ok()),
            ..Default::default()
        }
    }

    /// Placement and size from the definition and its `cua.*` options.
    fn placement_of(def: &api::Sandbox) -> (Placement, Vec<String>) {
        let mut p = Placement::default();
        let mut warnings = Vec::new();
        if let Some(r) = &def.resources {
            let milli = if r.milli_cpu_max > 0 {
                r.milli_cpu_max
            } else {
                r.milli_cpu
            };
            if milli > 0 {
                p.cpus = Some(milli.div_ceil(1000).max(1));
            }
            let mem = if r.memory_mb_max > 0 {
                r.memory_mb_max
            } else {
                r.memory_mb
            };
            if mem > 0 {
                p.memory_mb = Some(u64::from(mem));
            }
        }
        for (k, v) in &def.experimental_options_v2 {
            match k.as_str() {
                "cua.on" => p.on = Some(v.clone()),
                "cua.kind" => p.kind = Some(v.clone()),
                "cua.cpus" => p.cpus = v.parse().ok().or(p.cpus),
                "cua.memory_mb" => p.memory_mb = v.parse().ok().or(p.memory_mb),
                "cua.gpu" | "cua.disk_gb" => {}
                _ => {}
            }
        }
        // Modal's `runtime` is the isolation kind (`gvisor` or `vm`). The SDK
        // sends `gvisor` for every Sandbox that did not ask otherwise, so only
        // `vm` is a request: the default leaves the kind to the gateway's
        // `--kind` (as E2B and Daytona do). An explicit `cua.kind` wins.
        if p.kind.is_none()
            && let Some(r) = def.runtime.as_ref().filter(|r| r.as_str() == "vm")
        {
            p.kind = Some(r.clone());
        }
        if let Some(sp) = &def.scheduler_placement
            && !sp.regions.is_empty()
            && p.on.is_none()
        {
            warnings.push(format!(
                "region {:?} is a Modal region; the Space is placed by the gateway's --on (set experimental_options={{\"cua.on\": ...}} to choose)",
                sp.regions
            ));
        }
        (p, warnings)
    }

    /// Makes the Space and starts the entrypoint. `from` is a snapshot to
    /// restore.
    pub async fn provision(self: &Arc<Self>, id: &str, from: Option<SnapshotRec>) {
        let r = self.provision_inner(id, from).await;
        if let Err(e) = r {
            tracing::warn!("compat: modal sandbox {id} could not start: {e}");
            // A Space made before the failure goes with the Sandbox.
            if let Some(rec) = self.sandbox(id)
                && let Some(name) = rec.cua_name.clone()
            {
                self.volsync_stop(id).await;
                match self.backend.delete(&rec.identity(), &name).await {
                    Ok(_) | Err(BackendError::NotFound(_)) => {}
                    Err(err) => tracing::warn!("compat: deleting {name}: {err}"),
                }
                self.backend_forget(id, &name).await;
            }
            self.update_sandbox(id, |s| {
                s.error = Some(e.clone());
                s.state = SbState::Finished;
                s.finished_at = now_s();
                s.result = Some(ResultRec {
                    status: status::FAILURE,
                    exitcode: 1,
                    exception: e,
                });
            });
        }
        self.notify_sb(id).await;
    }

    async fn provision_inner(
        self: &Arc<Self>,
        id: &str,
        from: Option<SnapshotRec>,
    ) -> Result<(), String> {
        let rec = self.sandbox(id).ok_or("sandbox vanished")?;
        let def = rec.def();
        let who = rec.identity();
        // The image.
        let built = if from.is_some() {
            // The machine comes back as it was; only its configuration is
            // needed.
            Arc::new(
                self.image(&rec.image_id)
                    .and_then(|i| i.built)
                    .unwrap_or_default(),
            )
        } else {
            self.ensure_image(rec.image_id.clone()).await?
        };
        let (place, _warnings) = Self::placement_of(&def);
        let source = match (&from, &built.checkpoint, &built.image) {
            (Some(s), _, _) => Source::Checkpoint(s.checkpoint.clone()),
            (None, Some(cp), _) => Source::Checkpoint(cp.clone()),
            (None, None, Some(img)) => Source::Image(Some(img.clone())),
            (None, None, None) if built.scratch => {
                return Err("an empty (FROM scratch) image cannot run a sandbox".into());
            }
            (None, None, None) => Source::Image(Some(self.base_image())),
        };
        // The environment: the image, secrets, the sandbox's own.
        let mut env: BTreeMap<String, String> = built.env.clone();
        for sid in &def.secret_ids {
            if let Some(s) = self.db.read(|d| d.secrets.get(sid).cloned()) {
                env.extend(s.env);
            }
        }
        if let Some(m) = &def.environment_variables {
            env.extend(m.contents.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        env.insert("MODAL_SANDBOX_ID".into(), id.to_string());
        env.insert("MODAL_TASK_ID".into(), rec.task_id.clone());
        env.insert("MODAL_IMAGE_ID".into(), rec.image_id.clone());
        env.insert("MODAL_ENVIRONMENT".into(), "main".into());
        let block = def
            .network_access
            .as_ref()
            .is_some_and(|n| n.network_access_type == 2);
        let name = format!("modal-{}", id.to_ascii_lowercase());
        let created = self
            .backend
            .create_with(
                &who,
                CreateSpec {
                    name,
                    source,
                    env: Default::default(),
                    internet: !block,
                    place,
                },
                Self::extras_of(&def.experimental_options_v2),
            )
            .await
            .map_err(|e| e.to_string())?;
        self.update_sandbox(id, |s| {
            s.cua_name = Some(created.name.clone());
            // Secrets given at create (`secret_sources`) win over the image's.
            let given = std::mem::take(&mut s.env);
            s.env = env;
            s.env.extend(given);
        });
        if built.reboot {
            // A filesystem snapshot: boot its disk afresh, so processes
            // start new and the memory of the snapshotted machine is not
            // carried over.
            self.backend
                .restart(&who, &created.name)
                .await
                .map_err(|e| format!("booting the snapshot: {e}"))?;
            self.live.clients.lock().await.remove(id);
            self.live.guest.lock().await.remove(id);
            self.live.drivers.forget(&created.name).await;
            self.live.drivers.forget(id).await;
        }
        let rec = self.sandbox(id).ok_or("sandbox vanished")?;
        // After a reboot cua-spacesd takes a moment to answer again.
        let mut client = self.spacesd(&rec).await;
        if built.reboot {
            for _ in 0..45 {
                if client.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                self.live.clients.lock().await.remove(id);
                client = self.spacesd(&rec).await;
            }
        }
        let client = client.map_err(|e| e.to_string())?;
        // Files of mounts, workdir, volumes.
        self.prepare_guest(&rec, &def, &client, &built).await?;
        // Tunnels.
        let tunnels = self.open_tunnels(&rec, &def).await?;
        self.update_sandbox(id, |s| {
            s.tunnels = tunnels;
            s.state = SbState::Running;
            s.started_at = now_s();
            s.restored = from.is_some();
        });
        self.touch(id).await;
        // The entrypoint.
        let argv = entrypoint_argv(&def, &built);
        let entry = if from.is_some() {
            // Restored from a memory snapshot: the entrypoint is already
            // running in the restored machine.
            self.attach_entrypoint(&rec, &client).await
        } else if argv.is_empty() {
            None
        } else {
            // A filesystem snapshot carries the memory of the machine it was
            // taken from, with that machine's entrypoint still running: end it,
            // the new Sandbox runs its own.
            if !built.reboot
                && let Some(old) = exec::attach(&client, ENTRY_TAG).await
            {
                old.signal(cua_spacesd_client::pb::Signal::Kill, true).await;
                for _ in 0..50 {
                    if exec::attach(&client, ENTRY_TAG).await.is_none() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            Some(
                self.launch(
                    &rec,
                    ExecReq {
                        argv,
                        env: BTreeMap::new(),
                        cwd: None,
                        user: None,
                        timeout: None,
                        pty: def
                            .pty_info
                            .as_ref()
                            .filter(|p| p.enabled)
                            .map(|p| (p.winsz_cols.max(80), p.winsz_rows.max(24))),
                        stdin: true,
                        tag: Some(ENTRY_TAG.into()),
                    },
                    exec::Config {
                        discard_stdout: false,
                        discard_stderr: false,
                        merge_stderr: false,
                    },
                )
                .await
                .map_err(|e| e.message().to_string())?,
            )
        };
        if let Some(e) = &entry {
            self.live
                .entrypoints
                .lock()
                .await
                .insert(id.to_string(), e.clone());
        }
        let me = self.clone();
        let sid = id.to_string();
        tokio::spawn(async move { me.supervise(sid, entry).await });
        Ok(())
    }

    /// Reattaches the entrypoint process of a restored machine.
    async fn attach_entrypoint(
        &self,
        rec: &SandboxRec,
        client: &SpacesdClient,
    ) -> Option<Arc<Session>> {
        let _ = rec;
        exec::attach(client, ENTRY_TAG).await
    }

    /// After a gateway restart: reattaches the entrypoint of a sandbox that
    /// is still running and supervises it again.
    pub async fn resume_supervision(self: &Arc<Self>, rec: &SandboxRec) {
        let Ok(client) = self.spacesd(rec).await else {
            return;
        };
        let entry = self.attach_entrypoint(rec, &client).await;
        if let Some(e) = &entry {
            self.live
                .entrypoints
                .lock()
                .await
                .insert(rec.id.clone(), e.clone());
        }
        self.touch(&rec.id).await;
        let me = self.clone();
        let id = rec.id.clone();
        tokio::spawn(async move { me.supervise(id, entry).await });
    }

    /// Starts a process in the sandbox as Modal would run it.
    pub async fn launch(
        &self,
        rec: &SandboxRec,
        req: ExecReq,
        cfg: exec::Config,
    ) -> Result<Arc<Session>, tonic::Status> {
        let client = self.spacesd(rec).await.map_err(errors::from_backend)?;
        let built = self
            .image(&rec.image_id)
            .and_then(|i| i.built)
            .unwrap_or_default();
        let def = rec.def();
        let (driver, _) = self.guest_of(rec, &client).await;
        let user = req
            .user
            .clone()
            .or_else(|| built.user.clone())
            .unwrap_or_else(|| self.cfg.default_user.clone());
        let cwd = req
            .cwd
            .clone()
            .or_else(|| def.workdir.clone().filter(|w| !w.is_empty()))
            .or_else(|| built.workdir.clone())
            .unwrap_or_else(|| "/".into());
        let mut env = rec.env.clone();
        env.extend(req.env.clone());
        let (program, args, u, cwd) = crate::guest::as_user_in(
            &driver,
            &user,
            req.argv[0].clone(),
            req.argv[1..].to_vec(),
            cwd,
        );
        let mut cmd = Command::new(program).args(args);
        for (k, v) in env {
            cmd = cmd.env(k, v);
        }
        if !u.is_empty() {
            cmd = cmd.user(u);
        }
        if !cwd.is_empty() {
            cmd = cmd.cwd(cwd);
        }
        if let Some(t) = req.timeout {
            // Modal enforces an exec's `timeout` at the client: its wait gives
            // up at the deadline and returns -1. The guest kills the process a
            // moment later, so the exit never races the client's deadline (a
            // kill at exactly the deadline made the client see the kill's
            // status instead of -1 on a loaded host).
            cmd = cmd.timeout(t + EXEC_KILL_GRACE);
        }
        if let Some((cols, rows)) = req.pty {
            cmd = cmd.pty(cols, rows);
        }
        cmd.stdin = req.stdin;
        cmd.tag = req.tag.clone();
        exec::start(&client, cmd, cfg)
            .await
            .map_err(|e| errors::conflict(format!("could not start the process: {e}")))
    }

    /// `(driver user, linux)` for the sandbox's guest (cached).
    pub async fn guest_of(&self, rec: &SandboxRec, client: &SpacesdClient) -> (String, bool) {
        if let Some(g) = self.live.guest.lock().await.get(&rec.id) {
            return g.clone();
        }
        let driver = self.live.drivers.of(&rec.id, client).await;
        let linux = match client.run(Command::new("uname").arg("-s")).await {
            Ok(o) if o.success() => String::from_utf8_lossy(&o.stdout).trim() == "Linux",
            _ => true,
        };
        self.live
            .guest
            .lock()
            .await
            .insert(rec.id.clone(), (driver.clone(), linux));
        (driver, linux)
    }

    /// Mount files, the working directory and volumes.
    async fn prepare_guest(
        &self,
        rec: &SandboxRec,
        def: &api::Sandbox,
        client: &SpacesdClient,
        built: &crate::modal::image::Built,
    ) -> Result<(), String> {
        let (driver, linux) = self.guest_of(rec, client).await;
        let _ = (linux, built);
        // Local files of `add_local_*` (not copied at build time).
        for mid in &def.mount_ids {
            if let Some(m) = self.db.read(|d| d.mounts.get(mid).cloned()) {
                for f in &m.files {
                    let data = self.blobs.get_sha(&f.sha256).ok_or_else(|| {
                        format!(
                            "mount file {} is missing from the gateway's store",
                            f.filename
                        )
                    })?;
                    let target = if f.filename.starts_with('/') {
                        f.filename.clone()
                    } else {
                        format!("/{}", f.filename)
                    };
                    self.put_file_as_root(client, &driver, &target, data, f.mode)
                        .await
                        .map_err(|e| format!("mounting {target}: {e}"))?;
                }
            }
        }
        if let Some(w) = def.workdir.as_ref().filter(|w| !w.is_empty()) {
            client
                .make_dir(w)
                .await
                .map_err(|e| format!("creating the working directory {w}: {e}"))?;
        }
        self.mount_volumes(rec, def, client, &driver).await
    }

    /// Writes a file as root (staged through `/tmp` when the driver is not).
    pub async fn put_file_as_root(
        &self,
        client: &SpacesdClient,
        driver: &str,
        path: &str,
        data: Vec<u8>,
        mode: Option<u32>,
    ) -> Result<(), String> {
        let mut w = crate::guestfs::Writer::begin(client, path, "root", driver)
            .await
            .map_err(|e| e.1)?;
        w.write(data.into()).await.map_err(|e| e.1)?;
        w.finish().await.map_err(|e| e.1)?;
        if let Some(m) = mode {
            let _ = crate::guest::run_as(
                client,
                driver,
                "root",
                "chmod \"$1\" \"$2\"",
                &[&format!("{:o}", m & 0o7777), path],
            )
            .await;
        }
        Ok(())
    }

    /// Watches a sandbox: finishes it on entrypoint exit, timeout, idle
    /// timeout, or when it is terminated elsewhere.
    async fn supervise(self: Arc<Self>, id: String, entry: Option<Arc<Session>>) {
        let Some(rec) = self.sandbox(&id) else { return };
        let def = rec.def();
        let started = tokio::time::Instant::now();
        let deadline = (def.timeout_secs > 0)
            .then(|| started + Duration::from_secs(u64::from(def.timeout_secs)));
        let idle = def
            .idle_timeout_secs
            .filter(|s| *s > 0)
            .map(|s| Duration::from_secs(u64::from(s)));
        let mut rx = self.sb_events(&id).await;
        loop {
            if self
                .sandbox(&id)
                .is_none_or(|s| s.state == SbState::Finished)
            {
                return;
            }
            let wait_entry = async {
                match &entry {
                    Some(s) => s.wait().await,
                    None => std::future::pending().await,
                }
            };
            let until_deadline = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                exit = wait_entry => {
                    let result = result_of_exit(&exit);
                    self.finish(&id, result).await;
                    return;
                }
                _ = until_deadline => {
                    self.finish(&id, ResultRec {
                        status: status::TIMEOUT,
                        exitcode: 0,
                        exception: format!("Sandbox timed out after {} seconds", def.timeout_secs),
                    }).await;
                    return;
                }
                _ = rx.changed() => {}
                _ = tokio::time::sleep(Duration::from_millis(500)), if idle.is_some() => {
                    let last = self.live.activity.lock().await.get(&id).copied().unwrap_or(started);
                    if let Some(limit) = idle
                        && last.elapsed() >= limit
                    {
                        self.finish(&id, ResultRec {
                            status: status::IDLE_TIMEOUT,
                            exitcode: 0,
                            exception: format!("Sandbox was idle for {} seconds", limit.as_secs()),
                        }).await;
                        return;
                    }
                }
            }
        }
    }

    /// Ends a sandbox with `result`: the Space is deleted, waiters wake.
    pub async fn finish(self: &Arc<Self>, id: &str, result: ResultRec) {
        let was = self.update_sandbox(id, |s| {
            if s.state == SbState::Finished {
                return None;
            }
            s.state = SbState::Finished;
            s.finished_at = now_s();
            s.result = Some(result.clone());
            Some((s.cua_name.clone(), s.identity()))
        });
        let Some(Some((name, who))) = was else { return };
        // Volume changes made in the Space are committed before it goes.
        self.volsync_stop(id).await;
        if name.is_some()
            && self.sandbox(id).is_some_and(|r| {
                r.def()
                    .experimental_options_v2
                    .get("enable_exit_snapshot")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"))
            })
            && let Some(rec) = self.sandbox(id)
        {
            self.take_exit_snapshot(&rec).await;
        }
        if let Some(name) = name {
            match self.backend.delete(&who, &name).await {
                Ok(_) | Err(BackendError::NotFound(_)) => {}
                Err(e) => tracing::warn!("compat: deleting {name}: {e}"),
            }
            self.backend_forget(id, &name).await;
        }
        // What was running when the sandbox ended was killed.
        let task = self.sandbox(id).map(|r| r.task_id).unwrap_or_default();
        for ((t, _), s) in self.live.execs.lock().await.iter() {
            if *t == task {
                s.finish(Exit {
                    signal: Some(9),
                    ..Default::default()
                });
            }
        }
        if let Some(e) = self.live.entrypoints.lock().await.get(id) {
            e.finish(Exit {
                signal: Some(9),
                ..Default::default()
            });
        }
        self.notify_sb(id).await;
        // The output of a finished sandbox is kept for an hour.
        let me = Arc::downgrade(self);
        let id = id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            if let Some(m) = me.upgrade() {
                m.live.execs.lock().await.retain(|(t, _), _| *t != task);
                m.live.entrypoints.lock().await.remove(&id);
                m.live.events.lock().await.remove(&id);
            }
        });
    }

    async fn backend_forget(&self, id: &str, name: &str) {
        self.live.clients.lock().await.remove(id);
        self.live.guest.lock().await.remove(id);
        self.live.drivers.forget(name).await;
        self.live.drivers.forget(id).await;
        self.live.activity.lock().await.remove(id);
        // The loopback listeners of the sandbox's raw tunnels go with it.
        self.live
            .raw_tunnels
            .lock()
            .await
            .retain(|(sb, _), (_, task)| {
                let keep = sb != id;
                if !keep {
                    task.abort();
                }
                keep
            });
    }

    /// `SandboxTerminate`: a no-op when finished.
    pub async fn terminate(self: &Arc<Self>, id: &str) {
        self.finish(
            id,
            ResultRec {
                status: status::TERMINATED,
                exitcode: 0,
                exception: "Sandbox terminated".into(),
            },
        )
        .await;
    }

    /// Waits up to `timeout` for the sandbox to finish.
    pub async fn wait_finished(&self, id: &str, timeout: Duration) -> Option<SandboxRec> {
        let mut rx = self.sb_events(id).await;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let rec = self.sandbox(id)?;
            if rec.state == SbState::Finished || timeout.is_zero() {
                return Some(rec);
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return self.sandbox(id);
            }
        }
    }

    /// Waits until the sandbox is scheduled (or finished).
    pub async fn wait_scheduled(&self, id: &str, timeout: Duration) -> Option<SandboxRec> {
        let mut rx = self.sb_events(id).await;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let rec = self.sandbox(id)?;
            if rec.state != SbState::Scheduling {
                return Some(rec);
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return self.sandbox(id);
            }
        }
    }

    /// Opens the sandbox's tunnels: TLS names under the host router for
    /// encrypted ports, a loopback listener for unencrypted ones.
    async fn open_tunnels(
        &self,
        rec: &SandboxRec,
        def: &api::Sandbox,
    ) -> Result<Vec<TunnelRec>, String> {
        let mut out = Vec::new();
        let Some(api::sandbox::OpenPortsOneof::OpenPorts(ports)) = &def.open_ports_oneof else {
            return Ok(out);
        };
        let (host, port) = match self.cfg.sandbox_domain.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => {
                (h.to_string(), p.parse().unwrap_or(443))
            }
            _ => (self.cfg.sandbox_domain.clone(), 443u32),
        };
        for p in &ports.ports {
            let mut t = TunnelRec {
                container_port: p.port,
                host: format!("{}-{}.{}", p.port, rec.id.to_ascii_lowercase(), host),
                port,
                unencrypted_host: None,
                unencrypted_port: None,
            };
            if p.unencrypted {
                let addr = self.raw_tunnel(rec, p.port as u16).await?;
                let (h, pt) = addr.rsplit_once(':').ok_or("bad tunnel address")?;
                t.unencrypted_host = Some(h.to_string());
                t.unencrypted_port = pt.parse().ok();
            }
            out.push(t);
        }
        Ok(out)
    }

    /// A loopback TCP listener that forwards to a guest port.
    async fn raw_tunnel(&self, rec: &SandboxRec, port: u16) -> Result<String, String> {
        let key = (rec.id.clone(), port);
        if let Some((a, _)) = self.live.raw_tunnels.lock().await.get(&key) {
            return Ok(a.clone());
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let addr = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let backend = self.backend.clone();
        let who = rec.identity();
        let name = rec
            .cua_name
            .clone()
            .ok_or_else(|| "sandbox is not scheduled".to_string())?;
        let live_id = rec.id.clone();
        let me_events = self.sb_events(&live_id).await;
        let _ = me_events;
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let backend = backend.clone();
                let who = who.clone();
                let name = name.clone();
                tokio::spawn(async move {
                    let Ok(fwd) = backend.forward(&who, &name, port).await else {
                        return;
                    };
                    let Ok(mut up) = tokio::net::TcpStream::connect(&fwd).await else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut sock, &mut up).await;
                });
            }
        });
        self.live
            .raw_tunnels
            .lock()
            .await
            .insert(key, (addr.clone(), accept));
        Ok(addr)
    }
}

/// How long after an exec's own `timeout` the guest kills it.
const EXEC_KILL_GRACE: Duration = Duration::from_secs(5);

/// The sandbox result for the entrypoint's exit.
pub fn result_of_exit(exit: &Exit) -> ResultRec {
    match exit.status() {
        (Some(0), _) => ResultRec {
            status: status::SUCCESS,
            exitcode: 0,
            exception: String::new(),
        },
        (Some(c), _) => ResultRec {
            status: status::FAILURE,
            exitcode: c,
            exception: exit.error.clone().unwrap_or_default(),
        },
        (None, Some(sig)) => ResultRec {
            status: status::FAILURE,
            exitcode: 128 + sig,
            exception: String::new(),
        },
        _ => ResultRec {
            status: status::FAILURE,
            exitcode: 1,
            exception: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sdk_default_runtime_leaves_the_kind_to_the_gateway() {
        let mut def = api::Sandbox {
            runtime: Some("gvisor".into()),
            ..Default::default()
        };
        assert_eq!(Modal::placement_of(&def).0.kind, None);
        def.runtime = Some("vm".into());
        assert_eq!(Modal::placement_of(&def).0.kind.as_deref(), Some("vm"));
        // An explicit cua.kind wins over the runtime.
        def.experimental_options_v2
            .insert("cua.kind".into(), "container".into());
        assert_eq!(
            Modal::placement_of(&def).0.kind.as_deref(),
            Some("container")
        );
    }

    #[test]
    fn exit_to_result() {
        assert_eq!(result_of_exit(&Exit::code(0)).status, status::SUCCESS);
        let r = result_of_exit(&Exit::code(3));
        assert_eq!((r.status, r.exitcode), (status::FAILURE, 3));
        let r = result_of_exit(&Exit {
            signal: Some(9),
            ..Default::default()
        });
        assert_eq!(r.exitcode, 137);
    }

    #[test]
    fn entrypoint_is_image_entrypoint_then_args_or_cmd() {
        let mut built = crate::modal::image::Built {
            cmd: Some(vec!["sleep".into(), "9".into()]),
            ..Default::default()
        };
        let mut def = api::Sandbox::default();
        assert_eq!(entrypoint_argv(&def, &built), ["sleep", "9"]);
        def.entrypoint_args = vec!["echo".into(), "hi".into()];
        assert_eq!(entrypoint_argv(&def, &built), ["echo", "hi"]);
        built.entrypoint = Some(vec!["/bin/sh".into(), "-c".into()]);
        assert_eq!(
            entrypoint_argv(&def, &built),
            ["/bin/sh", "-c", "echo", "hi"]
        );
    }
}

impl Modal {
    /// Validates a sandbox definition the way the server does, refusing
    /// with Modal's own error types what Cua Spaces cannot do.
    pub fn check_sandbox_definition(
        &self,
        caller: &Caller,
        def: &api::Sandbox,
    ) -> Result<(), tonic::Status> {
        let owned_image = self.db.read(|d| {
            d.images
                .get(&def.image_id)
                .is_some_and(|i| i.key_id == caller.key_id())
        });
        if !owned_image {
            return Err(errors::not_found(format!(
                "Image {} not found",
                def.image_id
            )));
        }
        for sid in &def.secret_ids {
            let ok = self.db.read(|d| {
                d.secrets
                    .get(sid)
                    .is_some_and(|s| s.key_id == caller.key_id())
            });
            if !ok {
                return Err(errors::not_found(format!("Secret {sid} not found")));
            }
        }
        for mid in &def.mount_ids {
            let ok = self.db.read(|d| {
                d.mounts
                    .get(mid)
                    .is_some_and(|s| s.key_id == caller.key_id())
            });
            if !ok {
                return Err(errors::not_found(format!("Mount {mid} not found")));
            }
        }
        if def.timeout_secs > 24 * 3600 {
            return Err(errors::invalid("timeout must be at most 24 hours"));
        }
        if let Some(api::sandbox::OpenPortsOneof::OpenPorts(ports)) = &def.open_ports_oneof {
            let mut seen = std::collections::BTreeSet::new();
            for p in &ports.ports {
                if !(1..=65535).contains(&p.port) {
                    return Err(errors::invalid(format!(
                        "invalid port {}: ports are 1 to 65535",
                        p.port
                    )));
                }
                if !seen.insert((p.port, p.unencrypted)) {
                    return Err(errors::invalid(format!("port {} is listed twice", p.port)));
                }
            }
        }
        if let Some(n) = &def.network_access {
            use api::network_access::NetworkAccessType as T;
            if n.network_access_type == T::Allowlist as i32 {
                return Err(errors::unsupported(
                    "outbound_cidr_allowlist / outbound_domain_allowlist",
                    "Cua Spaces can block egress entirely (block_network=True) or leave it open; a host-enforced allowlist (which code running as root in the sandbox cannot undo) needs egress filtering the Cua VM layer does not have",
                ));
            }
        }
        if !def.inbound_cidr_allowlist.is_empty() {
            return Err(errors::unsupported(
                "inbound_cidr_allowlist",
                "tunnels are served on loopback or your own network; Cua does not filter inbound connections by source CIDR",
            ));
        }
        if def.experimental_outbound_policy.is_some() || def.outbound_policy.is_some() {
            return Err(errors::unsupported(
                "outbound policies",
                "header replacement and routing for outbound traffic need a host-side proxy Cua Spaces do not have",
            ));
        }
        if def.proxy_id.is_some() {
            return Err(errors::unsupported(
                "proxy",
                "Modal Proxies are a Modal networking product",
            ));
        }
        if !def.custom_domain.is_empty() {
            return Err(errors::unsupported(
                "custom_domain",
                "tunnels are served under the gateway's own domain",
            ));
        }
        if def.include_oidc_identity_token || def.include_x509_svid {
            return Err(errors::unsupported(
                "identity tokens",
                "OIDC and SPIFFE tokens are issued by Modal's identity provider",
            ));
        }
        if def.i6pn_enabled {
            return Err(errors::unsupported(
                "i6pn",
                "private IPv6 networking between sandboxes is a Modal network feature",
            ));
        }
        if !def.nfs_mounts.is_empty() {
            return Err(errors::unsupported(
                "network_file_systems",
                "NetworkFileSystem is deprecated by Modal; use a Volume",
            ));
        }
        if let Some(r) = &def.runtime
            && !matches!(r.as_str(), "gvisor" | "vm" | "")
        {
            return Err(errors::invalid(format!(
                "runtime must be one of ['gvisor', 'vm'], got {r:?}"
            )));
        }
        for (k, v) in &def.experimental_options {
            if k.starts_with("cua.") || k == "enable_exit_snapshot" {
                continue;
            }
            if *v {
                return Err(errors::unsupported(
                    &format!("experimental option {k:?}"),
                    "it enables a Modal runtime feature Cua Spaces do not have",
                ));
            }
        }
        for k in def.experimental_options_v2.keys() {
            if k.starts_with("cua.") || k == "enable_exit_snapshot" {
                continue;
            }
            return Err(errors::unsupported(
                &format!("experimental option {k:?}"),
                "it configures a Modal runtime feature Cua Spaces do not have (Cua options are experimental_options={\"cua.on\": ..., \"cua.kind\": ..., \"cua.cpus\": ..., \"cua.memory_mb\": ..., \"cua.gpu\": ..., \"cua.disk_gb\": ...})",
            ));
        }
        if let Some(g) = def.resources.as_ref().and_then(|r| r.gpu_config.as_ref())
            && (g.count > 0 || !g.gpu_type.is_empty() || g.r#type != 0)
            && !def.experimental_options_v2.contains_key("cua.gpu")
        {
            return Err(errors::unsupported("gpu", GPU_HINT));
        }
        Ok(())
    }

    /// The logs of the entrypoint as `TaskLogsBatch`es from `last_entry_id`
    /// (a byte offset), for up to `hold`.
    pub async fn sandbox_log_stream(
        self: &Arc<Self>,
        rec: SandboxRec,
        fd: usize,
        last_entry_id: String,
        hold: Duration,
    ) -> tonic::codegen::BoxStream<api::TaskLogsBatch> {
        let me = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + hold;
            let mut off: u64 = last_entry_id.parse().unwrap_or(0);
            let fdn = if fd == exec::STDERR {
                api::FileDescriptor::Stderr as i32
            } else {
                api::FileDescriptor::Stdout as i32
            };
            let mut carry: Vec<u8> = Vec::new();
            loop {
                let session = me.live.entrypoints.lock().await.get(&rec.id).cloned();
                let Some(s) = session else {
                    // No entrypoint (none was given, or it was forgotten):
                    // the logs end when the sandbox does.
                    let r = me
                        .wait_finished(
                            &rec.id,
                            deadline.saturating_duration_since(tokio::time::Instant::now()),
                        )
                        .await;
                    if r.is_some_and(|r| r.state == SbState::Finished) {
                        let _ = tx.send(Ok(eof_batch(&rec, off))).await;
                    }
                    return;
                };
                let next = tokio::time::timeout_at(deadline, s.read(fd, off)).await;
                match next {
                    Err(_) => return, // hold elapsed: the client asks again
                    Ok(Ok(Some(chunk))) => {
                        off += chunk.len() as u64;
                        carry.extend_from_slice(&chunk);
                        let text = take_utf8(&mut carry);
                        if text.is_empty() {
                            continue;
                        }
                        let batch = api::TaskLogsBatch {
                            task_id: rec.task_id.clone(),
                            items: vec![api::TaskLogs {
                                data: text,
                                file_descriptor: fdn,
                                timestamp: now_s(),
                                ..Default::default()
                            }],
                            entry_id: off.to_string(),
                            sandbox_id: rec.id.clone(),
                            ..Default::default()
                        };
                        if tx.send(Ok(batch)).await.is_err() {
                            return;
                        }
                    }
                    Ok(Ok(None)) => {
                        if !carry.is_empty() {
                            let text = String::from_utf8_lossy(&carry).into_owned();
                            carry.clear();
                            let _ = tx
                                .send(Ok(api::TaskLogsBatch {
                                    task_id: rec.task_id.clone(),
                                    items: vec![api::TaskLogs {
                                        data: text,
                                        file_descriptor: fdn,
                                        timestamp: now_s(),
                                        ..Default::default()
                                    }],
                                    entry_id: off.to_string(),
                                    sandbox_id: rec.id.clone(),
                                    ..Default::default()
                                }))
                                .await;
                        }
                        let _ = tx.send(Ok(eof_batch(&rec, off))).await;
                        return;
                    }
                    Ok(Err(exec::ReadErr::Evicted { start })) => {
                        off = start;
                    }
                }
            }
        });
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

fn eof_batch(rec: &SandboxRec, off: u64) -> api::TaskLogsBatch {
    api::TaskLogsBatch {
        task_id: rec.task_id.clone(),
        entry_id: off.to_string(),
        eof: true,
        sandbox_id: rec.id.clone(),
        ..Default::default()
    }
}

/// Takes the longest valid UTF-8 prefix of `buf` (invalid bytes become
/// U+FFFD; an incomplete sequence at the end stays for the next chunk).
pub fn take_utf8(buf: &mut Vec<u8>) -> String {
    let mut out = String::new();
    loop {
        match std::str::from_utf8(buf) {
            Ok(s) => {
                out.push_str(s);
                buf.clear();
                return out;
            }
            Err(e) => {
                let good = e.valid_up_to();
                out.push_str(std::str::from_utf8(&buf[..good]).unwrap_or_default());
                match e.error_len() {
                    Some(n) => {
                        out.push('\u{FFFD}');
                        buf.drain(..good + n);
                    }
                    None => {
                        buf.drain(..good);
                        return out;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod utf8_tests {
    use super::take_utf8;

    #[test]
    fn splits_never_inside_a_character() {
        let mut b = "h\u{e9}llo".as_bytes()[..2].to_vec(); // 'h' + first byte of é
        assert_eq!(take_utf8(&mut b), "h");
        b.extend_from_slice(&"é".as_bytes()[1..]);
        assert_eq!(take_utf8(&mut b), "é");
        let mut bad = vec![b'a', 0xff, b'b'];
        assert_eq!(take_utf8(&mut bad), "a\u{FFFD}b");
        assert!(bad.is_empty());
    }
}

/// Terminates a sandbox if dropped before [`CancelGuard::disarm`]: a V2
/// create the client abandoned.
pub struct CancelGuard {
    m: Arc<Modal>,
    id: String,
    armed: std::sync::atomic::AtomicBool,
}

impl CancelGuard {
    pub fn new(m: Arc<Modal>, id: String) -> CancelGuard {
        CancelGuard {
            m,
            id,
            armed: std::sync::atomic::AtomicBool::new(true),
        }
    }

    pub fn disarm(&self) {
        self.armed.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            let (m, id) = (self.m.clone(), self.id.clone());
            tokio::spawn(async move {
                // Wait for the Space (if it is still being made) to exist,
                // then end the sandbox.
                let _ = m.wait_scheduled(&id, Duration::from_secs(30 * 60)).await;
                m.terminate(&id).await;
            });
        }
    }
}
