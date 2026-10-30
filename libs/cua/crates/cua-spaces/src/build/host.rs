//! [`Spaces::build`] and the builder Space behind it: a [`BuildHost`] over
//! the Spaces runtime (a builder is a Space; its shell and file upload are
//! cua-spacesd's; its layers are pinned sandbox checkpoints).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_spacesd_client::{Command, ProcessEvent, UploadOptions, pb};

use super::engine::{self, BuildHost, BuilderSpec, ExecOutcome, ExecSpec, LineSink, RunOpts};
use super::plan::{self, Shape};
use super::recipe::{BuildOptions, BuildResult};
use super::store::{self, BuildInfo};
use crate::error::{Error, Result};
use crate::spaces::{SpaceCreate, SpaceCreated, Spaces};

/// `build:<tag>` as an image: a Space made from that build (a fork of its
/// checkpoint).
pub const BUILD_IMAGE_PREFIX: &str = "build:";

/// Default vCPUs and memory of a build's machine (Spaces made from the build
/// have them).
const DEFAULT_CPUS: u32 = 2;
const DEFAULT_MEMORY_MB: u64 = 4096;
const DEFAULT_STEP_TIMEOUT: Duration = Duration::from_secs(3600);

fn local_name(id: &str) -> Result<String> {
    match crate::id::SpaceId::parse(id)? {
        crate::id::SpaceId::Local { name } => Ok(name),
        other => Err(Error::invalid(format!("{other} is not a local builder"))),
    }
}

#[async_trait]
impl BuildHost for Spaces {
    fn home(&self) -> &Path {
        self.home_dir()
    }

    async fn has_layer(&self, name: &str) -> bool {
        self.sandboxes().checkpoint_record(name).is_some()
    }

    async fn start_base(&self, spec: &BuilderSpec) -> Result<String> {
        let created = self.create(builder_create(spec)).await?;
        match created {
            SpaceCreated::Ready { info, .. } => Ok(info.id),
            SpaceCreated::Starting(p) => Ok(p.id),
        }
    }

    async fn start_layer(&self, layer: &str, name: &str) -> Result<String> {
        Ok(self.fork_local(layer, Some(name.to_string())).await?.id)
    }

    async fn exec(
        &self,
        builder: &str,
        spec: ExecSpec,
        on_line: &mut dyn LineSink,
    ) -> Result<ExecOutcome> {
        let space = self.space(builder).await?;
        let client = space.spacesd()?;
        let mut cmd = Command::new(spec.program).args(spec.args);
        for (k, v) in spec.env {
            cmd = cmd.env(k, v);
        }
        if let Some(cwd) = spec.cwd {
            cmd = cmd.cwd(cwd);
        }
        if let Some(user) = spec.user {
            cmd = cmd.user(user);
        }
        cmd = cmd.timeout(spec.timeout);
        let mut h = client.spawn(cmd).await?;
        let mut pending = String::new();
        // A separate stream (stdout/stderr) may interleave mid-line; lines
        // are split per stream.
        let mut pending_err = String::new();
        let flush = |buf: &mut String, on_line: &mut dyn LineSink, all: bool| {
            while let Some(i) = buf.find('\n') {
                let line: String = buf.drain(..=i).collect();
                on_line.line(line.trim_end_matches(['\n', '\r']));
            }
            if all && !buf.is_empty() {
                on_line.line(buf.trim_end_matches(['\n', '\r']));
                buf.clear();
            }
        };
        loop {
            match h.next_event().await? {
                Some(ProcessEvent::Stdout { data, .. } | ProcessEvent::Pty { data, .. }) => {
                    pending.push_str(&String::from_utf8_lossy(&data));
                    flush(&mut pending, on_line, false);
                }
                Some(ProcessEvent::Stderr { data, .. }) => {
                    pending_err.push_str(&String::from_utf8_lossy(&data));
                    flush(&mut pending_err, on_line, false);
                }
                Some(ProcessEvent::Exit(st)) => {
                    flush(&mut pending, on_line, true);
                    flush(&mut pending_err, on_line, true);
                    return Ok(ExecOutcome {
                        code: st.code,
                        error: st
                            .error
                            .clone()
                            .or_else(|| st.signal.map(|s| format!("killed by {s:?}"))),
                        timed_out: st.timed_out,
                    });
                }
                None => {
                    return Ok(ExecOutcome {
                        code: None,
                        error: Some("the command's stream ended without an exit".into()),
                        timed_out: false,
                    });
                }
            }
        }
    }

    async fn put(&self, builder: &str, path: &str, data: Vec<u8>, mode: u32) -> Result<()> {
        let space = self.space(builder).await?;
        space
            .spacesd()?
            .upload(
                path,
                data,
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    permissions: mode,
                    create_parents: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

    async fn put_file(&self, builder: &str, path: &str, host: &Path) -> Result<()> {
        let space = self.space(builder).await?;
        space
            .spacesd()?
            .upload(
                path,
                host,
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    permissions: 0o644,
                    create_parents: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

    async fn save_layer(&self, builder: &str, name: &str) -> Result<()> {
        let sandbox = local_name(builder)?;
        self.sandboxes()
            .checkpoint_with(&sandbox, name, true)
            .await?;
        Ok(())
    }

    async fn drop_layer(&self, name: &str) {
        let sbx = self.sandboxes();
        if sbx.checkpoint_record(name).is_none() {
            return;
        }
        let _ = sbx.pin_checkpoint(name, false).await;
        if let Err(e) = sbx.delete_checkpoint(name).await {
            tracing::warn!(layer = name, "could not delete a build layer: {e}");
        }
    }

    async fn drop_builder(&self, builder: &str) {
        if let Err(e) = self.delete(builder).await {
            tracing::warn!(builder, "could not delete a builder Space: {e}");
        }
    }
}

impl Spaces {
    /// Builds an image from a Dockerfile or steps, in a Space, as pinned
    /// layer checkpoints (see [`crate::build`]). Layers already built are
    /// reused; identical builds in flight on this machine share one run. A
    /// failed step returns [`Error::Build`] with the step's command, exit
    /// code and last output; earlier layers stay cached and the builder is
    /// deleted.
    ///
    /// The result's checkpoint is a fork source (`Spaces::fork_local`,
    /// `cua sb fork`, `image = "build:<tag>"`).
    pub async fn build(&self, opts: BuildOptions) -> Result<BuildResult> {
        match &opts.on {
            None | Some(On::Local) => {}
            Some(other) => {
                return Err(Error::invalid(format!(
                    "building on {other} is not supported: a build's layers are checkpoints, and a checkpoint lives on the machine that took it. Build on that machine (run `cua build` there), or copy the checkpoint to this one with `cua sb publish` or the fork-many snapshot sync"
                )));
            }
        }
        let kind = match opts.kind {
            Kind::Auto | Kind::Vm => "vm",
            Kind::Container => "container",
            Kind::MicroVm => "microvm",
        };
        self.reap_builders().await;
        let recipe = opts.to_recipe()?;
        let shape = Shape {
            kind: kind.to_string(),
            cpus: opts.cpus.unwrap_or(DEFAULT_CPUS),
            memory_mb: opts.memory_mb.unwrap_or(DEFAULT_MEMORY_MB),
            headless: opts.headless.unwrap_or(true),
        };
        if let Some(t) = &opts.tag
            && !store::valid_tag(t)
        {
            return Err(Error::invalid(format!(
                "invalid tag {t:?}: 1-63 characters of [A-Za-z0-9._-], starting with a letter or digit"
            )));
        }
        // A tag that moved is another base: its digest is in the first key.
        let image = super::classify_base(&recipe.base)?.image;
        let digest = self.resolve_base_digest(&image, kind).await;
        let plan = Arc::new(plan::plan(
            &recipe,
            opts.context_dir.as_deref(),
            shape,
            digest.as_deref(),
        )?);
        let run_opts = RunOpts {
            tag: opts.tag.clone(),
            no_cache: opts.no_cache,
            step_timeout: opts.step_timeout.unwrap_or(DEFAULT_STEP_TIMEOUT),
            sink: opts.progress.clone(),
        };
        let flight_key = format!(
            "{}|{}|{}",
            self.home_dir().display(),
            plan.key,
            opts.no_cache
        );
        let spaces = self.clone();
        let shared_plan = plan.clone();
        // The shared run reports to the first caller only; every caller
        // applies its own tag below.
        let shared_opts = RunOpts {
            tag: None,
            ..run_opts.clone()
        };
        let (flight, leader) = engine::shared(flight_key, move || async move {
            engine::run(&spaces, &shared_plan, &shared_opts).await
        });
        if !leader && let Some(s) = &opts.progress {
            s(&engine::BuildEvent::Note {
                text: "an identical build is already running; waiting for it".into(),
            });
        }
        let mut result = flight.await.map_err(|e| match &*e {
            Error::Build {
                step,
                command,
                exit_code,
                output,
            } => Error::Build {
                step: *step,
                command: command.clone(),
                exit_code: *exit_code,
                output: output.clone(),
            },
            other => Error::Build {
                step: 0,
                command: String::new(),
                exit_code: None,
                output: other.to_string(),
            },
        })?;
        if !leader {
            // A waiter's result is the leader's, as a cache hit.
            result.cached = true;
            for s in &mut result.steps {
                s.cached = true;
                s.seconds = 0.0;
            }
        }
        if let Some(tag) = &opts.tag {
            store::set_tag(self.home_dir(), tag, &result.key)?;
            result.tag = Some(tag.clone());
        }
        Ok(result)
    }

    /// The digest of the variant of `image` this build runs: from the
    /// registry, else (offline) from this machine's image cache.
    async fn resolve_base_digest(&self, image: &str, kind: &str) -> Option<String> {
        use cua_image::Backend;
        let backend = if kind == "container" {
            Backend::Container
        } else {
            Backend::Vm
        };
        match cua_image::resolve(image, backend, std::env::consts::ARCH).await {
            Ok(r) => Some(r.digest),
            Err(e) => {
                tracing::debug!(
                    image,
                    "base digest not resolved ({e}); using the local cache"
                );
                cua_image::ImageCache::new(self.home_dir().join("images"))
                    .refs()
                    .into_iter()
                    .find(|(r, _)| r == image)
                    .map(|(_, d)| d)
            }
        }
    }

    /// Deletes builder Spaces left by builds whose process is gone (Ctrl-C,
    /// a crash).
    pub async fn reap_builders(&self) {
        for id in store::stale_builders(self.home_dir(), &cua_host::service::process_alive) {
            tracing::info!(builder = %id, "deleting the builder of a build that was cut off");
            if self.delete(&id).await.is_ok() || self.registry().get(&id).ok().flatten().is_none() {
                store::forget_builder(self.home_dir(), &id);
            }
        }
    }

    /// The builds on this machine, oldest first, with their tags.
    pub fn builds(&self) -> Result<Vec<BuildInfo>> {
        let home = self.home_dir();
        let tags = store::tags(home);
        Ok(store::load_records(home)
            .into_iter()
            .filter(|r| self.sandboxes().checkpoint_record(&r.checkpoint).is_some())
            .map(|r| BuildInfo {
                tags: tags
                    .iter()
                    .filter(|(_, k)| k.get(..12) == r.key.get(..12))
                    .map(|(t, _)| t.clone())
                    .collect(),
                base: r.config.base.clone(),
                checkpoint: r.checkpoint,
                key: r.key,
                layers: r.layers,
                created_at: r.created_at,
                config: r.config,
            })
            .collect())
    }

    /// The checkpoint a tag, a recipe key or a layer name refers to
    /// (`build:` prefix optional), when it exists on this machine.
    pub fn build_checkpoint(&self, reference: &str) -> Option<String> {
        let cp = store::resolve_ref(self.home_dir(), reference)?;
        self.sandboxes().checkpoint_record(&cp).map(|_| cp)
    }

    /// The image configuration of the build `reference` names.
    pub fn build_config(&self, reference: &str) -> Option<super::recipe::ImageConfig> {
        let home = self.home_dir();
        let r = reference
            .strip_prefix(BUILD_IMAGE_PREFIX)
            .unwrap_or(reference);
        let key = store::tags(home).get(r).cloned();
        if let Some(rec) = key.and_then(|k| store::load_record(home, &k)) {
            return Some(rec.config);
        }
        store::load_records(home)
            .into_iter()
            .find(|rec| rec.checkpoint == r || rec.key.starts_with(r) && r.len() >= 12)
            .map(|rec| rec.config)
    }

    /// Writes a build's image configuration into Space `id`'s registry
    /// record, so every later connection (this process, the daemon, an SDK
    /// attached to it) applies it.
    pub(crate) fn record_image_config(
        &self,
        id: &str,
        config: &super::recipe::ImageConfig,
    ) -> Result<()> {
        let registry = self.registry();
        let Some(mut record) = registry.get(id)? else {
            return Ok(());
        };
        record.image_config = serde_json::to_string(config)?;
        let credential = registry.credential(id)?.unwrap_or_default();
        registry.upsert(record, credential)
    }

    /// A Space made from a build: a fork of the build's checkpoint with the
    /// image's environment, working directory and user (`image` is
    /// `build:<tag, key or checkpoint>`). The machine is the build's: a VM
    /// fork given other vCPUs, memory or a bigger disk is resized right
    /// after the fork (it boots again from its own disk, which has every
    /// build step on it); options that would reshape it otherwise are
    /// refused.
    pub(crate) async fn create_from_build(
        &self,
        reference: &str,
        opts: SpaceCreate,
    ) -> Result<SpaceCreated> {
        use cua_sandbox_core::progress::{Phase, Progress, report};
        let refuse = |what: &str| {
            Err(Error::invalid(format!(
                "{what} does not apply to a Space made from a build: it is a fork of the build's machine; set it when you build (cpus, memory) or run the Space after creating it"
            )))
        };
        if !matches!(opts.on, None | Some(On::Local)) {
            return Err(Error::invalid(format!(
                "build:{reference} runs on the machine that built it (local), not {}",
                opts.on.as_ref().map(|o| o.to_string()).unwrap_or_default()
            )));
        }
        let reshape = cua_sandbox_core::Resize {
            cpus: opts.cpus,
            memory_mb: opts.memory_mb,
            disk_gb: opts.disk_gb,
            probes: Vec::new(),
        };
        if opts.gpu.is_some() {
            return refuse("gpu");
        }
        if opts.durable {
            return refuse("durable");
        }
        if !opts.services.is_empty() {
            return refuse("services");
        }
        if opts.command.is_some() {
            return refuse("command");
        }
        if !opts.mounts.is_empty() {
            return refuse("mounts");
        }
        let Some(checkpoint) = self.build_checkpoint(reference) else {
            return Err(Error::NotFound(format!(
                "build {reference} (build it first: `cua build -t {reference}`)"
            )));
        };
        let sink = opts.progress.map(|p| p.0);
        let create = async {
            report(Progress::phase(Phase::Creating));
            let info = self.fork_local(&checkpoint, opts.name.clone()).await?;
            if !reshape.is_empty() {
                let name = info
                    .id
                    .strip_prefix("local:")
                    .unwrap_or(&info.id)
                    .to_string();
                let sized = async {
                    let runtime = self.sandboxes().get(&name).await?.runtime_type;
                    if runtime != "qemu" {
                        return Err(Error::invalid(format!(
                            "cpus, memory and disk_gb of a Space made from a build need a VM build \
                             (this one runs on {runtime}); set them when you build instead"
                        )));
                    }
                    self.sandboxes().resize(&name, &reshape).await?;
                    Ok(())
                }
                .await;
                if let Err(e) = sized {
                    // Nothing half-made stays behind.
                    let _ = self.delete(&info.id).await;
                    return Err(e);
                }
            }
            let mut config = self.build_config(reference).unwrap_or_default();
            // `env` of the create is the Space's environment, over the image's.
            config.env.extend(opts.env.clone());
            if !config.is_default() {
                self.record_image_config(&info.id, &config)?;
                if let Ok(space) = self.space(&info.id).await {
                    space.set_image_config(config);
                }
            }
            report(Progress::phase(Phase::Ready));
            let info = self.find(&info.id).await?;
            Ok(SpaceCreated::Ready {
                info,
                reused: false,
            })
        };
        cua_sandbox_core::progress::carry(sink, create).await
    }

    /// Deletes the build `reference` names (a tag, a recipe key or its
    /// first 12 characters): its tags, its record, and the layer checkpoints
    /// no other build uses. Returns the checkpoints deleted.
    pub async fn delete_build(&self, reference: &str) -> Result<Vec<String>> {
        let home = self.home_dir().to_path_buf();
        let r = reference
            .strip_prefix(BUILD_IMAGE_PREFIX)
            .unwrap_or(reference);
        let records = store::load_records(&home);
        let tags = store::tags(&home);
        let key = tags.get(r).cloned().or_else(|| {
            records
                .iter()
                .find(|rec| rec.checkpoint == r || (r.len() >= 12 && rec.key.starts_with(r)))
                .map(|rec| rec.key.clone())
        });
        let Some(key) = key else {
            return Err(Error::NotFound(format!("build {reference}")));
        };
        // A tag shared by nothing else names this build alone; deleting the
        // build removes every tag of it.
        let rec = store::load_record(&home, &key)
            .ok_or_else(|| Error::NotFound(format!("build {reference}")))?;
        store::remove_tags_of(&home, &key)?;
        store::remove_record(&home, &key);
        let others: std::collections::HashSet<String> = store::load_records(&home)
            .into_iter()
            .flat_map(|o| o.layers)
            .collect();
        let mut deleted = Vec::new();
        for layer in rec.layers.iter().rev() {
            if others.contains(layer) {
                continue;
            }
            if self.sandboxes().checkpoint_record(layer).is_some() {
                self.drop_layer(layer).await;
                deleted.push(layer.clone());
            }
        }
        Ok(deleted)
    }
}

/// The create request of a builder Space. A build runs every step through
/// cua-spacesd, so the create always waits for it: an image that is not a
/// canonical reference (a local disk, a registry tag) would otherwise get one
/// short probe, and a guest still booting would read as "runs no cua-spacesd".
fn builder_create(spec: &BuilderSpec) -> SpaceCreate {
    let kind = match spec.kind.as_str() {
        "container" => Kind::Container,
        "microvm" => Kind::MicroVm,
        _ => Kind::Vm,
    };
    SpaceCreate {
        image: Some(spec.image.clone()),
        on: Some(On::Local),
        kind,
        runtime: Runtime::Auto,
        name: Some(spec.name.clone()),
        cpus: Some(spec.cpus),
        memory_mb: Some(spec.memory_mb),
        wait: Some(true),
        headless: spec.headless,
        forkable: true,
        spacesd: Some(true),
        ..Default::default()
    }
}

#[cfg(test)]
mod builder_create_tests {
    use super::*;

    #[test]
    fn the_builder_waits_for_spacesd_whatever_its_image() {
        let spec = BuilderSpec {
            image: "/tmp/a/local/disk.img".into(),
            kind: "vm".into(),
            name: "build-x".into(),
            cpus: 2,
            memory_mb: 2048,
            headless: true,
        };
        let c = builder_create(&spec);
        assert_eq!(c.spacesd, Some(true));
        assert!(crate::spaces::expects_spacesd(
            c.image.as_deref(),
            c.spacesd
        ));
    }
}
