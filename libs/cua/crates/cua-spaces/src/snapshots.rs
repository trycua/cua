//! Checkpoints across your machines (snapshot sync).
//!
//! A checkpoint lives on the machine that took it. Forking it on another
//! machine of the pool needs a copy there first:
//!
//! - [`Spaces::export_snapshot`] describes a local checkpoint as a
//!   manifest of files by SHA-256 ([`cua_snapshot_sync`]) and keeps an
//!   index of where they are, so the host's cua-spacesd serves them to
//!   other hosts that hold a read grant ([`Spaces::snapshot_grant`]).
//! - [`Spaces::pull_snapshot`] fetches the files this machine lacks from
//!   the hosts that have them, then from the fallback (S3, or a folder
//!   every host mounts such as the Cua Volume), checks every file, lays
//!   them out for the runtime and adds the checkpoint under its name.
//! - [`Spaces::publish_snapshot`] uploads a checkpoint to the fallback.
//!
//! `HostSnapshotsService` (`cua.env.v1`) serves these to the owner's other
//! machines ([`crate::host_spaces::HostSpacesServer`]).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use cua_proto::env::v1 as pb;
use cua_snapshot_sync as sync;
use cua_snapshot_sync::store::Store;

use crate::spaces::Spaces;
use crate::{Error, Result};

/// One export or pull per checkpoint name at a time on this machine.
static LOCKS: LazyLock<std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

fn lock_of(name: &str) -> Arc<tokio::sync::Mutex<()>> {
    LOCKS
        .lock()
        .unwrap()
        .entry(name.to_string())
        .or_default()
        .clone()
}

fn sync_err(e: sync::Error) -> Error {
    match e {
        sync::Error::NotFound(m) => Error::NotFound(m),
        sync::Error::Invalid(m) => Error::InvalidArgument(m),
        sync::Error::Io(e) => Error::Io(e),
        sync::Error::Json(e) => Error::Json(e),
        // Refused, unavailable or corrupt: the copy failed (with why).
        other => Error::Transfer(format!("snapshot copy: {other}")),
    }
}

/// The fallback store a request names, if any.
pub async fn fallback_store(
    fb: Option<&pb::SnapshotFallback>,
) -> Result<Option<sync::fallback::FallbackStore>> {
    let Some(fb) = fb else { return Ok(None) };
    if let Some(s3) = fb.s3.as_ref().filter(|s| !s.bucket.trim().is_empty()) {
        return sync::fallback::FallbackStore::s3(
            &sync::fallback::S3Location {
                bucket: s3.bucket.clone(),
                region: s3.region.clone(),
                endpoint: s3.endpoint.clone(),
            },
            &s3.prefix,
        )
        .await
        .map(Some)
        .map_err(sync_err);
    }
    if !fb.dir.trim().is_empty() {
        return Ok(Some(sync::fallback::FallbackStore::dir(fb.dir.trim())));
    }
    Ok(None)
}

/// A local checkpoint as a host reports it (`HostSnapshot`), without its
/// digest, ABI and size (those come from its manifest, once exported).
pub fn host_snapshot_of(rec: &cua_sandbox_core::CheckpointRecord) -> pb::HostSnapshot {
    let created = rec.created_at.replace("+00:00", "Z");
    pb::HostSnapshot {
        name: rec.name.clone(),
        source: rec.source.clone(),
        created_at_ms: humantime::parse_rfc3339_weak(&created)
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        pinned: rec.pinned,
        shared_memory: rec.shared_memory,
        runtime_type: rec.runtime_type.clone(),
        image: rec
            .template
            .image
            .get("registry")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        ..Default::default()
    }
}

/// Whether a fallback is set at all.
pub fn has_fallback(fb: Option<&pb::SnapshotFallback>) -> bool {
    fb.is_some_and(|f| {
        f.s3.as_ref().is_some_and(|s| !s.bucket.trim().is_empty()) || !f.dir.trim().is_empty()
    })
}

impl Spaces {
    /// This machine's snapshot directory.
    pub fn snapshot_store(&self) -> Store {
        Store::of_home(self.home_dir())
    }

    /// The digest of local checkpoint `rec` when it was exported or copied
    /// here as it is now (a checkpoint taken again under the same name is
    /// another snapshot).
    fn current_digest(&self, rec: &cua_sandbox_core::CheckpointRecord) -> Option<String> {
        let store = self.snapshot_store();
        let d = store.digest_of(&rec.name)?;
        let m = store.manifest(&d).ok()?;
        same_take(&m, rec).then_some(d)
    }

    /// Local checkpoints (name to digest, empty when never exported) and
    /// the checkpoint ABIs this machine restores.
    pub async fn local_snapshots(&self) -> (BTreeMap<String, String>, Vec<String>) {
        let sbx = self.sandboxes();
        let names = sbx
            .checkpoints()
            .into_iter()
            .map(|rec| {
                let d = self.current_digest(&rec).unwrap_or_default();
                (rec.name, d)
            })
            .collect();
        (names, sbx.snapshot_abis().await)
    }

    /// `ListHostSnapshots`.
    pub async fn list_snapshots(&self) -> pb::ListHostSnapshotsResponse {
        let store = self.snapshot_store();
        let (names, abis) = self.local_snapshots().await;
        let recs: BTreeMap<String, cua_sandbox_core::CheckpointRecord> = self
            .sandboxes()
            .checkpoints()
            .into_iter()
            .map(|r| (r.name.clone(), r))
            .collect();
        pb::ListHostSnapshotsResponse {
            snapshots: names
                .into_iter()
                .map(|(name, digest)| {
                    let m = (!digest.is_empty())
                        .then(|| store.manifest(&digest).ok())
                        .flatten();
                    let mut s = match recs.get(&name) {
                        Some(r) => host_snapshot_of(r),
                        None => pb::HostSnapshot {
                            name,
                            ..Default::default()
                        },
                    };
                    s.abi = m.as_ref().map(|m| m.abi.clone()).unwrap_or_default();
                    s.size_bytes = m.as_ref().map(|m| m.size()).unwrap_or(0);
                    s.digest = digest;
                    s
                })
                .collect(),
            abis,
        }
    }

    /// Describes local checkpoint `name` as a manifest (hashing its files
    /// the first time; a checkpoint exported before and unchanged is not
    /// read again) and keeps the index the host's cua-spacesd serves its
    /// files from. Returns the manifest and its digest.
    pub async fn export_snapshot(&self, name: &str) -> Result<(sync::Manifest, String)> {
        let lock = lock_of(name);
        let _g = lock.lock().await;
        let store = self.snapshot_store();
        let rec = self
            .sandboxes()
            .checkpoint_record(name)
            .ok_or_else(|| Error::NotFound(format!("checkpoint {name}")))?;
        if let Some(d) = self.current_digest(&rec)
            && let Ok(m) = store.manifest(&d)
            && let Ok(idx) = store.export_index(&d)
            && idx
                .files
                .values()
                .all(|e| sync::hash::Stamp::of(&e.path).is_ok_and(|s| s == e.stamp))
        {
            return Ok((m, d));
        }
        let (rec, ex) = match self.sandboxes().export_checkpoint(name).await {
            Ok(x) => x,
            Err(e) => return Err(export_refusal(e)),
        };
        let mut by_path: BTreeMap<PathBuf, String> = BTreeMap::new();
        let mut dirs = Vec::new();
        for (i, d) in ex.dirs.iter().enumerate() {
            let id = format!("d{i}");
            by_path.insert(d.clone(), id.clone());
            dirs.push(id);
        }
        for (i, f) in ex.files.iter().enumerate() {
            by_path.insert(f.path.clone(), format!("f{i}"));
        }
        let mut vmm = ex.state.clone();
        sync::store::insert_placeholders(&mut vmm, &by_path);
        let files = ex
            .files
            .iter()
            .map(|f| sync::FileSpec {
                id: by_path[&f.path].clone(),
                path: f.path.clone(),
                dir: f.dir.as_ref().map(|d| by_path[d].clone()),
                qcow2_backing: f.qcow2_backing.as_ref().map(|b| by_path[b].clone()),
            })
            .collect();
        store
            .export(
                name,
                &rec.runtime_type,
                &ex.abi,
                vmm,
                serde_json::to_value(&rec)?,
                dirs,
                files,
            )
            .await
            .map_err(sync_err)
    }

    /// A grant to read the files of exported manifest `digest` from this
    /// machine for `ttl_secs` (0: an hour).
    pub fn snapshot_grant(&self, digest: &str, ttl_secs: u64) -> Result<String> {
        let store = self.snapshot_store();
        store.export_index(digest).map_err(sync_err)?;
        let key = store.grant_key().map_err(sync_err)?;
        Ok(sync::grant::issue(
            &key,
            digest,
            ttl_secs,
            sync::grant::now(),
        ))
    }

    /// Copies a checkpoint here: the files of `manifest` (or, without one,
    /// of the snapshot `name` published to the fallback) this machine does
    /// not have, from `sources` in order and then the fallback, each
    /// verified; then it becomes local checkpoint `name`. A checkpoint of
    /// that name that is the same snapshot is kept as it is.
    pub async fn pull_snapshot(
        &self,
        manifest: Option<sync::Manifest>,
        name: &str,
        sources: &[pb::SnapshotSource],
        fallback: Option<&pb::SnapshotFallback>,
        progress: Option<sync::ProgressFn>,
    ) -> Result<pb::HostSnapshot> {
        let fb = fallback_store(fallback).await?;
        let m = match manifest {
            Some(m) => m,
            None => match &fb {
                Some(f) => f.manifest_by_name(name).await.map_err(sync_err)?,
                None => {
                    return Err(Error::invalid(
                        "pass the snapshot's manifest, or a fallback that has it",
                    ));
                }
            },
        };
        m.validate().map_err(sync_err)?;
        let digest = m.digest();
        let lock = lock_of(&m.name);
        let _g = lock.lock().await;
        let store = self.snapshot_store();
        let done = |reused: u64| pb::HostSnapshot {
            name: m.name.clone(),
            digest: digest.clone(),
            abi: m.abi.clone(),
            size_bytes: reused,
            ..Default::default()
        };
        if let Some(rec) = self.sandboxes().checkpoint_record(&m.name) {
            let mine = self.current_digest(&rec);
            if mine.as_deref() == Some(digest.as_str()) || same_take(&m, &rec) {
                if let Some(p) = &progress {
                    p(sync::Progress {
                        name: m.name.clone(),
                        digest: digest.clone(),
                        bytes_reused: m.size(),
                        done: true,
                        verified: true,
                        ..Default::default()
                    });
                }
                return Ok(done(m.size()));
            }
            return Err(Error::invalid(format!(
                "this machine has another checkpoint named {}; delete it first",
                m.name
            )));
        }
        let abis = self.sandboxes().snapshot_abis().await;
        if !abis.contains(&m.abi) {
            return Err(Error::host(
                "snapshot copy",
                format!(
                    "{} is a {} checkpoint; this machine restores {}",
                    m.name,
                    m.abi,
                    if abis.is_empty() {
                        "none".to_string()
                    } else {
                        abis.join(", ")
                    }
                ),
            ));
        }
        let mut srcs: Vec<Arc<dyn sync::pull::BlobSource>> = Vec::new();
        for s in sources {
            srcs.push(Arc::new(
                sync::pull::HttpSource::new(
                    &s.url,
                    &s.grant,
                    if s.name.is_empty() { &s.url } else { &s.name },
                )
                .map_err(sync_err)?,
            ));
        }
        if let Some(f) = fb {
            srcs.push(Arc::new(f));
        }
        sync::pull::pull(&store, &m, srcs, progress)
            .await
            .map_err(sync_err)?;
        let dest = self
            .sandboxes()
            .import_dir(&m.runtime_type, &digest)
            .await
            .ok_or_else(|| {
                Error::host(
                    "snapshot copy",
                    format!("this machine cannot take {} checkpoints", m.runtime_type),
                )
            })?;
        let paths = store.materialize(&m, &dest).map_err(sync_err)?;
        let mut state = m.vmm.clone();
        sync::store::resolve_placeholders(&mut state, &paths).map_err(sync_err)?;
        let rebase = m
            .files
            .iter()
            .filter_map(|f| {
                let b = f.qcow2_backing.as_ref()?;
                Some((paths[&f.id].clone(), paths[b].clone()))
            })
            .collect();
        let rec: cua_sandbox_core::CheckpointRecord = serde_json::from_value(m.record.clone())?;
        self.sandboxes()
            .import_checkpoint(&m.name, rec, state, rebase)
            .await?;
        store.save_manifest(&m).map_err(sync_err)?;
        store.set_name(&m.name, &digest).map_err(sync_err)?;
        Ok(done(0))
    }

    /// Uploads local checkpoint `name` to the fallback (exporting it
    /// first). Returns the bytes uploaded.
    pub async fn publish_snapshot(
        &self,
        name: &str,
        fallback: &pb::SnapshotFallback,
        progress: Option<sync::ProgressFn>,
    ) -> Result<u64> {
        let fb = fallback_store(Some(fallback))
            .await?
            .ok_or_else(|| Error::invalid("publishing needs a fallback (s3 or dir)"))?;
        let (m, _) = self.export_snapshot(name).await?;
        fb.publish(&self.snapshot_store(), &m, progress)
            .await
            .map_err(sync_err)
    }
}

/// An export the engine cannot do (a container has no files to copy) as the
/// plain refusal a user can act on, not an internal error.
fn export_refusal(e: cua_sandbox_core::Error) -> Error {
    use cua_sandbox_core::{Error as S, RuntimeError as R};
    match e {
        S::Runtime(R::Unsupported { backend, op }) if op == "export_checkpoint" => {
            let what = if matches!(backend.as_str(), "container" | "gvisor" | "runc" | "docker") {
                "a container".to_string()
            } else {
                format!("a {backend} Space")
            };
            Error::Unsupported(format!(
                "publishing a checkpoint is only supported for VM Spaces; this is {what}"
            ))
        }
        other => other.into(),
    }
}

/// Whether manifest `m` is of checkpoint `rec` as it is now: the same
/// source and the same time taken (a checkpoint taken again under the same
/// name is another snapshot; a copy keeps both).
fn same_take(m: &sync::Manifest, rec: &cua_sandbox_core::CheckpointRecord) -> bool {
    m.record.get("created_at").and_then(|v| v.as_str()) == Some(rec.created_at.as_str())
        && m.record.get("source").and_then(|v| v.as_str()) == Some(rec.source.as_str())
}

/// `SnapshotSyncProgress` of a progress report.
pub fn progress_to_pb(p: &sync::Progress) -> pb::SnapshotSyncProgress {
    pb::SnapshotSyncProgress {
        name: p.name.clone(),
        digest: p.digest.clone(),
        from: p.from.clone(),
        bytes_done: p.bytes_done,
        bytes_total: p.bytes_total,
        bytes_reused: p.bytes_reused,
        done: p.done,
        verified: p.verified,
    }
}

#[cfg(test)]
mod export_refusal_tests {
    use super::*;

    #[test]
    fn a_container_checkpoint_cannot_be_published_and_says_so() {
        let e = export_refusal(cua_sandbox_core::Error::Runtime(
            cua_sandbox_core::RuntimeError::Unsupported {
                backend: "container".into(),
                op: "export_checkpoint".into(),
            },
        ));
        assert_eq!(e.tag(), "wrong_provider");
        assert_eq!(
            e.to_string(),
            "publishing a checkpoint is only supported for VM Spaces; this is a container"
        );
        // Anything else keeps its own kind.
        let other = export_refusal(cua_sandbox_core::Error::NotFound("x".into()));
        assert_eq!(other.tag(), "not_found");
    }

    #[test]
    fn the_refusal_reaches_the_wire_as_unimplemented() {
        let e = Error::Unsupported(
            "publishing a checkpoint is only supported for VM Spaces; this is a container".into(),
        );
        let st = crate::host_spaces::to_status(&e);
        assert_eq!(st.code(), tonic::Code::Unimplemented);
    }
}
