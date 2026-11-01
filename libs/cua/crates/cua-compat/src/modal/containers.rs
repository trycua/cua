//! Function containers: the autoscaler and the runtimes that run Modal's own
//! container runtime (`python -m modal._container_entrypoint`) for a function.
//!
//! - The autoscaler starts a container when inputs wait and none can take
//!   them (up to the function's `max_containers`, 8 by default), keeps
//!   `min_containers` warm, and stops containers idle for `scaledown_window`
//!   seconds (60 by default).
//! - A container is a Cua Space made from the function's image (plus the
//!   `modal` client, installed once per image). It reaches the gateway
//!   through a stdio bridge (`bridge.rs`, `_cua_bridge.py`): the Space has
//!   no route to the host's network, only to the gateway's API port; its
//!   output goes to the App's logs.
//! - A container that exits with inputs in flight fails them with
//!   `InternalFailure` (retried by the function's retry policy), as Modal's
//!   server does for a lost worker.

use crate::backend::{CreateSpec, Placement, Source};
use crate::modal::db::FunctionRec;
use crate::modal::functions::{ContainerInfo, InputState};
use crate::modal::image::Built;
use crate::modal::pb::api as pb;
use crate::modal::{Modal, ids};
use async_trait::async_trait;
use prost::Message;
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::time::Instant;

/// Everything a runtime needs to start one container.
pub struct ContainerSpec {
    pub task_id: String,
    pub function_id: String,
    pub app_id: String,
    pub key_id: String,
    /// Files to place: `(absolute path, content, mode)`.
    pub files: Vec<(String, Vec<u8>, Option<u32>)>,
    pub env: BTreeMap<String, String>,
    /// The serialized `ContainerArguments`.
    pub arguments: Vec<u8>,
    pub image: Arc<Built>,
    pub function_image_id: String,
    /// Volume and bucket mounts the function declares.
    pub volume_mounts: Vec<pb::VolumeMount>,
    pub cloud_bucket_mounts: Vec<pb::CloudBucketMount>,
    pub placement: Placement,
    /// The runtime's GPU option and disk size (`cua.gpu`, `cua.disk_gb`).
    pub extras: crate::backend::CreateExtras,
    /// The address of the gateway as the container must dial it.
    pub server_port: u16,
    /// The gateway's address on the host (a runtime that shares the host
    /// network dials this).
    pub server_url: String,
    /// The gateway's API address on the host (what the bridge connects to).
    pub api_addr: String,
    pub function_name: String,
}

/// A running container.
#[async_trait]
pub trait ContainerHandle: Send + Sync {
    /// Waits for the runtime to exit; the exit code (`None`: lost).
    async fn wait(&self) -> Option<i32>;
    /// Stops it and removes whatever it made.
    async fn kill(&self);
    /// The Space the container runs in, to snapshot it (a build function).
    fn space(&self) -> Option<(crate::backend::Identity, String)> {
        None
    }
}

/// Output lines of a container.
pub type LogSink = Arc<dyn Fn(usize, &str) + Send + Sync>;

#[async_trait]
pub trait ContainerRuntime: Send + Sync + 'static {
    async fn start(
        &self,
        m: &Arc<Modal>,
        spec: ContainerSpec,
        logs: LogSink,
    ) -> Result<Box<dyn ContainerHandle>, String>;
}

/// Containers as Cua Spaces.
pub struct SpaceRuntime;

struct SpaceContainer {
    m: Arc<Modal>,
    task: String,
    who: crate::backend::Identity,
    name: String,
    done: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Option<i32>>>>,
    exit: tokio::sync::Mutex<Option<Option<i32>>>,
}

#[async_trait]
impl ContainerHandle for SpaceContainer {
    async fn wait(&self) -> Option<i32> {
        if let Some(e) = *self.exit.lock().await {
            return e;
        }
        let rx = self.done.lock().await.take();
        let code = match rx {
            Some(rx) => rx.await.unwrap_or(None),
            None => None,
        };
        *self.exit.lock().await = Some(code);
        code
    }

    async fn kill(&self) {
        self.m.volsync_stop(&self.task).await;
        let _ = self.m.backend.delete(&self.who, &self.name).await;
        self.m.live.drivers.forget(&self.name).await;
    }

    fn space(&self) -> Option<(crate::backend::Identity, String)> {
        Some((self.who.clone(), self.name.clone()))
    }
}

#[async_trait]
impl ContainerRuntime for SpaceRuntime {
    async fn start(
        &self,
        m: &Arc<Modal>,
        spec: ContainerSpec,
        logs: LogSink,
    ) -> Result<Box<dyn ContainerHandle>, String> {
        let who = m.identity_of_key(&spec.key_id).await;
        let source = match (&spec.image.checkpoint, &spec.image.image) {
            (Some(cp), _) => Source::Checkpoint(cp.clone()),
            (None, Some(img)) => Source::Image(Some(img.clone())),
            _ => Source::Image(Some(m.base_image())),
        };
        let name = format!("modal-fn-{}", spec.task_id.to_ascii_lowercase());
        let created = m
            .backend
            .create_with(
                &who,
                CreateSpec {
                    name,
                    source,
                    env: Default::default(),
                    internet: true,
                    place: spec.placement.clone(),
                },
                spec.extras.clone(),
            )
            .await
            .map_err(|e| e.to_string())?;
        let cleanup = |m: Arc<Modal>, who: crate::backend::Identity, name: String| async move {
            let _ = m.backend.delete(&who, &name).await;
        };
        let client = match m.backend.spacesd(&who, &created.name).await {
            Ok(c) => c,
            Err(e) => {
                cleanup(m.clone(), who, created.name).await;
                return Err(e.to_string());
            }
        };
        let r = async {
            let driver = m.live.drivers.of(&created.name, &client).await;
            for (path, data, mode) in &spec.files {
                m.put_file_as_root(&client, &driver, path, data.clone(), *mode)
                    .await
                    .map_err(|e| format!("placing {path}: {e}"))?;
            }
            m.put_file_as_root(
                &client,
                &driver,
                "/__modal/container-arguments.bin",
                spec.arguments.clone(),
                None,
            )
            .await
            .map_err(|e| format!("placing the container arguments: {e}"))?;
            m.put_file_as_root(
                &client,
                &driver,
                "/__modal/_cua_bridge.py",
                crate::modal::bridge::SCRIPT.as_bytes().to_vec(),
                Some(0o755),
            )
            .await?;
            m.mount_volume_list(
                &spec.task_id,
                &spec.key_id,
                &who,
                &spec.volume_mounts,
                &spec.cloud_bucket_mounts,
                &Default::default(),
                &client,
                &driver,
            )
            .await?;
            Ok::<_, String>(driver)
        }
        .await;
        let driver = match r {
            Ok(x) => x,
            Err(e) => {
                cleanup(m.clone(), who, created.name).await;
                return Err(e);
            }
        };
        let mut env = spec.env.clone();
        env.insert(
            "MODAL_CONTAINER_ARGUMENTS_PATH".into(),
            "/__modal/container-arguments.bin".into(),
        );
        let python = m.python_of(&spec.image);
        let argv: Vec<String> = vec![
            "python3".into(),
            "/__modal/_cua_bridge.py".into(),
            "--port".into(),
            spec.server_port.to_string(),
            "--".into(),
            python,
            "-m".into(),
            "modal._container_entrypoint".into(),
        ];
        let (program, args, user, cwd) = crate::guest::as_user_in(
            &driver,
            "root",
            argv[0].clone(),
            argv[1..].to_vec(),
            "/root".into(),
        );
        let mut cmd = cua_spacesd_client::Command::new(program)
            .args(args)
            .stdin(true);
        for (k, v) in &env {
            cmd = cmd.env(k, v);
        }
        if !user.is_empty() {
            cmd = cmd.user(user);
        }
        if !cwd.is_empty() {
            cmd = cmd.cwd(cwd);
        }
        let handle = client
            .spawn(cmd)
            .await
            .map_err(|e| format!("starting the container runtime: {e}"))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        // The gateway as the container reaches it: through the bridge, to this API address.
        let api = spec.api_addr.clone();
        let bridged = client.clone();
        tokio::spawn(async move {
            let code = crate::modal::bridge::run(bridged, handle, api, logs).await;
            let _ = tx.send(code);
        });
        Ok(Box::new(SpaceContainer {
            m: m.clone(),
            task: spec.task_id.clone(),
            who,
            name: created.name,
            done: tokio::sync::Mutex::new(Some(rx)),
            exit: tokio::sync::Mutex::new(None),
        }))
    }
}

impl Modal {
    /// The interpreter that runs the container runtime in an image.
    pub fn python_of(&self, _image: &Built) -> String {
        "python".into()
    }

    /// The image a function's containers start from: the function's image
    /// plus the `modal` client (what the container runtime needs), built
    /// once and cached with the layers it sits on.
    pub async fn function_image(self: &Arc<Self>, image_id: &str) -> Result<Arc<Built>, String> {
        let base = self.ensure_image(image_id.to_string()).await?;
        if base.scratch {
            return Err("an empty (FROM scratch) image cannot run a Function".into());
        }
        let key = format!("fnrt-{}-{}", crate::modal::CLIENT_VERSION, image_id);
        if let Some(b) = self.live.fn_images.lock().await.get(&key) {
            return Ok(b.clone());
        }
        let lock = self
            .live
            .fn_image_locks
            .lock()
            .await
            .entry(key.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        if let Some(b) = self.live.fn_images.lock().await.get(&key) {
            return Ok(b.clone());
        }
        let rec = self.image(image_id).ok_or("image vanished")?;
        let built = self
            .add_runtime_layer(&rec.key_id, &rec.hash, &base)
            .await?;
        let built = Arc::new(built);
        self.live.fn_images.lock().await.insert(key, built.clone());
        Ok(built)
    }

    async fn add_runtime_layer(
        self: &Arc<Self>,
        key_id: &str,
        image_hash: &str,
        base: &Built,
    ) -> Result<Built, String> {
        use sha2::Digest;
        if self.cfg.dry_run_builds || self.cfg.skip_runtime_layer {
            return Ok(base.clone());
        }
        let hash = hex::encode(sha2::Sha256::digest(
            format!("fnrt:{}:{image_hash}", crate::modal::CLIENT_VERSION).as_bytes(),
        ));
        let cp = format!("modal-fnrt-{}", &hash[..20]);
        let who = self.identity_of_key(key_id).await;
        if let Ok(list) = self.backend.checkpoints(&who).await
            && list.iter().any(|c| c.name == cp)
        {
            return Ok(Built {
                checkpoint: Some(cp),
                image: None,
                ..base.clone()
            });
        }
        let source = match (&base.checkpoint, &base.image) {
            (Some(c), _) => Source::Checkpoint(c.clone()),
            (None, Some(i)) => Source::Image(Some(i.clone())),
            _ => Source::Image(Some(self.base_image())),
        };
        let log = crate::modal::image::BuildLog::new();
        let sp = self
            .open_build_space(key_id, &format!("fnrt-{}", &hash[..16]), source)
            .await?;
        let script = runtime_install_script(crate::modal::CLIENT_VERSION);
        let r = sp
            .run(
                &["/bin/sh".into(), "-c".into(), script],
                &BTreeMap::new(),
                "/",
                "root",
                &log,
            )
            .await;
        if let Err(e) = r {
            sp.discard().await;
            let tail: Vec<String> = log.lines_from(log.lines_from(0).len().saturating_sub(12));
            return Err(format!(
                "installing the modal client in the Function image failed (exit {e}):\n{}",
                tail.join("\n")
            ));
        }
        sp.checkpoint_as(&cp).await?;
        sp.discard().await;
        Ok(Built {
            checkpoint: Some(cp),
            image: None,
            ..base.clone()
        })
    }
}

/// Makes `python` exist and installs the `modal` client into it.
pub fn runtime_install_script(version: &str) -> String {
    format!(
        r#"set -e
if ! command -v python >/dev/null 2>&1; then
  if command -v python3 >/dev/null 2>&1; then ln -sf "$(command -v python3)" /usr/local/bin/python
  else echo "this image has no Python: use Image.debian_slim(), a python:* base or add_python=" >&2; exit 1; fi
fi
if ! python -m pip --version >/dev/null 2>&1; then
  python -m ensurepip --upgrade >/dev/null 2>&1 || (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-pip >/dev/null)
fi
python -m pip install --quiet --disable-pip-version-check --break-system-packages "modal=={version}" 2>&1 || python -m pip install --quiet --disable-pip-version-check "modal=={version}"
python -c "import modal; print('modal', modal.__version__)"
"#
    )
}

// ---- the autoscaler ---------------------------------------------------------

fn scaledown() -> Duration {
    Duration::from_secs(
        std::env::var("CUA_MODAL_SCALEDOWN_S")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
    )
}

fn max_containers_default() -> u32 {
    std::env::var("CUA_MODAL_MAX_CONTAINERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
}

/// Runs for the life of the gateway.
pub async fn autoscaler(weak: Weak<Modal>) {
    loop {
        let Some(m) = weak.upgrade() else { return };
        let mut rx = m.fn_events.subscribe();
        m.scale().await;
        drop(m);
        // Woken by any change, and at least twice a second (timeouts).
        let _ = tokio::time::timeout(Duration::from_millis(500), rx.changed()).await;
    }
}

impl Modal {
    async fn scale(self: &Arc<Self>) {
        let mut to_start: Vec<(String, u32)> = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        let now = Instant::now();
        {
            let mut g = self.fns.lock().expect("fns");
            let ids: Vec<String> = g.queues.keys().cloned().collect();
            for fid in ids {
                let Some(f) = self.function(&fid) else {
                    continue;
                };
                let def = self.function_def(&f);
                let max = f
                    .max_containers
                    .or(Some(def.concurrency_limit).filter(|v| *v > 0))
                    .unwrap_or_else(max_containers_default);
                let min = f.min_containers.unwrap_or(def.warm_pool_size);
                let per = def
                    .max_concurrent_inputs
                    .max(def.target_concurrent_inputs)
                    .max(1) as usize;
                let idle_after = f
                    .scaledown_window
                    .or(Some(def.task_idle_timeout_secs).filter(|v| *v > 0))
                    .map(|s| Duration::from_secs(u64::from(s)))
                    .unwrap_or_else(scaledown);
                let q = g.queues.get_mut(&fid).expect("queue");
                let running: usize = q.containers.values().map(|c| c.running_inputs).sum();
                let demand = q.pending.len() + running;
                let want = demand.div_ceil(per).max(min as usize).min(max as usize);
                let have = q.containers.len() + q.starting as usize;
                if want > have && !(q.failures >= 3 && !q.pending.is_empty()) {
                    to_start.push((fid.clone(), (want - have).min(4) as u32));
                    q.starting += (want - have).min(4) as u32;
                }
                // A function whose containers cannot start fails what waits.
                if q.failures >= 3
                    && !q.pending.is_empty()
                    && q.containers.is_empty()
                    && q.starting == 0
                {
                    failed.push((
                        fid.clone(),
                        q.last_error
                            .clone()
                            .unwrap_or_else(|| "the container did not start".into()),
                    ));
                    q.failures = 0;
                }
                // Idle containers go.
                let keep = min as usize;
                let mut alive = q.containers.len();
                for c in q.containers.values_mut() {
                    if alive > keep.max(1) - usize::from(keep == 0)
                        && !c.stop
                        && c.running_inputs == 0
                        && q.pending.is_empty()
                        && now.duration_since(c.last_busy) > idle_after
                        && c.ready
                    {
                        c.stop = true;
                        alive -= 1;
                    }
                }
            }
            // Inputs past their timeout: the server ends them (Modal's
            // containers do not enforce `timeout`), and the container that
            // runs one is told to terminate, as Modal does.
            let mut overdue: Vec<(String, String, u32)> = Vec::new();
            {
                let fns = &mut *g;
                for call in fns.calls.values() {
                    let Some(f) = self.function(&call.function_id) else {
                        continue;
                    };
                    let t = self.function_def(&f).timeout_secs;
                    if t == 0 {
                        continue;
                    }
                    for s in &call.inputs {
                        if s.state == InputState::Running
                            && s.started.is_some_and(|st| {
                                now.duration_since(st) > Duration::from_secs(u64::from(t))
                            })
                        {
                            overdue.push((call.id.clone(), s.input_id.clone(), t));
                            if let Some(c) = s.task.as_ref().and_then(|t| {
                                fns.queues
                                    .get_mut(&call.function_id)
                                    .and_then(|q| q.containers.get_mut(t))
                            }) {
                                c.cancel.push(s.input_id.clone());
                                c.terminate_on_cancel = true;
                            }
                        }
                    }
                }
            }
            drop(g);
            for (call, input, t) in overdue {
                self.finish_input(
                    &call,
                    &input,
                    Self::failure_item(4, &format!("Function timed out after {t} seconds")),
                    true,
                );
            }
        }
        for (fid, msg) in failed {
            self.fail_pending(&fid, &msg);
        }
        for (fid, n) in to_start {
            for _ in 0..n {
                let me = self.clone();
                let fid = fid.clone();
                tokio::spawn(async move { me.run_container(fid).await });
            }
        }
    }

    /// Fails every waiting input of a function whose containers will not start.
    fn fail_pending(self: &Arc<Self>, fid: &str, msg: &str) {
        let waiting: Vec<(String, String)> = {
            let mut g = self.fns.lock().expect("fns");
            g.queues
                .get_mut(fid)
                .map(|q| q.pending.drain(..).collect())
                .unwrap_or_default()
        };
        for (call, input) in waiting {
            // INIT_FAILURE: the container never got as far as running the input.
            self.finish_input(&call, &input, Self::failure_item(5, msg), false);
        }
    }

    /// Starts one container for a function and supervises it.
    async fn run_container(self: Arc<Self>, fid: String) {
        let task_id = ids::new_id("ta");
        let r = self.start_container(&fid, &task_id).await;
        let handle = match r {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("compat: modal container for {fid} did not start: {e}");
                {
                    let mut g = self.fns.lock().expect("fns");
                    let q = g.queues.entry(fid.clone()).or_default();
                    q.starting = q.starting.saturating_sub(1);
                    q.failures += 1;
                    q.last_error = Some(e);
                }
                self.fn_changed();
                return;
            }
        };
        {
            let mut g = self.fns.lock().expect("fns");
            let q = g.queues.entry(fid.clone()).or_default();
            q.starting = q.starting.saturating_sub(1);
            q.containers.insert(
                task_id.clone(),
                ContainerInfo {
                    task_id: task_id.clone(),
                    started_at: Instant::now(),
                    ready: false,
                    running_inputs: 0,
                    last_busy: Instant::now(),
                    stop: false,
                    cancel: Vec::new(),
                    terminate_on_cancel: false,
                    space: handle.space(),
                },
            );
        }
        self.fn_changed();
        let code = handle.wait().await;
        handle.kill().await;
        self.container_exited(&fid, &task_id, code);
    }

    fn container_exited(self: &Arc<Self>, fid: &str, task_id: &str, code: Option<i32>) {
        let mut lost: Vec<(String, String)> = Vec::new();
        let mut never_ready = false;
        {
            let mut g = self.fns.lock().expect("fns");
            let fns = &mut *g;
            if let Some(q) = fns.queues.get_mut(fid)
                && let Some(c) = q.containers.remove(task_id)
            {
                never_ready = !c.ready;
                if never_ready && code != Some(0) {
                    q.failures += 1;
                    q.last_error = Some(format!(
                        "the container exited (code {}) before it took an input; see the App's logs",
                        code.map(|c| c.to_string()).unwrap_or_else(|| "lost".into())
                    ));
                } else {
                    q.failures = 0;
                }
            }
            for call in fns.calls.values() {
                for s in &call.inputs {
                    if s.state == InputState::Running && s.task.as_deref() == Some(task_id) {
                        lost.push((call.id.clone(), s.input_id.clone()));
                    }
                }
            }
        }
        let _ = never_ready;
        for (call, input) in lost {
            self.finish_input(
                &call,
                &input,
                Self::failure_item(
                    6,
                    &format!(
                        "The container {task_id} exited unexpectedly (code {})",
                        code.map(|c| c.to_string()).unwrap_or_else(|| "lost".into())
                    ),
                ),
                true,
            );
        }
        self.fn_changed();
    }

    /// Builds the container's arguments and starts it.
    async fn start_container(
        self: &Arc<Self>,
        fid: &str,
        task_id: &str,
    ) -> Result<Box<dyn ContainerHandle>, String> {
        let f = self.function(fid).ok_or("function vanished")?;
        let def = self.function_def(&f);
        let image = self.function_image(&def.image_id).await?;
        // Files of the function's mounts.
        let mut files = Vec::new();
        for mid in &def.mount_ids {
            let Some(m) = self.db.read(|d| d.mounts.get(mid).cloned()) else {
                continue;
            };
            for file in &m.files {
                let data = self.blobs.get_sha(&file.sha256).ok_or_else(|| {
                    format!(
                        "mount file {} is missing from the gateway's store",
                        file.filename
                    )
                })?;
                let path = if file.filename.starts_with('/') {
                    file.filename.clone()
                } else {
                    format!("/{}", file.filename)
                };
                files.push((path, data, file.mode));
            }
        }
        let mut env: BTreeMap<String, String> = image.env.clone();
        for sid in &def.secret_ids {
            if let Some(s) = self.db.read(|d| d.secrets.get(sid).cloned()) {
                env.extend(s.env);
            }
        }
        let layout = self.app_layout_for(&f);
        let args = pb::ContainerArguments {
            task_id: task_id.to_string(),
            function_id: f.id.clone(),
            app_id: f.app_id.clone(),
            function_def: Some(def.clone()),
            serialized_params: f
                .bound_params
                .as_ref()
                .and_then(|p| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(p).ok()
                })
                .unwrap_or_default()
                .into(),
            environment_name: "main".into(),
            app_layout: Some(layout),
            ..Default::default()
        };
        let token = self.signer.mint_with(
            "task",
            task_id,
            7 * 24 * 3600,
            serde_json::json!({"fn": f.id, "key": f.key_id}),
        );
        env.insert("MODAL_IS_REMOTE".into(), "1".into());
        env.insert("MODAL_TASK_ID".into(), task_id.to_string());
        env.insert("MODAL_IMAGE_ID".into(), def.image_id.clone());
        env.insert("MODAL_ENVIRONMENT".into(), "main".into());
        env.insert(
            "MODAL_OVERRIDE_HEADERS".into(),
            format!("x-cua-task:{token}"),
        );
        env.insert("PYTHONUNBUFFERED".into(), "1".into());
        let (server_url, server_port) = self.cfg.container_server();
        env.insert("MODAL_SERVER_URL".into(), server_url.clone());
        let spec = ContainerSpec {
            task_id: task_id.to_string(),
            function_id: f.id.clone(),
            app_id: f.app_id.clone(),
            key_id: f.key_id.clone(),
            files,
            env,
            arguments: args.encode_to_vec(),
            image,
            function_image_id: def.image_id.clone(),
            volume_mounts: def.volume_mounts.clone(),
            cloud_bucket_mounts: def.cloud_bucket_mounts.clone(),
            placement: Placement {
                cpus: def
                    .resources
                    .as_ref()
                    .map(|r| r.milli_cpu_max.max(r.milli_cpu))
                    .filter(|m| *m > 0)
                    .map(|m| m.div_ceil(1000).max(1)),
                memory_mb: def
                    .resources
                    .as_ref()
                    .map(|r| r.memory_mb_max.max(r.memory_mb))
                    .filter(|m| *m > 0)
                    .map(u64::from),
                on: def.experimental_options.get("cua.on").cloned(),
                kind: def.experimental_options.get("cua.kind").cloned(),
                ..Default::default()
            },
            extras: Modal::extras_of(&def.experimental_options),
            server_port,
            server_url,
            api_addr: self
                .cfg
                .api_url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string(),
            function_name: f.name.clone(),
        };
        let me = self.clone();
        let (app_id, function_id, tid) = (f.app_id.clone(), f.id.clone(), task_id.to_string());
        let logs: LogSink = Arc::new(move |fd, line| {
            me.push_app_log(&app_id, &function_id, &tid, fd, line);
        });
        let runtime = self.containers.clone();
        runtime.start(self, spec, logs).await
    }

    /// The layout a container needs to hydrate the objects its code uses.
    fn app_layout_for(&self, f: &FunctionRec) -> pb::AppLayout {
        let def = self.function_def(f);
        let mut objects = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut push = |id: &str, objects: &mut Vec<pb::Object>| {
            if !seen.insert(id.to_string()) {
                return;
            }
            objects.push(self.object_for(id));
        };
        // What the client published for the App (`AppPublish`), else what
        // was created.
        let (function_ids, class_ids): (BTreeMap<String, String>, BTreeMap<String, String>) =
            self.db.read(|d| {
                let published = d.apps.get(&f.app_id).map(|a| {
                    (
                        a.function_ids
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<BTreeMap<_, _>>(),
                        a.class_ids
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<BTreeMap<_, _>>(),
                    )
                });
                if let Some(p) = published.filter(|p| !p.0.is_empty() || !p.1.is_empty()) {
                    return p;
                }
                let fs = d
                    .functions
                    .values()
                    .filter(|x| x.app_id == f.app_id && x.created && x.base_function_id.is_none())
                    .map(|x| (x.name.clone(), x.id.clone()))
                    .collect();
                let cs = d
                    .classes
                    .values()
                    .filter(|x| x.app_id == f.app_id)
                    .map(|x| (x.name.clone(), x.id.clone()))
                    .collect();
                (fs, cs)
            });
        push(&f.id, &mut objects);
        for id in function_ids.values().chain(class_ids.values()) {
            push(id, &mut objects);
        }
        for dep in &def.object_dependencies {
            push(&dep.object_id, &mut objects);
        }
        pb::AppLayout {
            objects,
            function_ids: function_ids.into_iter().collect(),
            class_ids: class_ids.into_iter().collect(),
        }
    }

    pub fn object_for(&self, id: &str) -> pb::Object {
        use pb::object::HandleMetadataOneof as H;
        let handle = if id.starts_with("fu-") {
            self.function(id)
                .map(|f| H::FunctionHandleMetadata(self.function_handle(&f)))
        } else if id.starts_with("cs-") {
            self.db
                .read(|d| d.classes.get(id).cloned())
                .map(|c| H::ClassHandleMetadata(self.class_handle(&c)))
        } else if id.starts_with("vo-") {
            self.db.read(|d| d.volumes.get(id).cloned()).map(|v| {
                H::VolumeMetadata(pb::VolumeMetadata {
                    version: pb::VolumeFsVersion::V1 as i32,
                    name: v.name.unwrap_or_default(),
                    creation_info: None,
                })
            })
        } else if id.starts_with("mo-") {
            Some(H::MountHandleMetadata(pb::MountHandleMetadata::default()))
        } else {
            None
        };
        pb::Object {
            object_id: id.to_string(),
            handle_metadata_oneof: handle,
        }
    }

    /// App log lines for `AppGetLogs` / `AppFetchLogs` (container output).
    pub fn push_app_log(
        &self,
        app_id: &str,
        function_id: &str,
        task_id: &str,
        fd: usize,
        line: &str,
    ) {
        let at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut g = self.app_logs.lock().expect("app logs");
        let v = g.entry(app_id.to_string()).or_default();
        v.push(AppLogLine {
            at_ns,
            fd,
            text: line.to_string(),
            function_id: function_id.to_string(),
            task_id: task_id.to_string(),
        });
        if v.len() > 100_000 {
            v.drain(..10_000);
        }
        drop(g);
        self.fn_changed();
    }
}

/// One line of container output.
#[derive(Debug, Clone)]
pub struct AppLogLine {
    pub at_ns: u64,
    pub fd: usize,
    pub text: String,
    pub function_id: String,
    pub task_id: String,
}

/// Test only: runs the container entrypoint (`python -m
/// modal._container_entrypoint`, Modal's own) as a host process, so the
/// hermetic suites exercise the whole Function path without Spaces.
/// Selected with `CUA_MODAL_LOCAL_RUNTIME=<python>`; `CUA_MODAL_LOCAL_PYTHONPATH`
/// is the user code's import path (Modal places it with mounts in a real
/// container).
#[cfg(any(test, feature = "testing"))]
pub struct LocalRuntime {
    pub python: String,
    pub pythonpath: String,
}

#[cfg(any(test, feature = "testing"))]
struct LocalContainer {
    child: tokio::sync::Mutex<tokio::process::Child>,
    dir: std::path::PathBuf,
}

#[cfg(any(test, feature = "testing"))]
impl Drop for LocalContainer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(any(test, feature = "testing"))]
#[async_trait]
impl ContainerHandle for LocalContainer {
    async fn wait(&self) -> Option<i32> {
        let mut c = self.child.lock().await;
        c.wait().await.ok().map(|s| s.code().unwrap_or(137))
    }

    async fn kill(&self) {
        let _ = self.child.lock().await.start_kill();
    }
}

#[cfg(any(test, feature = "testing"))]
#[async_trait]
impl ContainerRuntime for LocalRuntime {
    async fn start(
        &self,
        _m: &Arc<Modal>,
        spec: ContainerSpec,
        logs: LogSink,
    ) -> Result<Box<dyn ContainerHandle>, String> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let dir = std::env::temp_dir().join(format!("cua-modal-fn-{}", spec.task_id));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let args = dir.join("container-arguments.bin");
        std::fs::write(&args, &spec.arguments).map_err(|e| e.to_string())?;
        // The function's mounts (its source) as a container would hold
        // them: under a private root, with `/root` first on the import path.
        let root = dir.join("rootfs");
        for (path, data, _) in &spec.files {
            let at = root.join(path.trim_start_matches('/'));
            if let Some(p) = at.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            std::fs::write(&at, data).map_err(|e| e.to_string())?;
        }
        let pythonpath = std::env::join_paths(
            [
                root.join("root"),
                std::path::PathBuf::from(&self.pythonpath),
            ]
            .into_iter()
            .filter(|p| !p.as_os_str().is_empty()),
        )
        .map_err(|e| e.to_string())?;
        let mut cmd = tokio::process::Command::new(&self.python);
        cmd.args(["-m", "modal._container_entrypoint"])
            .envs(&spec.env)
            .env("MODAL_CONTAINER_ARGUMENTS_PATH", &args)
            .env("MODAL_SERVER_URL", &spec.server_url)
            .env("PYTHONPATH", &pythonpath)
            .env("HOME", &dir)
            .current_dir(&dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("starting {}: {e}", self.python))?;
        for (fd, pipe) in [
            (
                0usize,
                child
                    .stdout
                    .take()
                    .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            ),
            (
                1usize,
                child
                    .stderr
                    .take()
                    .map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
            ),
        ] {
            let Some(pipe) = pipe else { continue };
            let logs = logs.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    logs(fd, &format!("{l}\n"));
                }
            });
        }
        Ok(Box::new(LocalContainer {
            child: tokio::sync::Mutex::new(child),
            dir,
        }))
    }
}
