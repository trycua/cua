//! [`Spaces`] over the cua SDK: the cua daemon (`Cua::connect`), so the
//! runner Spaces show in Cua Spaces like any other, or a runtime in this
//! process.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_sdk::{Cua, CuaError, Space};

use cua_ci::egress::EgressPolicy;
use cua_ci::spaces::{ShOutput, SpaceRow, SpaceSpec, Spaces, SpacesError};

/// The cua SDK's Spaces.
pub struct SdkSpaces {
    cua: Arc<Cua>,
    handles: tokio::sync::Mutex<HashMap<String, Arc<Space>>>,
}

impl SdkSpaces {
    /// Over `cua`.
    pub fn new(cua: Arc<Cua>) -> Self {
        Self {
            cua,
            handles: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    async fn space(&self, id: &str) -> Result<Arc<Space>, SpacesError> {
        let mut h = self.handles.lock().await;
        if let Some(s) = h.get(id) {
            return Ok(s.clone());
        }
        let s = self
            .cua
            .spaces()
            .space(id.to_string())
            .await
            .map_err(classify)?;
        h.insert(id.to_string(), s.clone());
        Ok(s)
    }

    async fn forget(&self, id: &str) {
        self.handles.lock().await.remove(id);
    }
}

fn row(i: cua_sdk::SpaceInfo) -> SpaceRow {
    SpaceRow {
        id: i.id,
        name: i.name,
        labels: i.labels.into_iter().collect(),
        os: i.os,
        arch: i.arch,
        provider: i.provider,
        power_state: i.power_state,
    }
}

/// An SDK error as a [`SpacesError`]: a capacity refusal (two macOS VMs per
/// Mac, a host's Space limit or its memory) is
/// [`SpacesError::LimitExceeded`], an egress refusal
/// [`SpacesError::EgressUnsupported`].
pub fn classify(e: CuaError) -> SpacesError {
    let msg = e.to_string();
    let lower = msg.to_ascii_lowercase();
    match e {
        CuaError::HostCapacity(m) => SpacesError::LimitExceeded(m),
        CuaError::NotFound(m) => SpacesError::NotFound(m),
        CuaError::Unsupported(m) if m.to_ascii_lowercase().contains("egress") => {
            SpacesError::EgressUnsupported(m)
        }
        _ if is_limit(&lower) => SpacesError::LimitExceeded(msg),
        _ => SpacesError::Other(msg),
    }
}

/// The words of a capacity refusal: the host's own limits
/// (`already runs 2 macOS VMs and allows 2`, `already provides N Spaces,
/// its limit`) and Apple Virtualization's (`The maximum supported number
/// of active virtual machines has been reached`).
fn is_limit(lower: &str) -> bool {
    (lower.contains("macos vm") && lower.contains("allows"))
        || lower.contains("its limit")
        || lower.contains("maximum supported number of active virtual machines")
        || lower.contains("virtualmachinelimitexceeded")
        || lower.contains("no room for another space")
}

#[async_trait]
impl Spaces for SdkSpaces {
    async fn create(&self, spec: &SpaceSpec) -> Result<SpaceRow, SpacesError> {
        let options = cua_sdk::SpaceCreateOptions {
            image: Some(spec.image.clone()),
            on: Some(spec.on.clone()),
            name: Some(spec.name.clone()),
            cpus: spec.cpus,
            memory_mb: spec.memory_mb,
            disk_gb: spec.disk_gb,
            timeout_ms: Some(spec.timeout.as_millis() as u64),
            wait: Some(true),
            create_id: Some(spec.create_id.clone()),
            labels: spec.labels.clone().into_iter().collect(),
            egress: spec.egress.as_ref().map(|p| cua_sdk::EgressPolicy {
                allow: p.allow.clone(),
                deny: p.deny.clone(),
            }),
            ..Default::default()
        };
        let created = match self.cua.spaces().create(options).await.map_err(classify) {
            Ok(c) => c,
            // Apple Virtualization refuses a third macOS VM late: the VM
            // "stopped while booting". With two already running, that is
            // the limit, not a broken image.
            Err(SpacesError::Other(m)) if m.contains("stopped while booting") => {
                return Err(match self.macos_vms_running().await {
                    Some(n) if n >= cua_ci::config::MACOS_VMS_PER_HOST => {
                        SpacesError::LimitExceeded(format!(
                            "{m} (this Mac runs {n} macOS VMs; Apple allows {})",
                            cua_ci::config::MACOS_VMS_PER_HOST
                        ))
                    }
                    _ => SpacesError::Other(m),
                });
            }
            Err(e) => return Err(e),
        };
        let info = created
            .space
            .ok_or_else(|| SpacesError::Other("the create returned no Space".into()))?;
        Ok(row(info))
    }

    async fn delete(&self, id: &str) -> Result<(), SpacesError> {
        self.forget(id).await;
        self.cua
            .spaces()
            .delete(id.to_string())
            .await
            .map(|_| ())
            .map_err(classify)
    }

    async fn list(&self) -> Result<Vec<SpaceRow>, SpacesError> {
        Ok(self
            .cua
            .spaces()
            .list()
            .await
            .map_err(classify)?
            .into_iter()
            .map(row)
            .collect())
    }

    async fn sh(&self, id: &str, script: &str, timeout: Duration) -> Result<ShOutput, SpacesError> {
        let s = self.space(id).await?;
        let o = s
            .bash(script.to_string(), Some(timeout.as_millis() as u64))
            .await
            .map_err(classify)?;
        Ok(ShOutput {
            stdout: o.stdout,
            stderr: o.stderr,
            exit_code: o.exit_code,
        })
    }

    async fn write_file(
        &self,
        id: &str,
        path: &str,
        data: &[u8],
        mode: u32,
    ) -> Result<(), SpacesError> {
        let s = self.space(id).await?;
        let d = s.spacesd().map_err(classify)?;
        d.upload(
            path.to_string(),
            data.to_vec(),
            Some(cua_sdk::UploadOptions {
                permissions: mode,
                create_parents: true,
                ..Default::default()
            }),
        )
        .await
        .map(|_| ())
        .map_err(classify)
    }

    async fn upload_file(&self, id: &str, local: &Path, path: &str) -> Result<(), SpacesError> {
        let s = self.space(id).await?;
        let d = s.spacesd().map_err(classify)?;
        d.upload_file(
            local.to_string_lossy().into_owned(),
            path.to_string(),
            Some(cua_sdk::UploadOptions {
                create_parents: true,
                ..Default::default()
            }),
        )
        .await
        .map(|_| ())
        .map_err(classify)
    }

    async fn download(&self, id: &str, remote: &str, local_dir: &Path) -> Result<(), SpacesError> {
        let s = self.space(id).await?;
        s.download(remote.to_string(), local_dir.to_string_lossy().into_owned())
            .await
            .map(|_| ())
            .map_err(classify)
    }

    async fn spawn(
        &self,
        id: &str,
        program: &str,
        args: &[String],
        cwd: &str,
        tag: &str,
    ) -> Result<(), SpacesError> {
        let s = self.space(id).await?;
        let d = s.spacesd().map_err(classify)?;
        let p = d
            .spawn(cua_sdk::SpacesdCommand {
                program: program.to_string(),
                args: args.to_vec(),
                env: HashMap::new(),
                cwd: Some(cwd.to_string()),
                user: None,
                timeout_ms: None,
                tag: Some(tag.to_string()),
                stdin: false,
                pty: None,
                request_id: None,
                epoch: None,
                scrollback_bytes: Some(64 * 1024),
                kill_on_disconnect: false,
            })
            .await
            .map_err(classify)?;
        // Detached: it keeps running after this handle goes.
        p.detach().await;
        Ok(())
    }

    // The SDK's create options carry no host folder shares yet (the
    // runtime's create spec has `mounts`; see REQUESTS-B.md), so the pool
    // falls back to uploading the caches.
    fn supports_mounts(&self) -> bool {
        false
    }

    async fn checkpoint(&self, id: &str, name: &str) -> Result<(), SpacesError> {
        let s = self.space(id).await?;
        s.checkpoint(name.to_string(), true)
            .await
            .map(|_| ())
            .map_err(classify)
    }

    async fn checkpoints(&self) -> Result<Vec<String>, SpacesError> {
        Ok(self
            .cua
            .sandboxes()
            .checkpoints()
            .await
            .map_err(classify)?
            .into_iter()
            .map(|c| c.name)
            .collect())
    }

    async fn delete_checkpoint(&self, name: &str) -> Result<(), SpacesError> {
        let sb = self.cua.sandboxes();
        // A pinned checkpoint is unpinned first; a missing one is fine.
        let _ = sb.pin_checkpoint(name.to_string(), false).await;
        match sb
            .delete_checkpoint(name.to_string())
            .await
            .map_err(classify)
        {
            Ok(()) | Err(SpacesError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn macos_vms_running(&self) -> Option<u32> {
        if !cfg!(target_os = "macos") {
            return Some(0);
        }
        let client = cua_vmm::lume::LumeClient::new(cua_vmm::lume::LumeConfig::default().url);
        match tokio::time::timeout(Duration::from_secs(4), client.list()).await {
            Ok(Ok(vms)) => Some(
                vms.iter()
                    .filter(|v| v.status == "running" && v.os.eq_ignore_ascii_case("macos"))
                    .count() as u32,
            ),
            _ => None,
        }
    }

    async fn set_egress(&self, id: &str, policy: &EgressPolicy) -> Result<(), SpacesError> {
        self.cua
            .spaces()
            .set_egress(
                id.to_string(),
                cua_sdk::EgressPolicy {
                    allow: policy.allow.clone(),
                    deny: policy.deny.clone(),
                },
            )
            .await
            .map_err(classify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_refusals_are_limit_exceeded() {
        for e in [
            CuaError::Env(
                "mini already runs 2 macOS VMs and allows 2: Apple's macOS license allows two \
                 macOS VMs per Mac"
                    .into(),
            ),
            CuaError::Runtime(
                "lume: The maximum supported number of active virtual machines has been reached."
                    .into(),
            ),
            CuaError::HostCapacity("memory".into()),
            CuaError::Env("mini already provides 4 Spaces, its limit".into()),
        ] {
            assert!(
                matches!(classify(e.clone()), SpacesError::LimitExceeded(_)),
                "{e}"
            );
        }
        assert!(matches!(
            classify(CuaError::Unsupported(
                "An egress policy (macOS VMs share Apple's vmnet NAT, which only pf can filter, \
                 and pf needs root) is not supported for lume Spaces"
                    .into()
            )),
            SpacesError::EgressUnsupported(m) if m.contains("pf needs root")
        ));
        assert!(matches!(
            classify(CuaError::NotFound("local:x".into())),
            SpacesError::NotFound(_)
        ));
        assert!(matches!(
            classify(CuaError::Runtime("image pull failed".into())),
            SpacesError::Other(_)
        ));
    }
}
