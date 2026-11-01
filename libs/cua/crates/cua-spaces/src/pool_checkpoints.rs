//! Checkpoints of Spaces on any of your machines.
//!
//! A checkpoint is taken where its Space runs and stays there
//! ([`crate::snapshots`] copies it to another machine when a fork needs it
//! there). This module makes that one view from this machine:
//!
//! - [`Spaces::checkpoint_space`] checkpoints a Space wherever it runs:
//!   here (`Sandboxes::checkpoint_with`), or on the machine of yours that
//!   provides it (`HostSnapshotsService.CreateHostSnapshot`).
//! - [`Spaces::pool_checkpoints`] lists this machine's checkpoints, then
//!   those your machines that provide Spaces hold, each name once.
//! - [`Spaces::delete_checkpoint_everywhere`] deletes a checkpoint here and
//!   on every machine that holds a copy.
//! - [`Spaces::create`] with image `checkpoint:<name>` starts a Space from a
//!   checkpoint where `on` says ([`Spaces::create_from_checkpoint`]): by
//!   default where the checkpoint is, else on any machine of yours after a
//!   verified copy (the batched-fork planner, [`crate::fork_batch`], with
//!   one fork).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use cua_proto::daemon::v1 as dpb;
use cua_proto::env::v1 as pb;
use cua_sandbox_core::placement::{Kind, On};
use tokio::sync::mpsc;

use crate::fork_batch::{BatchSpec, THIS_MACHINE};
use crate::{Error, Result, SpaceCreate, SpaceCreated, SpaceId, Spaces};

/// The image prefix that starts a Space from a checkpoint
/// (`checkpoint:<name>`).
pub const CHECKPOINT_IMAGE: &str = "checkpoint:";

/// The checkpoint an image names (`checkpoint:<name>`), if it names one.
pub fn checkpoint_image(image: Option<&str>) -> Option<&str> {
    image
        .map(str::trim)
        .and_then(|i| i.strip_prefix(CHECKPOINT_IMAGE))
        .map(str::trim)
}

/// How long a pool listing is reused (creates from a checkpoint list it
/// first; a burst of them asks every machine once).
const POOL_TTL: Duration = Duration::from_secs(2);

/// How long each machine has to list its checkpoints.
const LIST_PROBE: Duration = Duration::from_secs(4);

/// A checkpoint and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolCheckpoint {
    /// What it is (name, source, time, engine, ...). Digest, ABI and size
    /// are set once it was exported (copied, or listed by another machine).
    pub snapshot: pb::HostSnapshot,
    /// `None`: this machine holds it. Else the machine of yours that does,
    /// by name.
    pub host: Option<String>,
    /// What `on = "host:<id>"` takes for that machine.
    pub host_id: Option<String>,
}

/// A pool listing per home, and when it was made.
type PoolCache = HashMap<PathBuf, (Instant, Vec<PoolCheckpoint>)>;

static POOL: LazyLock<Mutex<PoolCache>> = LazyLock::new(Default::default);

fn forget_pool(home: &std::path::Path) {
    POOL.lock().unwrap_or_else(|e| e.into_inner()).remove(home);
}

/// The kind of Space a checkpoint of engine `runtime_type` forks as.
fn kind_of_engine(runtime_type: &str) -> Option<Kind> {
    match runtime_type {
        "qemu" | "lume" => Some(Kind::Vm),
        "firecracker" | "vz" | "microvm" => Some(Kind::MicroVm),
        "gvisor" | "runc" | "docker" | "podman" | "container" | "apple" => Some(Kind::Container),
        _ => None,
    }
}

/// What a new checkpoint is, and where it was taken.
#[derive(Clone, Debug)]
pub enum SpaceCheckpoint {
    /// Taken on this machine.
    Local(Box<cua_sandbox_core::CheckpointRecord>),
    /// Taken on `host`, a machine of yours, which keeps it.
    Remote {
        /// The machine's name.
        host: String,
        /// The checkpoint.
        snapshot: pb::HostSnapshot,
    },
}

impl Spaces {
    /// The machine of yours that provides Space `id` (what
    /// [`Spaces::host_of`] takes), and the Space's id there (what its
    /// `HostSpacesService` and `HostSnapshotsService` take). `None` for a
    /// Space no machine of yours provides.
    async fn provider_of(&self, id: &SpaceId) -> Result<Option<(String, String)>> {
        Ok(match id {
            SpaceId::Relay { machine_id } => self
                .relay_row(machine_id)
                .await
                .and_then(|m| m.host.filter(|h| !h.is_empty()))
                .map(|host| (host, machine_id.clone())),
            SpaceId::Direct { .. } => self
                .registry()
                .direct_host_of(&id.to_string())?
                .map(|(host, on_host)| (host.name, on_host)),
            _ => None,
        })
    }

    /// Checkpoints Space `space` (an id, or a name the registry resolves)
    /// as `checkpoint`, where it runs: on this machine, or on the machine of
    /// yours that provides it, which keeps the checkpoint. It keeps running.
    /// `pin` keeps the checkpoint until it is unpinned.
    pub async fn checkpoint_space(
        &self,
        space: &str,
        checkpoint: &str,
        pin: bool,
    ) -> Result<SpaceCheckpoint> {
        let id = self.resolve(space)?;
        let refuse = |provider: &str| Error::WrongProvider {
            op: "Checkpointing".into(),
            provider: provider.into(),
        };
        let done = match &id {
            SpaceId::Local { name } => SpaceCheckpoint::Local(Box::new(
                self.sandboxes()
                    .checkpoint_with(name, checkpoint, pin)
                    .await?,
            )),
            SpaceId::Cloud { .. } => return Err(refuse("cloud")),
            other => {
                // A Space in your own cloud is a sandbox of this machine's.
                if let Some(sandbox) = self.cloud_sandbox_of(&other.to_string())? {
                    SpaceCheckpoint::Local(Box::new(
                        self.sandboxes()
                            .checkpoint_with(&sandbox, checkpoint, pin)
                            .await?,
                    ))
                } else {
                    let Some((host, on_host)) = self.provider_of(other).await? else {
                        // Only a Space on this machine, or one a machine of
                        // yours provides, has checkpoints.
                        return Err(refuse(match other {
                            SpaceId::Relay { .. } => "relay",
                            _ => "direct",
                        }));
                    };
                    let (host_space, _) = self.host_of(&host).await?;
                    let host_name = self
                        .resolve_host(&host)
                        .await
                        .map(|h| h.name())
                        .unwrap_or(host);
                    crate::host_spaces::guard_host_rpc(
                        &host_space,
                        &host_name,
                        "checkpoints of Spaces",
                    )?;
                    let snapshot = host_space
                        .spacesd()?
                        .host_snapshots()
                        .create_host_snapshot(pb::CreateHostSnapshotRequest {
                            space: on_host,
                            name: checkpoint.to_string(),
                            pin,
                        })
                        .await
                        .map_err(|e| {
                            crate::host_spaces::from_new_host_rpc(
                                e,
                                &host_name,
                                "checkpoints of Spaces",
                            )
                        })?
                        .into_inner()
                        .snapshot
                        .unwrap_or_else(|| pb::HostSnapshot {
                            name: checkpoint.to_string(),
                            ..Default::default()
                        });
                    SpaceCheckpoint::Remote {
                        host: host_name,
                        snapshot,
                    }
                }
            }
        };
        forget_pool(self.home_dir());
        Ok(done)
    }

    /// Every checkpoint you have: this machine's (oldest first), then those
    /// your machines that provide Spaces hold, each name once (a copy on
    /// several machines is listed at the first that answered). A machine
    /// that does not answer within a few seconds is left out. Reused for
    /// [`POOL_TTL`].
    pub async fn pool_checkpoints(&self) -> Result<Vec<PoolCheckpoint>> {
        let home = self.home_dir().to_path_buf();
        if let Some((at, list)) = POOL.lock().unwrap_or_else(|e| e.into_inner()).get(&home)
            && at.elapsed() < POOL_TTL
        {
            return Ok(list.clone());
        }
        let mut out: Vec<PoolCheckpoint> = self
            .sandboxes()
            .checkpoints()
            .iter()
            .map(|rec| PoolCheckpoint {
                snapshot: crate::snapshots::host_snapshot_of(rec),
                host: None,
                host_id: None,
            })
            .collect();
        let mut seen: BTreeSet<String> = out.iter().map(|c| c.snapshot.name.clone()).collect();
        // Boxed: the host listing's future is deep.
        let listing: std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Vec<crate::host_spaces::HostOffer>>>
                    + Send
                    + '_,
            >,
        > = Box::pin(self.hosts());
        let offers = listing.await.unwrap_or_default();
        let holding: Vec<_> = offers
            .into_iter()
            .filter(|h| {
                h.online
                    && h.node
                        .as_ref()
                        .is_some_and(|n| n.snapshots.keys().any(|k| !seen.contains(k)))
            })
            .collect();
        let lists = futures_util::future::join_all(holding.iter().map(|h| async move {
            let listed = tokio::time::timeout(LIST_PROBE, async {
                let (space, _) = self.host_of(&h.id).await?;
                crate::host_spaces::guard_host_rpc(&space, &h.name, "checkpoints")?;
                space
                    .spacesd()?
                    .host_snapshots()
                    .list_host_snapshots(pb::ListHostSnapshotsRequest {})
                    .await
                    .map(tonic::Response::into_inner)
                    .map_err(|e| crate::host_spaces::from_new_host_rpc(e, &h.name, "checkpoints"))
            })
            .await;
            match listed {
                Ok(Ok(l)) => Some(l.snapshots),
                Ok(Err(e)) => {
                    tracing::debug!(host = %h.name, "checkpoints not listed: {e}");
                    None
                }
                Err(_) => None,
            }
        }))
        .await;
        for (h, list) in holding.iter().zip(lists) {
            for s in list.unwrap_or_default() {
                if seen.insert(s.name.clone()) {
                    out.push(PoolCheckpoint {
                        snapshot: s,
                        host: Some(h.name.clone()),
                        host_id: Some(h.id.clone()),
                    });
                }
            }
        }
        POOL.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(home, (Instant::now(), out.clone()));
        Ok(out)
    }

    /// Where checkpoint `name` is: `Some(None)` on this machine,
    /// `Some(Some(host id))` on another machine of yours, `None` nowhere.
    pub async fn checkpoint_holder(&self, name: &str) -> Result<Option<Option<String>>> {
        if self.sandboxes().checkpoint_record(name).is_some() {
            return Ok(Some(None));
        }
        Ok(self
            .pool_checkpoints()
            .await?
            .into_iter()
            .find(|c| c.snapshot.name == name)
            .map(|c| c.host_id))
    }

    /// Deletes checkpoint `name` here and on every machine of yours that
    /// holds it (forks made from it keep running). Returns the machines it
    /// was deleted on (`local` for this one). [`Error::NotFound`] when no
    /// machine holds it; a pinned checkpoint is refused until it is
    /// unpinned. A machine that refuses fails the call after the others
    /// were tried.
    pub async fn delete_checkpoint_everywhere(&self, name: &str) -> Result<Vec<String>> {
        let mut deleted = Vec::new();
        let mut failed: Option<Error> = None;
        if self.sandboxes().checkpoint_record(name).is_some() {
            match self.sandboxes().delete_checkpoint(name).await {
                Ok(()) => deleted.push(THIS_MACHINE.to_string()),
                // Pinned, or still the source of a fork that is starting:
                // nothing elsewhere is deleted either.
                Err(e) => return Err(e.into()),
            }
        }
        forget_pool(self.home_dir());
        let listing: std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<Vec<crate::host_spaces::HostOffer>>>
                    + Send
                    + '_,
            >,
        > = Box::pin(self.hosts());
        let holders: Vec<_> = listing
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|h| {
                h.online
                    && h.node
                        .as_ref()
                        .is_some_and(|n| n.snapshots.contains_key(name))
            })
            .collect();
        for h in holders {
            let r = async {
                let (space, _) = self.host_of(&h.id).await?;
                crate::host_spaces::guard_host_rpc(&space, &h.name, "checkpoints")?;
                space
                    .spacesd()?
                    .host_snapshots()
                    .delete_host_snapshot(pb::DeleteHostSnapshotRequest {
                        name: name.to_string(),
                    })
                    .await
                    .map_err(|e| crate::host_spaces::from_new_host_rpc(e, &h.name, "checkpoints"))
            }
            .await;
            match r {
                Ok(_) => deleted.push(h.name.clone()),
                // Gone there meanwhile.
                Err(Error::NotFound(_)) => {}
                Err(e) => {
                    tracing::warn!(host = %h.name, checkpoint = name, "not deleted there: {e}");
                    failed.get_or_insert(e);
                }
            }
        }
        forget_pool(self.home_dir());
        if let Some(e) = failed {
            return Err(e);
        }
        if deleted.is_empty() {
            return Err(Error::NotFound(format!("checkpoint {name}")));
        }
        Ok(deleted)
    }

    /// [`Spaces::create`] of image `checkpoint:<name>`: a fork of the
    /// checkpoint, started where `opts.on` says. Unset: where the
    /// checkpoint is (this machine when it holds it, else a machine of
    /// yours that does). `local`, `host:<machine>` or `auto` (the
    /// least-loaded of your machines): a machine that lacks the checkpoint
    /// gets a verified copy first. The fork keeps the checkpoint's disk,
    /// memory and processes, so it takes no command, environment, services,
    /// mounts, GPU or disk size; its kind is the checkpoint's. It returns
    /// once the fork answers (`wait` does not apply).
    pub async fn create_from_checkpoint(&self, opts: SpaceCreate) -> Result<SpaceCreated> {
        let checkpoint = checkpoint_image(opts.image.as_deref())
            .unwrap_or_default()
            .to_string();
        if checkpoint.is_empty() {
            return Err(Error::invalid(
                "image checkpoint:<name> needs the checkpoint's name",
            ));
        }
        let mut refused = Vec::new();
        if opts.command.is_some() {
            refused.push("command");
        }
        if !opts.env.is_empty() {
            refused.push("environment variables");
        }
        if !opts.services.is_empty() {
            refused.push("services");
        }
        if !opts.mounts.is_empty() {
            refused.push("mounts");
        }
        if opts.gpu.is_some() {
            refused.push("GPU");
        }
        if opts.disk_gb.is_some_and(|g| g > 0) {
            refused.push("disk size");
        }
        if opts.reuse {
            refused.push("reuse");
        }
        if opts.durable {
            refused.push("durable");
        }
        if opts.forkable {
            refused.push("forkable");
        }
        // Never dropped silently: a fork from a checkpoint cannot take a
        // network or egress setting here, so it refuses one (fail closed).
        if opts.network != cua_sandbox_core::NetworkMode::Default {
            refused.push("network setting");
        }
        if !opts.egress.is_empty() {
            refused.push("egress policy");
        }
        if !opts.volumes.is_empty() {
            refused.push("volumes");
        }
        if !opts.labels.is_empty() {
            refused.push("labels");
        }
        if !refused.is_empty() {
            return Err(Error::invalid(format!(
                "a Space started from checkpoint {checkpoint} keeps its disk, memory and \
                 processes, so it takes no {}",
                refused.join(", ")
            )));
        }
        let local = self.sandboxes().checkpoint_record(&checkpoint);
        let listed = if local.is_some() {
            None
        } else {
            self.pool_checkpoints()
                .await?
                .into_iter()
                .find(|c| c.snapshot.name == checkpoint)
        };
        let engine = match (&local, &listed) {
            (Some(rec), _) => rec.runtime_type.clone(),
            (None, Some(c)) => c.snapshot.runtime_type.clone(),
            // Only in a fallback store, or nowhere: the batch says which.
            (None, None) => String::new(),
        };
        if opts.kind != Kind::Auto
            && let Some(k) = kind_of_engine(&engine)
            && k != opts.kind
        {
            return Err(Error::invalid(format!(
                "checkpoint {checkpoint} is a {} checkpoint, so its forks are {} Spaces, not {}",
                engine,
                k.as_str(),
                opts.kind.as_str()
            )));
        }
        let on = fork_batch_on(
            opts.on.as_ref(),
            &checkpoint,
            local.is_some(),
            listed
                .as_ref()
                .map(|c| c.host_id.clone().unwrap_or_default()),
        )?;
        let name = opts
            .name
            .clone()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("{checkpoint}-{:06x}", rand::random::<u32>() & 0xff_ffff));
        let spec = BatchSpec {
            groups: vec![dpb::ForkBatchGroup {
                source: checkpoint.clone(),
                count: 1,
                name_prefix: name,
            }],
            on,
            max_parallel_per_host: 1,
            cpus: opts.cpus,
            memory_mb: opts.memory_mb,
            retries: 1,
            exact_names: true,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel::<dpb::ForkSandboxBatchResponse>(64);
        let collect = async move {
            let mut item = None;
            while let Some(ev) = rx.recv().await {
                if let Some(dpb::fork_sandbox_batch_response::Event::Item(i)) = ev.event {
                    item = Some(i);
                }
            }
            item
        };
        let (ran, item) = tokio::join!(Box::pin(self.fork_batch(spec, tx)), collect);
        ran?;
        let item = item.ok_or_else(|| {
            Error::host(
                THIS_MACHINE,
                format!("the fork of checkpoint {checkpoint} ended without a result"),
            )
        })?;
        if !item.error.is_empty() {
            return Err(match item.error_code.as_str() {
                "no_host" | "no_capacity" => Error::LimitExceeded(item.error),
                _ if item.error.contains("no machine of yours holds it") => Error::NotFound(
                    format!("checkpoint {checkpoint} (no machine of yours holds it)"),
                ),
                _ => Error::host(
                    if item.host.is_empty() {
                        THIS_MACHINE
                    } else {
                        item.host.as_str()
                    },
                    format!("fork of checkpoint {checkpoint}: {}", item.error),
                ),
            });
        }
        let info = self.info(&SpaceId::parse(&item.space)?)?;
        Ok(SpaceCreated::Ready {
            info,
            reused: false,
        })
    }
}

/// Where the fork of `checkpoint` goes (`BatchSpec::on`): with no location,
/// this machine when it holds the checkpoint (`""`: it overflows into your
/// linked clouds when this machine is full and a budget is set, like a
/// create), else the machine that holds it; `auto` the least loaded of
/// your machines (growing the pool).
pub(crate) fn fork_batch_on(
    on: Option<&On>,
    checkpoint: &str,
    here: bool,
    holder: Option<String>,
) -> Result<String> {
    Ok(match on {
        None => match (here, holder) {
            (true, _) => String::new(),
            (false, Some(h)) => format!("host:{h}"),
            (false, None) => {
                return Err(Error::NotFound(format!(
                    "checkpoint {checkpoint} (no machine of yours holds it)"
                )));
            }
        },
        Some(On::Local) => "local".into(),
        Some(On::Auto) => "auto".into(),
        Some(On::Host(q)) => format!("host:{q}"),
        Some(other) => {
            return Err(Error::invalid(format!(
                "a checkpoint starts on this machine (local), one of your machines \
                 (host:<machine>) or the least loaded of them (auto), not {}",
                other.location()
            )));
        }
    })
}

#[cfg(test)]
mod fork_on_tests {
    use super::*;

    /// A fork of a checkpoint goes through the same placement as a create:
    /// no location means this machine with the overflow default, `auto`
    /// the pool (which grows), and a pin stays a pin.
    #[test]
    fn a_checkpoint_fork_is_placed_like_a_create() {
        assert_eq!(fork_batch_on(None, "w", true, None).unwrap(), "");
        assert_eq!(
            fork_batch_on(None, "w", false, Some("kvm-a".into())).unwrap(),
            "host:kvm-a"
        );
        assert!(matches!(
            fork_batch_on(None, "w", false, None),
            Err(Error::NotFound(_))
        ));
        assert_eq!(
            fork_batch_on(Some(&On::Auto), "w", true, None).unwrap(),
            "auto"
        );
        assert_eq!(
            fork_batch_on(Some(&On::Local), "w", true, None).unwrap(),
            "local"
        );
        // The batch rule: "" overflows when allowed, "auto" grows, "local"
        // never does.
        assert_eq!(crate::fork_batch::batch_growth("", true), Some(true));
        assert_eq!(crate::fork_batch::batch_growth("auto", false), Some(false));
        assert_eq!(crate::fork_batch::batch_growth("local", true), None);
    }
}
