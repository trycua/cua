//! Checkpoints and forks of local sandboxes.
//!
//! A checkpoint is a sandbox's saved state, kept under a name: memory and
//! disk where the runtime can (microVMs; gVisor containers on an engine
//! with cua's runsc wrapper; QEMU's own snapshots), else its filesystem
//! (containers). A fork is a new sandbox started from a sandbox (running or
//! stopped) or from a checkpoint. A microVM or container fork comes up with
//! its own identity (a new Spaces token); a QEMU fork keeps its source's.
//!
//! Records live next to the state files, in `<state dir>/.checkpoints/`
//! (owner-only: a record keeps the source's state, token included).
//!
//! Forking a running sandbox many times goes through an implied checkpoint
//! ([`Sandboxes::checkpoint_for_forks`]): its forks hold it
//! ([`CheckpointRecord::holders`]) and it is deleted when the last of them
//! is, so `fork(count = n)` of a live sandbox saves its memory once and
//! leaves nothing behind.

use super::*;
use serde::{Deserialize, Serialize};

/// A named checkpoint of a local sandbox.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckpointRecord {
    /// The checkpoint's name.
    pub name: String,
    /// The sandbox it was taken from.
    pub source: String,
    /// The engine that holds it (`firecracker`, `gvisor`, `qemu`, ...).
    pub runtime_type: String,
    /// ISO-8601 creation time.
    pub created_at: String,
    /// The source's state when it was taken: what every fork starts as.
    pub template: LocalState,
    /// Pinned: a fork source to keep. [`Sandboxes::delete_checkpoint`]
    /// refuses it and [`Sandboxes::prune_checkpoints`] leaves it until it
    /// is unpinned ([`Sandboxes::pin_checkpoint`]).
    #[serde(default)]
    pub pinned: bool,
    /// Its forks map its RAM copy-on-write and share the pages they do not
    /// write (a QEMU memory checkpoint).
    #[serde(default)]
    pub shared_memory: bool,
    /// Taken only to fork a running sandbox ([`Sandboxes::checkpoint_for_forks`]):
    /// deleted when the last fork that holds it is deleted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub release_with_forks: bool,
    /// The forks that started from it and still exist, for a checkpoint
    /// that is released with its forks.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub holders: std::collections::BTreeSet<String>,
}

/// Serializes changes to checkpoint holders within this process (forks of
/// one checkpoint start and end concurrently).
static HOLDS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Directory of the checkpoint records, inside the state directory
/// (hidden from state listings, which read `*.json` at the top).
pub const CHECKPOINT_DIR: &str = ".checkpoints";

fn valid_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "invalid name {name:?}: 1-63 characters of [A-Za-z0-9._-], starting with a letter or digit"
        )))
    }
}

impl Sandboxes {
    fn checkpoint_path(&self, name: &str) -> Result<PathBuf> {
        valid_name(name)?;
        Ok(self
            .inner
            .state
            .dir()
            .join(CHECKPOINT_DIR)
            .join(format!("{name}.json")))
    }

    /// The record of checkpoint `name`.
    pub fn checkpoint_record(&self, name: &str) -> Option<CheckpointRecord> {
        let p = self.checkpoint_path(name).ok()?;
        serde_json::from_slice(&std::fs::read(p).ok()?).ok()
    }

    /// Whether `name` is a checkpoint and no sandbox has that name.
    pub(crate) fn is_checkpoint_only(&self, name: &str) -> bool {
        self.inner.state.load(name).is_none() && self.checkpoint_record(name).is_some()
    }

    /// Every checkpoint, oldest first.
    pub fn checkpoints(&self) -> Vec<CheckpointRecord> {
        let Ok(rd) = std::fs::read_dir(self.inner.state.dir().join(CHECKPOINT_DIR)) else {
            return vec![];
        };
        let mut v: Vec<CheckpointRecord> = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        v
    }

    /// The local state of a sandbox that can be checkpointed or forked.
    fn forkable(&self, name: &str) -> Result<Option<LocalState>> {
        match self.inner.state.load(name) {
            None => Ok(None),
            Some(s) if contrib::contrib_of(&s).is_some() => Err(Error::Unsupported {
                provider: ProviderKind::Contrib,
                op: "checkpoints and forks (local sandboxes only)".into(),
            }),
            Some(SandboxState::Local(l)) if l.runtime_type != "direct" => Ok(Some(l)),
            Some(s) => Err(Error::Unsupported {
                provider: provider_of(&s),
                op: "checkpoints and forks (local sandboxes only)".into(),
            }),
        }
    }

    /// Saves sandbox `name`'s state as checkpoint `checkpoint` (memory and
    /// disk on a microVM; the filesystem of a container). A running
    /// sandbox keeps running. [`Sandboxes::checkpoint_with`] without a pin.
    pub async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<CheckpointRecord> {
        self.checkpoint_with(name, checkpoint, false).await
    }

    /// [`Sandboxes::checkpoint`], pinning the new checkpoint when `pin`
    /// (see [`Sandboxes::pin_checkpoint`]). The record says whether its
    /// forks share its RAM ([`CheckpointRecord::shared_memory`]).
    pub async fn checkpoint_with(
        &self,
        name: &str,
        checkpoint: &str,
        pin: bool,
    ) -> Result<CheckpointRecord> {
        let path = self.checkpoint_path(checkpoint)?;
        let template = self
            .forkable(name)?
            .ok_or_else(|| Error::NotFound(name.into()))?;
        if path.exists() || self.inner.state.load(checkpoint).is_some() {
            return Err(Error::InvalidArgument(format!(
                "{checkpoint:?} is already a checkpoint or a sandbox"
            )));
        }
        let rt = self.local()?;
        let shared_memory = rt.checkpoint_shared(name, checkpoint).await?;
        if pin
            && let Err(e) = rt
                .pin_checkpoint(&template.runtime_type, checkpoint, true)
                .await
        {
            // No record, no checkpoint: nothing would list or delete it.
            let _ = rt
                .delete_checkpoint(&template.runtime_type, checkpoint)
                .await;
            return Err(e.into());
        }
        let rec = CheckpointRecord {
            name: checkpoint.to_string(),
            source: name.to_string(),
            runtime_type: template.runtime_type.clone(),
            created_at: python_utc_now(),
            template,
            pinned: pin,
            shared_memory,
            release_with_forks: false,
            holders: Default::default(),
        };
        self.write_checkpoint(&path, &rec)?;
        Ok(rec)
    }

    /// Checkpoints running (or stopped) sandbox `source` to fork it many
    /// times: a checkpoint named `<source>-forks-<hex>` that its forks hold
    /// and that is deleted with the last of them ([`CheckpointRecord::holders`]).
    /// Its memory is saved once, and on QEMU every fork shares it
    /// copy-on-write.
    pub async fn checkpoint_for_forks(&self, source: &str) -> Result<CheckpointRecord> {
        let stem: String = source.chars().take(48).collect();
        let name = format!("{stem}-forks-{:06x}", rand::random::<u32>() & 0xff_ffff);
        let mut rec = self.checkpoint_with(source, &name, false).await?;
        rec.release_with_forks = true;
        self.write_checkpoint(&self.checkpoint_path(&name)?, &rec)?;
        Ok(rec)
    }

    /// Records fork `fork` as holding checkpoint `checkpoint` when that one
    /// is released with its forks; true when it did.
    async fn hold(&self, checkpoint: &str, fork: &str) -> Result<bool> {
        let _g = HOLDS.lock().await;
        let Some(mut rec) = self.checkpoint_record(checkpoint) else {
            return Ok(false);
        };
        if !rec.release_with_forks {
            return Ok(false);
        }
        rec.holders.insert(fork.to_string());
        self.write_checkpoint(&self.checkpoint_path(checkpoint)?, &rec)?;
        Ok(true)
    }

    /// Fork `fork` is gone: it no longer holds the checkpoints it started
    /// from, and a checkpoint released with its forks that nothing holds now
    /// is deleted. Failures are logged (the fork's own delete succeeded).
    pub async fn release_holds(&self, fork: &str) {
        let _g = HOLDS.lock().await;
        for mut rec in self.checkpoints() {
            if !rec.holders.remove(fork) {
                continue;
            }
            let path = match self.checkpoint_path(&rec.name) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if rec.release_with_forks && rec.holders.is_empty() && !rec.pinned {
                match self.local() {
                    Ok(rt) => match rt.delete_checkpoint(&rec.runtime_type, &rec.name).await {
                        Ok(()) | Err(crate::RuntimeError::NotFound(_)) => {
                            let _ = std::fs::remove_file(&path);
                            tracing::info!(checkpoint = %rec.name, "its last fork is gone: deleted");
                        }
                        Err(e) => {
                            tracing::warn!(checkpoint = %rec.name, error = %e, "could not delete a checkpoint its forks released");
                            let _ = self.write_checkpoint(&path, &rec);
                        }
                    },
                    Err(_) => {
                        let _ = self.write_checkpoint(&path, &rec);
                    }
                }
            } else if let Err(e) = self.write_checkpoint(&path, &rec) {
                tracing::warn!(checkpoint = %rec.name, error = %e, "could not release a fork's hold");
            }
        }
    }

    /// Deletes checkpoint `name` when it is released with its forks and no
    /// fork holds it (a batch whose forks all failed). True when deleted.
    pub async fn release_if_unheld(&self, name: &str) -> bool {
        let rec = {
            let _g = HOLDS.lock().await;
            match self.checkpoint_record(name) {
                Some(r) if r.release_with_forks && r.holders.is_empty() && !r.pinned => r,
                _ => return false,
            }
        };
        self.delete_checkpoint(&rec.name).await.is_ok()
    }

    fn write_checkpoint(&self, path: &std::path::Path, rec: &CheckpointRecord) -> Result<()> {
        std::fs::create_dir_all(path.parent().unwrap_or(self.inner.state.dir()))?;
        crate::state::write_private(path, &serde_json::to_vec_pretty(rec)?)?;
        Ok(())
    }

    /// Pins (`pinned`) or unpins checkpoint `checkpoint`. A pinned
    /// checkpoint is a fork source to keep: [`Sandboxes::delete_checkpoint`]
    /// refuses it and [`Sandboxes::prune_checkpoints`] leaves it. Returns
    /// the record as it is now. A checkpoint the engine no longer has can
    /// still be unpinned (and then deleted), so a stale record never sticks.
    pub async fn pin_checkpoint(&self, checkpoint: &str, pinned: bool) -> Result<CheckpointRecord> {
        let path = self.checkpoint_path(checkpoint)?;
        let mut rec = self
            .checkpoint_record(checkpoint)
            .ok_or_else(|| Error::NotFound(format!("checkpoint {checkpoint}")))?;
        match self
            .local()?
            .pin_checkpoint(&rec.runtime_type, checkpoint, pinned)
            .await
        {
            Ok(()) => {}
            // The engine lost the checkpoint (its instance is gone): the
            // record is all that is left, and unpinning it is how it can be
            // deleted. Pinning something that no longer exists is an error.
            Err(crate::RuntimeError::NotFound(_)) if !pinned => {}
            Err(e) => return Err(e.into()),
        }
        rec.pinned = pinned;
        self.write_checkpoint(&path, &rec)?;
        Ok(rec)
    }

    /// Deletes checkpoint `checkpoint` (forks made from it keep running).
    /// A pinned checkpoint is refused ([`Error::InvalidArgument`]) until it
    /// is unpinned.
    pub async fn delete_checkpoint(&self, checkpoint: &str) -> Result<()> {
        let path = self.checkpoint_path(checkpoint)?;
        let rec = self
            .checkpoint_record(checkpoint)
            .ok_or_else(|| Error::NotFound(format!("checkpoint {checkpoint}")))?;
        if rec.pinned {
            return Err(Error::InvalidArgument(format!(
                "checkpoint {checkpoint} is pinned; unpin it first (`cua sb checkpoints unpin \
                 {checkpoint}`)"
            )));
        }
        match self
            .local()?
            .delete_checkpoint(&rec.runtime_type, checkpoint)
            .await
        {
            Ok(()) | Err(crate::RuntimeError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        std::fs::remove_file(path)?;
        Ok(())
    }

    /// Checkpoint `checkpoint` as files, with its record, to copy it to
    /// another host (snapshot sync).
    pub async fn export_checkpoint(
        &self,
        checkpoint: &str,
    ) -> Result<(CheckpointRecord, cua_vmm::CheckpointExport)> {
        let rec = self
            .checkpoint_record(checkpoint)
            .ok_or_else(|| Error::NotFound(format!("checkpoint {checkpoint}")))?;
        let ex = self
            .local()?
            .export_checkpoint_on(&rec.runtime_type, checkpoint)
            .await?;
        Ok((rec, ex))
    }

    /// Where a copied checkpoint of engine `runtime_type` and manifest
    /// `digest` is laid out before [`Sandboxes::import_checkpoint`].
    pub async fn import_dir(&self, runtime_type: &str, digest: &str) -> Option<PathBuf> {
        self.local().ok()?.import_dir_on(runtime_type, digest).await
    }

    /// Makes a copied checkpoint a checkpoint here, named `name`: `record`
    /// is the record it had where it was taken, `state` the engine's state
    /// with this host's paths, `qcow2_rebase` its disk layers to point at
    /// their copied backing files.
    pub async fn import_checkpoint(
        &self,
        name: &str,
        record: CheckpointRecord,
        state: serde_json::Value,
        qcow2_rebase: Vec<(PathBuf, PathBuf)>,
    ) -> Result<CheckpointRecord> {
        let path = self.checkpoint_path(name)?;
        if path.exists() || self.inner.state.load(name).is_some() {
            return Err(Error::InvalidArgument(format!(
                "{name:?} is already a checkpoint or a sandbox"
            )));
        }
        self.local()?
            .import_checkpoint_on(&record.runtime_type, name, state, qcow2_rebase)
            .await?;
        // `created_at` stays when the checkpoint was taken: hosts tell
        // copies of one checkpoint from a new one of the same name by it.
        let rec = CheckpointRecord {
            name: name.to_string(),
            ..record
        };
        std::fs::create_dir_all(path.parent().unwrap_or(self.inner.state.dir()))?;
        crate::state::write_private(&path, &serde_json::to_vec_pretty(&rec)?)?;
        Ok(rec)
    }

    /// The checkpoint ABIs this machine restores.
    pub async fn snapshot_abis(&self) -> Vec<String> {
        match self.local() {
            Ok(rt) => rt.snapshot_abis().await,
            Err(_) => vec![],
        }
    }

    /// Deletes every checkpoint that is not pinned (forks made from them
    /// keep running) and returns their names. One that cannot be deleted
    /// is logged and left; the rest are still deleted. A checkpoint its
    /// forks still hold ([`CheckpointRecord::holders`]) is kept: it goes
    /// with its last fork.
    pub async fn prune_checkpoints(&self) -> Result<Vec<String>> {
        let mut deleted = Vec::new();
        for rec in self
            .checkpoints()
            .into_iter()
            .filter(|r| !r.pinned && r.holders.is_empty())
        {
            match self.delete_checkpoint(&rec.name).await {
                Ok(()) => deleted.push(rec.name),
                Err(e) => {
                    tracing::warn!(checkpoint = %rec.name, error = %e, "prune: could not delete a checkpoint; it is kept")
                }
            }
        }
        Ok(deleted)
    }

    /// Starts `new_name`, a copy of sandbox `source` (running or stopped)
    /// or of checkpoint `source`, and waits for it as its source's create
    /// did. A sandbox of that name wins over a checkpoint of that name.
    pub async fn fork(&self, source: &str, new_name: &str) -> Result<Sandbox> {
        valid_name(new_name)?;
        if self.inner.state.load(new_name).is_some() || self.local_name_in_use(new_name).await {
            return Err(Error::InvalidArgument(format!(
                "a sandbox named {new_name:?} already exists"
            )));
        }
        let (template, from_checkpoint) = match self.forkable(source)? {
            Some(l) => (l, false),
            None => match self.checkpoint_record(source) {
                Some(rec) => (rec.template, true),
                None => {
                    return Err(Error::NotFound(format!(
                        "{source} (neither a sandbox nor a checkpoint)"
                    )));
                }
            },
        };
        let rt = self.local()?;
        let started = Instant::now();
        let runtime_type = template.runtime_type.clone();
        // Held before the fork starts, so the checkpoint's last other fork
        // ending meanwhile does not take it away.
        let held = from_checkpoint && self.hold(source, new_name).await?;
        let made = if from_checkpoint {
            rt.fork_on(&runtime_type, source, new_name).await
        } else {
            rt.fork(source, new_name).await
        };
        if let Err(e) = made {
            if held {
                self.release_holds(new_name).await;
            }
            return Err(e.into());
        }
        let inherited = template
            .extra
            .get("env_token")
            .and_then(Value::as_str)
            .map(str::to_string);
        let own_identity = rt.fork_has_own_identity(&runtime_type);
        let token = match &inherited {
            Some(_) if own_identity => Some(mint_env_token()),
            other => other.clone(),
        };
        let services: BTreeMap<String, u16> = template
            .extra
            .get("services")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let probes = template
            .extra
            .get("ready_probes")
            .cloned()
            .map(probes_from_json)
            .unwrap_or_default();
        let mut ports: Vec<u16> = services
            .values()
            .copied()
            .filter(|p| *p > 0)
            .chain(
                template
                    .exposed_ports
                    .iter()
                    .flatten()
                    .filter_map(|(g, _)| g.parse().ok()),
            )
            .collect();
        ports.sort_unstable();
        ports.dedup();
        let spec = LocalStartSpec {
            ephemeral: false,
            warm: Some(false),
            mounts: vec![],
            forkable: false,
            name: new_name.to_string(),
            image: template
                .image
                .get("registry")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            os: template.os_type.clone().unwrap_or_else(|| "linux".into()),
            cpus: template.cpu_count.unwrap_or(2),
            memory_mb: template.memory_mb.unwrap_or(2048),
            disk_size_gb: None,
            ports,
            env: token
                .iter()
                .map(|t| ("CUA_ENV_TOKEN".to_string(), t.clone()))
                .collect(),
            command: None,
            ready_timeout: RESUME_READY_TIMEOUT,
            firmware: None,
            sidecars: vec![],
            registry_credentials: None,
            container_runtime: None,
            kind: None,
            runtime: None,
            restrict_network: false,
            // A fork restores the source's memory: headless or not as it was.
            headless: false,
            spacesd: None,
            egress: Default::default(),
            gpu: None,
            probes: probes
                .iter()
                .map(|p| match p {
                    Probe::Tcp(port) => crate::LocalProbe {
                        port: *port,
                        http_path: None,
                    },
                    Probe::Http { port, path, .. } => crate::LocalProbe {
                        port: *port,
                        http_path: Some(path.clone()),
                    },
                })
                .collect(),
        };
        let t_fork = started.elapsed();
        let inst = match rt.start_fork(&runtime_type, &spec).await {
            Ok(i) => i,
            Err(e) => {
                let _ = rt.delete(new_name).await;
                if held {
                    self.release_holds(new_name).await;
                }
                return Err(e.into());
            }
        };
        let ep = &inst.endpoints;
        let mut l = template;
        l.name = new_name.to_string();
        l.runtime_type = inst.backend.clone();
        l.host = ep.host.clone();
        l.api_port = ep
            .ports
            .get(&ENV_PORT)
            .copied()
            .or_else(|| ep.ports.values().next().copied())
            .unwrap_or(ENV_PORT);
        l.exposed_ports = Some(ep.ports.iter().map(|(g, h)| (g.to_string(), *h)).collect());
        l.status = "running".into();
        l.created_at = python_utc_now();
        l.extra.remove("env_token");
        if let Some(t) = &token {
            l.extra.insert("env_token".into(), Value::String(t.clone()));
        }
        l.extra
            .insert("forked_from".into(), Value::String(source.to_string()));
        if let Some(c) = &ep.container_id {
            l.extra
                .insert("container_id".into(), Value::String(c.clone()));
        }
        self.inner.state.save(&SandboxState::Local(l))?;
        let t_start = started.elapsed();
        let ready = async {
            let sb = self.connect(new_name).await?;
            if !probes.is_empty() {
                sb.wait_ready(&probes, RESUME_READY_TIMEOUT).await?;
            }
            let t_probes = started.elapsed();
            if sb.image_info.as_ref().and_then(|i| i.spacesd) == Some(true)
                && sb.services.contains_key("env")
            {
                if sb.restores_shared_memory() {
                    sb.wait_new_machine(RESUME_READY_TIMEOUT).await?;
                } else {
                    sb.wait_spacesd(RESUME_READY_TIMEOUT).await?;
                }
                tracing::debug!(
                    sandbox = new_name,
                    fork_ms = t_fork.as_millis() as u64,
                    start_ms = (t_start - t_fork).as_millis() as u64,
                    probes_ms = (t_probes - t_start).as_millis() as u64,
                    spacesd_ms = (started.elapsed() - t_probes).as_millis() as u64,
                    "fork ready"
                );
            }
            Ok::<_, Error>(sb)
        }
        .await;
        if ready.is_err() {
            let _ = rt.delete(new_name).await;
            let _ = self.inner.state.delete(new_name);
            if held {
                self.release_holds(new_name).await;
            }
        }
        ready
    }
}

impl Sandbox {
    /// [`Sandbox::wait_spacesd`] for a new Space that may have restored
    /// memory other Spaces share (a fork, a microVM's warm start): its
    /// cua-spacesd is told it is a new machine (`Init{new_machine,
    /// entropy}` with the Space's own token, which regenerates the keys it
    /// holds in memory), and that call is also the readiness check: it
    /// succeeds once the driver answers and accepts the token the backend
    /// delivered. Drivers that predate `new_machine` take it as a plain
    /// authenticated `Init` (presenting the current token changes nothing).
    /// Never used when a Space restarts or resumes itself.
    pub(super) async fn wait_new_machine(&self, timeout: Duration) -> Result<()> {
        let Some(token) = self.token() else {
            return self.wait_spacesd(timeout).await;
        };
        let deadline = Instant::now() + timeout;
        let mut client = None;
        let mut refused_since: Option<Instant> = None;
        loop {
            let last = match &client {
                None => match self
                    .env_connect(
                        &ConnectOptionsOverride {
                            probe_timeout: Some(Duration::from_secs(3)),
                            ..Default::default()
                        },
                        Some(token.clone()),
                    )
                    .await
                {
                    Ok(c) => {
                        client = Some(c);
                        continue;
                    }
                    Err(e) => e.to_string(),
                },
                Some(c) => {
                    let init = c
                        .init(cua_spacesd_client::pb::InitRequest {
                            token: token.clone(),
                            new_machine: true,
                            entropy: rand::random::<[u8; 32]>().to_vec(),
                            ..Default::default()
                        })
                        .await;
                    match init {
                        Ok(_) => return Ok(()),
                        // The driver has not applied the delivered token
                        // yet; a wrong one never heals, past the grace.
                        Err(
                            e @ (cua_spacesd_client::Error::NotInitialized(_)
                            | cua_spacesd_client::Error::Unauthenticated(_)
                            | cua_spacesd_client::Error::PermissionDenied(_)),
                        ) => {
                            let since = *refused_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= TOKEN_APPLY_GRACE || Instant::now() >= deadline {
                                return Err(Error::Env(e));
                            }
                            e.to_string()
                        }
                        Err(e) => {
                            client = None;
                            e.to_string()
                        }
                    }
                }
            };
            if let Some(why) = self.exited().await {
                return Err(Error::Runtime(crate::RuntimeError::Other(format!(
                    "sandbox {} {why} before cua-spacesd answered; see `cua sb logs {}`",
                    self.name, self.name
                ))));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "sandbox {}: cua-spacesd did not answer within {timeout:?} ({last}); \
                     see `cua sb logs {}`",
                    self.name, self.name
                )));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Whether this local Space runs on an engine that starts new instances
    /// from memory other instances share and leaves telling cua-spacesd to
    /// the client: a Firecracker microVM (golden snapshots, forks). Engines
    /// that restore memory and tell the driver themselves (gVisor) are not.
    pub(super) fn restores_shared_memory(&self) -> bool {
        matches!(
            self.mgr.inner.state.load(&self.name),
            Some(SandboxState::Local(l)) if l.runtime_type == "firecracker"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_names_are_validated() {
        assert!(valid_name("ckpt-1").is_ok());
        assert!(valid_name("../x").is_err());
        assert!(valid_name(".hidden").is_err());
        assert!(valid_name("").is_err());
    }
}
