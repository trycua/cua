//! Snapshots: filesystem snapshots (images), directory snapshots (images
//! you mount), memory snapshots (restorable sandboxes) and the exit
//! snapshot.
//!
//! All are Cua checkpoints (full-machine snapshots, copy-on-write where the
//! runtime supports it). A filesystem snapshot is started from by booting the
//! snapshot's disk afresh (processes do not carry over, as with Modal's
//! filesystem snapshots); a memory snapshot is restored with the machine
//! as it was, processes included.

use crate::modal::db::now_s;
use crate::modal::image::{Built, ImageRec, Spec, Status};
use crate::modal::pb::api;
use crate::modal::sandbox::{SandboxRec, SnapshotRec};
use crate::modal::{Caller, Modal, errors, ids};
use base64::Engine;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tonic::Status as TStatus;

impl Modal {
    /// Runs a short command as root in the sandbox; its output.
    pub async fn run_root(
        &self,
        rec: &SandboxRec,
        script: &str,
        args: &[&str],
    ) -> Result<cua_spacesd_client::Output, TStatus> {
        let client = self.spacesd(rec).await.map_err(errors::from_backend)?;
        let (driver, _) = self.guest_of(rec, &client).await;
        crate::guest::run_as(&client, &driver, "root", script, args)
            .await
            .map_err(|e| errors::conflict(format!("could not run a command in the sandbox: {e}")))
    }

    async fn checkpoint_of(&self, rec: &SandboxRec, cp: &str) -> Result<(), TStatus> {
        let name = rec
            .cua_name
            .as_deref()
            .ok_or_else(|| errors::conflict("the sandbox is not running"))?;
        // Flush the guest's file systems so the disk state is consistent.
        let _ = self.run_root(rec, "sync", &[]).await;
        self.checkpoint_retry(&rec.identity(), name, cp)
            .await
            .map_err(errors::from_backend)
    }

    /// A checkpoint, retried: the runtime can refuse one while the guest's
    /// disk is busy (a QEMU migration that fails to inactivate a block
    /// device), and succeeds a moment later.
    pub async fn checkpoint_retry(
        &self,
        who: &crate::backend::Identity,
        name: &str,
        cp: &str,
    ) -> Result<(), crate::backend::BackendError> {
        let mut last = None;
        for attempt in 0..4 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let _ = self.backend.delete_checkpoint(who, cp).await;
            }
            match self.backend.checkpoint(who, name, cp).await {
                Ok(()) => return Ok(()),
                Err(e @ crate::backend::BackendError::NotFound(_)) => return Err(e),
                Err(e) => last = Some(e),
            }
        }
        Err(last.expect("an attempt was made"))
    }

    /// The configuration an image made from this sandbox's inherits.
    fn inherited(&self, rec: &SandboxRec) -> Built {
        self.image(&rec.image_id)
            .and_then(|i| i.built)
            .unwrap_or_default()
    }

    fn register_image(&self, key_id: &str, built: Built, ttl: Option<f64>, what: &str) -> String {
        let id = ids::new_id("im");
        let spec = Spec {
            commands: vec![format!("# {what}")],
            ..Default::default()
        };
        let rec = ImageRec {
            id: id.clone(),
            key_id: key_id.to_string(),
            created_at: now_s(),
            hash: format!("snapshot:{id}"),
            spec,
            status: Status::Built,
            built: Some(built),
            tags: Default::default(),
            expires_at: ttl.map(|t| now_s() + t),
        };
        self.db.write(|d| d.images.insert(id.clone(), rec));
        id
    }

    /// `snapshot_filesystem`: the disk as an image.
    pub async fn snapshot_filesystem(&self, rec: &SandboxRec) -> Result<String, TStatus> {
        let cp = format!("modal-fs-{}", ids::new_id("s").to_ascii_lowercase());
        self.checkpoint_of(rec, &cp).await?;
        let mut built = self.inherited(rec);
        built.checkpoint = Some(cp);
        built.image = None;
        // A Cua checkpoint carries memory with the disk: a Sandbox made from it
        // continues from the snapshotted machine (a cold restart would drop the
        // disk on this runtime's root filesystem; verified on a Mac test host).
        built.reboot = false;
        Ok(self.register_image(&rec.key_id, built, None, "filesystem snapshot"))
    }

    /// `ttl` of the snapshot calls: seconds, `-1` forever.
    pub fn set_image_ttl(&self, image_id: &str, ttl: Option<i64>) {
        if let Some(t) = ttl.filter(|t| *t > 0) {
            self.db.write(|d| {
                if let Some(i) = d.images.get_mut(image_id) {
                    i.expires_at = Some(now_s() + t as f64);
                }
            });
        }
    }

    /// `snapshot_directory`: a tar of the directory, as an image.
    pub async fn snapshot_directory(
        &self,
        rec: &SandboxRec,
        path: &str,
    ) -> Result<String, TStatus> {
        if !path.starts_with('/') {
            return Err(errors::invalid(format!(
                "Snapshot path must be absolute; got: {path}"
            )));
        }
        let client = self.spacesd(rec).await.map_err(errors::from_backend)?;
        let (driver, _) = self.guest_of(rec, &client).await;
        let (program, args, user, cwd) = crate::guest::as_user_in(
            &driver,
            "root",
            "tar".into(),
            vec![
                "-C".into(),
                path.into(),
                "-cf".into(),
                "-".into(),
                ".".into(),
            ],
            "/".into(),
        );
        let mut cmd = cua_spacesd_client::Command::new(program)
            .args(args)
            .timeout(Duration::from_secs(600));
        if !user.is_empty() {
            cmd = cmd.user(user);
        }
        if !cwd.is_empty() {
            cmd = cmd.cwd(cwd);
        }
        let out = client
            .run(cmd)
            .await
            .map_err(|e| errors::conflict(format!("could not read {path}: {e}")))?;
        if !out.success() {
            let msg = out.stderr_str();
            return Err(
                if msg.contains("No such file") || msg.contains("Cannot chdir") {
                    errors::not_found(format!("{path}: no such directory in the sandbox"))
                } else {
                    errors::conflict(format!("could not snapshot {path}: {}", msg.trim()))
                },
            );
        }
        let sha = self
            .blobs
            .put_sha(None, &out.stdout)
            .map_err(errors::internal)?;
        let built = Built {
            dir_tar: Some(sha),
            scratch: true,
            ..Default::default()
        };
        Ok(self.register_image(&rec.key_id, built, None, "directory snapshot"))
    }

    /// `mount_image`: an image's directory snapshot (or an empty image)
    /// mounted over `path`, hiding what is there until `unmount_image`.
    pub async fn mount_image(
        &self,
        rec: &SandboxRec,
        path: &str,
        image_id: &str,
    ) -> Result<(), TStatus> {
        if !path.starts_with('/') || path == "/" {
            return Err(errors::invalid(format!(
                "Mount path must be an absolute path other than /; got: {path}"
            )));
        }
        let (tar_sha, empty) = if image_id.is_empty() {
            (None, true)
        } else {
            let img = self
                .image(image_id)
                .filter(|i| i.key_id == rec.key_id)
                .ok_or_else(|| errors::not_found(format!("Image {image_id} not found")))?;
            let b = img.built.clone().unwrap_or_default();
            match (&b.dir_tar, b.scratch) {
                (Some(sha), _) => (Some(sha.clone()), false),
                (None, true) => (None, true),
                _ => {
                    return Err(errors::unsupported(
                        "mounting a filesystem image",
                        "mount_image takes a directory snapshot (snapshot_directory) or an empty image (Image.from_scratch); a whole-filesystem image is a machine, which sandboxes are started from",
                    ));
                }
            }
        };
        let client = self.spacesd(rec).await.map_err(errors::from_backend)?;
        let (driver, _) = self.guest_of(rec, &client).await;
        let stage = format!("/__modal/mounts/{}", ids::new_id("m"));
        run_script(&client, &driver, "mkdir -p -- \"$1\"", &[&stage]).await?;
        if let Some(sha) = tar_sha {
            let data = self.blobs.get_sha(&sha).ok_or_else(|| {
                errors::not_found("the directory snapshot is missing from the gateway's store")
            })?;
            let tmp = format!("{stage}.tar");
            self.put_file_as_root(&client, &driver, &tmp, data, None)
                .await
                .map_err(|e| errors::conflict(format!("could not upload the snapshot: {e}")))?;
            run_script(
                &client,
                &driver,
                "tar -C \"$1\" -xf \"$2\" && rm -f -- \"$2\"",
                &[&stage, &tmp],
            )
            .await?;
        }
        let _ = empty;
        // A bind mount hides what the sandbox has at `path` and
        // `unmount_image` shows it again. Where the Space cannot mount (a
        // container Space), the directory is swapped for a read-only copy,
        // and the original kept beside it until `unmount_image`.
        run_script(
            &client,
            &driver,
            "mkdir -p -- \"$2\"; \
             if mount --bind -- \"$1\" \"$2\" 2>/dev/null; then exit 0; fi; \
             set -e; mv -- \"$2\" \"$2.cua-under\"; cp -a -- \"$1\" \"$2\"; chmod -R a-w -- \"$2\"; \
             printf '%s\\n' \"$2\" > \"$1.copy\"",
            &[&stage, path],
        )
        .await
        .map_err(|e| {
            errors::conflict(format!(
                "could not mount the image at {path}: {}",
                e.message()
            ))
        })?;
        Ok(())
    }

    /// `unmount_image`: shows what was under the mount again.
    pub async fn unmount_image(&self, rec: &SandboxRec, path: &str) -> Result<(), TStatus> {
        if !path.starts_with('/') {
            return Err(errors::invalid(format!(
                "Unmount path must be absolute; got: {path}"
            )));
        }
        let client = self.spacesd(rec).await.map_err(errors::from_backend)?;
        let (driver, _) = self.guest_of(rec, &client).await;
        // Only a mount `mount_image` made (a path under /__modal/mounts).
        let out = crate::guest::run_as(
            &client,
            &driver,
            "root",
            "m=$(grep -lsx -- \"$1\" /__modal/mounts/*.copy 2>/dev/null | head -1); \
             if [ -n \"$m\" ]; then chmod -R u+w -- \"$1\"; rm -rf -- \"$1\"; \
               [ -e \"$1.cua-under\" ] && mv -- \"$1.cua-under\" \"$1\"; rm -f -- \"$m\"; exit 0; fi; \
             grep -qs \" $1 \" /proc/self/mountinfo || { echo \"not a mount point\" >&2; exit 22; }; \
             umount -- \"$1\"",
            &[path],
        )
        .await
        .map_err(errors::conflict)?;
        if out.status.code == Some(22) {
            return Err(errors::invalid(format!("{path} is not a mounted image")));
        }
        if !out.success() {
            return Err(errors::conflict(format!(
                "could not unmount {path}: {}",
                out.stderr_str().trim()
            )));
        }
        Ok(())
    }

    /// `_experimental_snapshot`: the whole machine, to restore later.
    pub async fn snapshot_memory(
        &self,
        rec: &SandboxRec,
        idempotency_key: &str,
    ) -> Result<String, TStatus> {
        if !rec.def().enable_snapshot {
            return Err(errors::invalid(
                "The Sandbox was not created with _experimental_enable_snapshot=True",
            ));
        }
        if !idempotency_key.is_empty() {
            let known = self.db.read(|d| {
                d.snapshots
                    .values()
                    .find(|s| {
                        s.sandbox_id == rec.id
                            && s.tags.get("idempotency_key").map(String::as_str)
                                == Some(idempotency_key)
                    })
                    .map(|s| s.id.clone())
            });
            if let Some(id) = known {
                return Ok(id);
            }
        }
        let id = if rec.v2 {
            ids::new_id_v2("sn")
        } else {
            ids::new_id("sn")
        };
        let cp = format!("modal-snap-{}", id.to_ascii_lowercase());
        let name = rec
            .cua_name
            .as_deref()
            .ok_or_else(|| errors::conflict("the sandbox is not running"))?;
        self.checkpoint_retry(&rec.identity(), name, &cp)
            .await
            .map_err(errors::from_backend)?;
        let mut tags = rec.tags.clone();
        if !idempotency_key.is_empty() {
            tags.insert("idempotency_key".into(), idempotency_key.into());
        }
        let snap = SnapshotRec {
            id: id.clone(),
            key_id: rec.key_id.clone(),
            sandbox_id: rec.id.clone(),
            checkpoint: cp,
            app_id: rec.app_id.clone(),
            image_id: rec.image_id.clone(),
            definition: rec.definition.clone(),
            env: rec.env.clone(),
            tags,
            created_at: now_s(),
        };
        self.db.write(|d| d.snapshots.insert(id.clone(), snap));
        Ok(id)
    }

    /// `_experimental_from_snapshot`: a new sandbox from a memory snapshot.
    pub async fn restore_sandbox(
        self: &Arc<Self>,
        caller: &Caller,
        snapshot_id: &str,
        name_type: i32,
        name: &str,
        v2: bool,
    ) -> Result<SandboxRec, TStatus> {
        use api::sandbox_restore_request::SandboxNameOverrideType as N;
        let snap = self
            .db
            .read(|d| {
                d.snapshots
                    .get(snapshot_id)
                    .filter(|s| s.key_id == caller.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Snapshot {snapshot_id} not found")))?;
        let mut def = base64::engine::general_purpose::STANDARD
            .decode(&snap.definition)
            .ok()
            .and_then(|b| <api::Sandbox as prost::Message>::decode(b.as_slice()).ok())
            .unwrap_or_default();
        match N::try_from(name_type).unwrap_or(N::Unspecified) {
            N::Unspecified => {}
            N::None => def.name = None,
            N::String => def.name = Some(name.to_string()),
        }
        let tags: BTreeMap<String, String> = snap
            .tags
            .iter()
            .filter(|(k, _)| k.as_str() != "idempotency_key")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let rec = self
            .create_sandbox_from(
                caller,
                &snap.app_id,
                def,
                tags,
                v2,
                snap.env.clone(),
                Default::default(),
                Some(snap.clone()),
            )
            .await?;
        Ok(rec)
    }

    /// The exit snapshot of a finishing sandbox (see
    /// `experimental_options={"enable_exit_snapshot": True}`).
    pub async fn take_exit_snapshot(&self, rec: &SandboxRec) {
        let r = self.snapshot_filesystem(rec).await;
        self.update_sandbox(&rec.id, |s| match r {
            Ok(id) => s.exit_snapshot = Some(id),
            Err(e) => s.exit_snapshot_error = Some(e.message().to_string()),
        });
    }

    /// Drops snapshot images past their `ttl` and the checkpoints only they
    /// used.
    pub async fn expire_images(&self) {
        let now = now_s();
        let expired: Vec<ImageRec> = self.db.read(|d| {
            d.images
                .values()
                .filter(|i| i.expires_at.is_some_and(|t| t < now))
                .cloned()
                .collect()
        });
        for img in expired {
            let in_use = self.db.read(|d| {
                d.sandboxes
                    .values()
                    .any(|s| s.image_id == img.id && s.running())
            });
            if in_use {
                continue;
            }
            if let Some(cp) = img.built.as_ref().and_then(|b| b.checkpoint.clone()) {
                let shared = self.db.read(|d| {
                    d.images.values().any(|i| {
                        i.id != img.id
                            && i.built.as_ref().and_then(|b| b.checkpoint.as_ref()) == Some(&cp)
                    })
                });
                if !shared {
                    let who = self.identity_of_key(&img.key_id).await;
                    let _ = self.backend.delete_checkpoint(&who, &cp).await;
                }
            }
            self.db.write(|d| d.images.remove(&img.id));
        }
    }
}

async fn run_script(
    client: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    script: &str,
    args: &[&str],
) -> Result<(), TStatus> {
    let out = crate::guest::run_as(client, driver, "root", script, args)
        .await
        .map_err(errors::conflict)?;
    if out.success() {
        Ok(())
    } else {
        Err(errors::conflict(out.stderr_str().trim().to_string()))
    }
}
