//! Modal Volumes on Cua Volume.
//!
//! A Modal Volume is a folder of Cua Volume, `modal-volumes/<store>/`: one
//! versioned volume per account that every Space of the account shares, so
//! the files show in Finder and in every Space. The Volume API (listdir,
//! read_file, batch_upload, remove_file, copy_files, delete) works on the
//! folder through the Cua SDK; a sandbox's `volumes={path: volume}` mounts
//! it at `path` (a bind mount of the folder where the Space mounts Cua
//! Volume at `/volume`).
//!
//! Modal Volumes v1 are eventually consistent (`commit` publishes, `reload`
//! fetches). Cua Volume is consistent at once, so `commit` and `reload` have
//! nothing to wait for. File modes and ownership are not stored (a Cua
//! Volume is an object store with versioned content).

use crate::backend::{BackendError, VolumeEntry};
use crate::modal::db::{VolumeRec, now_s};
use crate::modal::pb::api;
use crate::modal::sandbox::SandboxRec;
use crate::modal::{Caller, Modal, errors, ids};
use cua_spacesd_client::SpacesdClient;
use std::collections::BTreeMap;
use tonic::Status;

pub const ROOT: &str = "modal-volumes";
pub const MARKER: &str = ".modal-volume";
/// The block size of the volume download protocol (`VolumeGetFile2`).
pub const BLOCK_SIZE: u64 = 8 * 1024 * 1024;

/// A normalised volume-relative path (no `..`, no leading `/`).
pub fn rel_path(path: &str) -> Result<String, Status> {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(errors::invalid(format!("path {path:?} escapes the volume")));
                }
            }
            p => parts.push(p),
        }
    }
    if parts.first() == Some(&MARKER) && parts.len() == 1 {
        return Err(errors::invalid(format!("{MARKER} is reserved")));
    }
    Ok(parts.join("/"))
}

impl Modal {
    pub fn volume_of(&self, caller: &Caller, id: &str) -> Result<VolumeRec, Status> {
        self.db
            .read(|d| {
                d.volumes
                    .get(id)
                    .filter(|v| v.key_id == caller.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Volume {id} not found")))
    }

    fn vol_prefix(v: &VolumeRec) -> String {
        format!("{ROOT}/{}", v.store)
    }

    pub fn vol_key(v: &VolumeRec, rel: &str) -> String {
        if rel.is_empty() {
            Self::vol_prefix(v)
        } else {
            format!("{}/{}", Self::vol_prefix(v), rel)
        }
    }

    /// Creates a volume's folder (a marker file keeps an empty one).
    pub async fn init_volume(&self, caller: &Caller, v: &VolumeRec) -> Result<(), Status> {
        self.backend
            .volume_write(
                &caller.identity(),
                &Self::vol_key(v, MARKER),
                v.id.clone().into_bytes(),
            )
            .await
            .map_err(errors::from_backend)
    }

    async fn ls(
        &self,
        caller: &Caller,
        v: &VolumeRec,
        rel: &str,
    ) -> Result<Vec<VolumeEntry>, Status> {
        let key = Self::vol_key(v, rel);
        let mut out = self
            .backend
            .volume_ls(&caller.identity(), &key)
            .await
            .map_err(errors::from_backend)?;
        out.retain(|e| e.name != MARKER);
        Ok(out)
    }

    fn entry_rel(v: &VolumeRec, e: &VolumeEntry) -> String {
        e.path
            .strip_prefix(&format!("{}/", Self::vol_prefix(v)))
            .unwrap_or(&e.path)
            .trim_end_matches('/')
            .to_string()
    }

    /// What `listdir` returns for `path`.
    pub async fn vol_list(
        &self,
        caller: &Caller,
        v: &VolumeRec,
        path: &str,
        recursive: bool,
        max: Option<u32>,
    ) -> Result<Vec<api::FileEntry>, Status> {
        let rel = rel_path(path)?;
        let file_entry = |e: &VolumeEntry| api::FileEntry {
            path: Self::entry_rel(v, e),
            r#type: if e.folder {
                api::file_entry::FileType::Directory as i32
            } else {
                api::file_entry::FileType::File as i32
            },
            mtime: e.modified_ms / 1000,
            size: e.size,
        };
        // A file path lists that file.
        if !rel.is_empty() {
            let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
            let name = rel.rsplit('/').next().unwrap_or(&rel);
            let sibs = self.ls(caller, v, parent).await.unwrap_or_default();
            if let Some(e) = sibs.iter().find(|e| e.name == name && !e.folder) {
                return Ok(vec![file_entry(e)]);
            }
        }
        let first = self.ls(caller, v, &rel).await?;
        if first.is_empty() && !rel.is_empty() {
            // An empty or missing folder: only the volume root may be empty.
            return Err(errors::not_found(format!(
                "No such file or directory: {path}"
            )));
        }
        let mut out = Vec::new();
        let mut queue: std::collections::VecDeque<VolumeEntry> = first.into();
        while let Some(e) = queue.pop_front() {
            out.push(file_entry(&e));
            if let Some(m) = max
                && out.len() >= m as usize
            {
                break;
            }
            if recursive && e.folder {
                let sub = Self::entry_rel(v, &e);
                queue.extend(self.ls(caller, v, &sub).await?);
            }
        }
        Ok(out)
    }

    /// A file's bytes.
    pub async fn vol_read(
        &self,
        caller: &Caller,
        v: &VolumeRec,
        rel: &str,
    ) -> Result<Vec<u8>, Status> {
        match self
            .backend
            .volume_read(&caller.identity(), &Self::vol_key(v, rel))
            .await
        {
            Ok(b) => Ok(b),
            Err(BackendError::NotFound(_)) => Err(errors::not_found(format!(
                "No such file or directory: {rel}"
            ))),
            Err(e) => Err(errors::from_backend(e)),
        }
    }

    /// Whether a file or folder exists at `rel`.
    pub async fn vol_exists(&self, caller: &Caller, v: &VolumeRec, rel: &str) -> bool {
        if rel.is_empty() {
            return true;
        }
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        let name = rel.rsplit('/').next().unwrap_or(rel);
        self.ls(caller, v, parent)
            .await
            .map(|l| l.iter().any(|e| e.name == name))
            .unwrap_or(false)
    }

    /// Removes a file, or a folder tree.
    pub async fn vol_remove(
        &self,
        caller: &Caller,
        v: &VolumeRec,
        rel: &str,
        recursive: bool,
    ) -> Result<(), Status> {
        if rel.is_empty() {
            return Err(errors::invalid("cannot remove the volume root"));
        }
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let sibs = self.ls(caller, v, parent).await.unwrap_or_default();
        let Some(e) = sibs.into_iter().find(|e| e.name == name) else {
            return Err(errors::not_found(format!(
                "No such file or directory: {rel}"
            )));
        };
        if e.folder {
            if !recursive {
                return Err(errors::invalid(format!(
                    "{rel} is a directory; pass recursive=True"
                )));
            }
            let mut stack = vec![rel.to_string()];
            while let Some(d) = stack.pop() {
                for c in self.ls(caller, v, &d).await? {
                    let r = Self::entry_rel(v, &c);
                    if c.folder {
                        stack.push(r);
                    } else {
                        self.backend
                            .volume_delete(&caller.identity(), &Self::vol_key(v, &r))
                            .await
                            .map_err(errors::from_backend)?;
                    }
                }
            }
            Ok(())
        } else {
            self.backend
                .volume_delete(&caller.identity(), &Self::vol_key(v, rel))
                .await
                .map_err(errors::from_backend)
        }
    }

    /// `copy_files(src_paths, dst_path)`.
    pub async fn vol_copy(
        &self,
        caller: &Caller,
        v: &VolumeRec,
        srcs: &[String],
        dst: &str,
        recursive: bool,
    ) -> Result<(), Status> {
        let dst_rel = rel_path(dst)?;
        let dst_is_dir = dst.ends_with('/')
            || dst_rel.is_empty()
            || self
                .ls(caller, v, &dst_rel)
                .await
                .map(|l| !l.is_empty())
                .unwrap_or(false);
        if srcs.len() > 1 && !dst_is_dir {
            return Err(errors::invalid(
                "copying several files needs a destination directory",
            ));
        }
        for s in srcs {
            let rel = rel_path(s)?;
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let mut jobs: Vec<(String, String)> = Vec::new();
            let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
            let sibs = self.ls(caller, v, parent).await.unwrap_or_default();
            let Some(e) = sibs.into_iter().find(|e| e.name == name) else {
                return Err(errors::not_found(format!("No such file or directory: {s}")));
            };
            let target_root = if dst_is_dir {
                if dst_rel.is_empty() {
                    name.clone()
                } else {
                    format!("{dst_rel}/{name}")
                }
            } else {
                dst_rel.clone()
            };
            if e.folder {
                if !recursive {
                    return Err(errors::invalid(format!(
                        "{s} is a directory; pass recursive=True"
                    )));
                }
                let mut stack = vec![rel.clone()];
                while let Some(d) = stack.pop() {
                    for c in self.ls(caller, v, &d).await? {
                        let r = Self::entry_rel(v, &c);
                        if c.folder {
                            stack.push(r);
                        } else {
                            let tail = r.strip_prefix(&format!("{rel}/")).unwrap_or(&r);
                            jobs.push((r.clone(), format!("{target_root}/{tail}")));
                        }
                    }
                }
            } else {
                jobs.push((rel.clone(), target_root));
            }
            for (from, to) in jobs {
                let data = self.vol_read(caller, v, &from).await?;
                self.backend
                    .volume_write(&caller.identity(), &Self::vol_key(v, &to), data)
                    .await
                    .map_err(errors::from_backend)?;
            }
        }
        Ok(())
    }

    /// Deletes every file of a volume.
    pub async fn vol_purge(&self, caller: &Caller, v: &VolumeRec) {
        let mut stack = vec![String::new()];
        while let Some(d) = stack.pop() {
            let key = Self::vol_key(v, &d);
            let Ok(list) = self.backend.volume_ls(&caller.identity(), &key).await else {
                continue;
            };
            for c in list {
                let r = Self::entry_rel(v, &c);
                if c.folder {
                    stack.push(r);
                } else {
                    let _ = self
                        .backend
                        .volume_delete(&caller.identity(), &Self::vol_key(v, &r))
                        .await;
                }
            }
        }
    }

    /// Mounts the sandbox's volumes (`volumes={path: Volume}`).
    pub async fn mount_volumes(
        &self,
        rec: &SandboxRec,
        def: &api::Sandbox,
        client: &SpacesdClient,
        driver: &str,
    ) -> Result<(), String> {
        let who = rec.identity();
        self.mount_volume_list(
            &rec.id,
            &rec.key_id,
            &who,
            &def.volume_mounts,
            &def.cloud_bucket_mounts,
            &rec.bucket_credentials,
            client,
            driver,
        )
        .await
    }

    /// Mounts volumes and cloud buckets in a guest (a sandbox or a Function
    /// container). `owner` names whatever ends the sync (a sandbox id or a
    /// container's task id): see [`Modal::volsync_stop`].
    #[allow(clippy::too_many_arguments)]
    pub async fn mount_volume_list(
        &self,
        owner: &str,
        key_id: &str,
        who: &crate::backend::Identity,
        volumes: &[api::VolumeMount],
        buckets: &[api::CloudBucketMount],
        credentials: &BTreeMap<String, BTreeMap<String, String>>,
        client: &SpacesdClient,
        driver: &str,
    ) -> Result<(), String> {
        if !volumes.is_empty() {
            let caller = Caller(
                crate::auth::KeyEntry {
                    key: String::new(),
                    label: String::new(),
                    daemon: who.daemon.clone(),
                    token: who.token.clone(),
                },
                false,
            );
            let sync = crate::modal::volsync::VolSync::new(
                who.clone(),
                self.backend.clone(),
                client.clone(),
                driver.to_string(),
            );
            for m in volumes {
                let v = self
                    .db
                    .read(|d| {
                        d.volumes
                            .get(&m.volume_id)
                            .filter(|v| v.key_id == key_id)
                            .cloned()
                    })
                    .ok_or_else(|| format!("Volume {} not found", m.volume_id))?;
                let sub = rel_path(m.sub_path.as_deref().unwrap_or(""))
                    .map_err(|e| e.message().to_string())?;
                self.init_volume(&caller, &v)
                    .await
                    .map_err(|e| e.message().to_string())?;
                sync.add(&m.mount_path, &Self::vol_key(&v, &sub), m.read_only)
                    .await
                    .map_err(|e| format!("mounting volume at {}: {e}", m.mount_path))?;
            }
            sync.start();
            self.live
                .volsyncs
                .lock()
                .await
                .insert(owner.to_string(), sync);
        }
        self.mount_buckets(key_id, buckets, credentials, client, driver)
            .await
    }

    /// `reload_volumes`: fetches what others committed since the mount.
    pub async fn reload_volumes(&self, rec: &SandboxRec) -> Result<(), Status> {
        let sync = self.live.volsyncs.lock().await.get(&rec.id).cloned();
        match sync {
            Some(s) => s.reload().await.map_err(errors::internal),
            None => Ok(()),
        }
    }

    /// The final push of an owner's mounts (its sandbox or container ended).
    pub async fn volsync_stop(&self, owner: &str) {
        let sync = self.live.volsyncs.lock().await.remove(owner);
        if let Some(s) = sync {
            s.stop().await;
        }
    }

    pub fn new_volume_rec(
        caller: &Caller,
        name: Option<String>,
        env: &str,
        version: i32,
    ) -> VolumeRec {
        let id = ids::new_id("vo");
        VolumeRec {
            store: name
                .clone()
                .unwrap_or_else(|| format!("_ephemeral-{}", id.to_ascii_lowercase())),
            id,
            key_id: caller.key_id(),
            name,
            environment: env.to_string(),
            created_at: now_s(),
            version,
            commits: 0,
            heartbeat_at: now_s(),
            ephemeral: false,
        }
    }
}

impl Modal {
    /// `CloudBucketMount`s, mounted in the guest with AWS Mountpoint for S3
    /// (what Modal mounts S3 buckets with, so file semantics match):
    /// AWS S3, and R2 and GCS through their S3-compatible endpoints.
    pub async fn mount_buckets(
        &self,
        key_id: &str,
        buckets: &[api::CloudBucketMount],
        credentials: &BTreeMap<String, BTreeMap<String, String>>,
        client: &SpacesdClient,
        driver: &str,
    ) -> Result<(), String> {
        use cua_spacesd_client::Command;
        if !buckets.is_empty() {
            let fuse = crate::guest::run_as(client, driver, "root", "test -c /dev/fuse", &[])
                .await?
                .success();
            if !fuse {
                return Err("this Space has no /dev/fuse (a container Space), which a bucket mount needs; use a VM Space (experimental_options cua.kind=vm)".into());
            }
            let installed = crate::guest::run_as(
                client,
                driver,
                "root",
                "command -v mount-s3 >/dev/null",
                &[],
            )
            .await?
            .success();
            if !installed {
                let out =
                    crate::guest::run_as(client, driver, "root", &mount_s3_install(), &[]).await?;
                if !out.success() {
                    return Err(format!(
                        "installing AWS Mountpoint for S3 in the sandbox failed (it needs network access): {}",
                        out.stderr_str().trim()
                    ));
                }
            }
        }
        for b in buckets {
            let mut env = std::collections::BTreeMap::new();
            if !b.credentials_secret_id.is_empty() {
                let sec = self
                    .db
                    .read(|d| {
                        d.secrets
                            .get(&b.credentials_secret_id)
                            .filter(|s| s.key_id == key_id)
                            .cloned()
                    })
                    .ok_or_else(|| format!("Secret {} not found", b.credentials_secret_id))?;
                env.extend(sec.env);
            }
            if let Some(extra) = credentials.get(&b.mount_path) {
                env.extend(extra.clone());
            }
            if b.oidc_auth_role_arn
                .as_deref()
                .is_some_and(|a| !a.is_empty())
            {
                return Err(
                    "oidc_auth_role_arn: assuming a role needs Modal's OIDC identity provider; give the mount a secret with AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY".into(),
                );
            }
            // GCS interoperability keys have Google names.
            if b.bucket_type == api::cloud_bucket_mount::BucketType::Gcp as i32 {
                if let Some(v) = env.remove("GOOGLE_ACCESS_KEY_ID") {
                    env.insert("AWS_ACCESS_KEY_ID".into(), v);
                }
                if let Some(v) = env.remove("GOOGLE_ACCESS_KEY_SECRET") {
                    env.insert("AWS_SECRET_ACCESS_KEY".into(), v);
                }
            }
            let mut args = vec![b.bucket_name.clone(), b.mount_path.clone()];
            if let Some(e) = b.bucket_endpoint_url.as_deref().filter(|e| !e.is_empty()) {
                args.push("--endpoint-url".into());
                args.push(e.into());
            }
            if b.read_only {
                args.push("--read-only".into());
            }
            if let Some(p) = b.key_prefix.as_deref().filter(|p| !p.is_empty()) {
                args.push("--prefix".into());
                args.push(p.into());
            }
            if b.requester_pays {
                args.push("--requester-pays".into());
            }
            if b.force_path_style {
                args.push("--force-path-style".into());
            }
            if env.is_empty() {
                args.push("--no-sign-request".into());
            }
            args.push("--allow-other".into());
            if let Some(r) = env.get("AWS_REGION").cloned() {
                args.push("--region".into());
                args.push(r);
            }
            // The mount point exists before the mount.
            crate::guest::run_as(
                client,
                driver,
                "root",
                "mkdir -p -- \"$1\"",
                &[&b.mount_path],
            )
            .await?;
            let (program, pargs, user, cwd) =
                crate::guest::as_user_in(driver, "root", "mount-s3".into(), args, "/".into());
            let mut cmd = Command::new(program).args(pargs);
            for (k, v) in &env {
                cmd = cmd.env(k, v);
            }
            if !user.is_empty() {
                cmd = cmd.user(user);
            }
            if !cwd.is_empty() {
                cmd = cmd.cwd(cwd);
            }
            let out = client.run(cmd).await.map_err(|e| e.to_string())?;
            if !out.success() {
                return Err(format!(
                    "mounting bucket {} at {}: {}",
                    b.bucket_name,
                    b.mount_path,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
        Ok(())
    }
}

/// Installs AWS Mountpoint for S3 (Apache-2.0) from AWS's release bucket.
fn mount_s3_install() -> String {
    format!("{}{}", super::image::FIX_APT_SOURCES, MOUNT_S3_INSTALL)
}

const MOUNT_S3_INSTALL: &str = r#"set -e
case "$(uname -m)" in
  x86_64) arch=x86_64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) echo "no Mountpoint for S3 build for $(uname -m)" >&2; exit 1 ;;
esac
tmp=$(mktemp -d)
curl -fsSL "https://s3.amazonaws.com/mountpoint-s3-release/latest/$arch/mount-s3.deb" -o "$tmp/mount-s3.deb"
(apt-get update -qq >/dev/null 2>&1 || true)
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "$tmp/mount-s3.deb" >/dev/null
rm -rf "$tmp"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_confined_to_the_volume() {
        assert_eq!(rel_path("/a/b/../c//d/").unwrap(), "a/c/d");
        assert_eq!(rel_path("").unwrap(), "");
        assert_eq!(rel_path("/").unwrap(), "");
        assert!(rel_path("../x").is_err());
        assert!(rel_path("a/../../x").is_err());
        assert!(rel_path(".modal-volume").is_err());
        assert_eq!(rel_path("sub/.modal-volume").unwrap(), "sub/.modal-volume");
    }
}
