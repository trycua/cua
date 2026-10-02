//! What local Spaces have room for: free space on the volume each engine
//! writes to (Lume's VM storage, the cua home for QEMU, the container
//! engine's data disk), the space cua keeps free, and which catalog images
//! are already pulled here (a pulled image downloads nothing).
//!
//! Read-only and bounded: nothing is started, pulled or installed; an engine
//! that does not answer counts as nothing pulled and no volume.

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use cua_vmm::storage::Volume;

/// Local storage as the New Space wizard needs it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageReport {
    /// This host's architecture (`arm64`, `amd64`).
    pub host_arch: String,
    /// Bytes cua keeps free on a volume: a create that would leave less
    /// fails (`CUA_DISK_MIN_FREE`, default 5 GiB).
    pub reserve: u64,
    /// Where Lume writes macOS VMs.
    pub lume: Option<Volume>,
    /// Where QEMU writes VM disks and the containerDisk cache.
    pub qemu: Option<Volume>,
    /// The container engine's data disk.
    pub container: Option<Volume>,
    /// Catalog refs whose published digest is already pulled here.
    pub pulled: Vec<String>,
}

const PROBE: Duration = Duration::from_secs(10);

/// Probes local storage (see the module docs).
pub async fn storage_report() -> StorageReport {
    let host_arch = cua_vmm::Arch::host().oci().to_string();
    let entries: Vec<_> = cua_image::catalog::entries()
        .iter()
        .filter(|e| e.published && e.digest.is_some())
        .collect();
    let lume = async {
        (
            cua_vmm::storage::host_volume(&cua_vmm::storage::lume_dir()),
            pulled_lume(&entries),
        )
    };
    let qemu = async {
        let cache = cua_image::ImageCache::default();
        let pulled: Vec<String> = entries
            .iter()
            .filter(|e| e.local.as_deref() == Some("qemu"))
            .filter(|e| {
                e.sizes
                    .as_ref()
                    .filter(|s| Some(&s.digest) == e.digest.as_ref())
                    .and_then(|s| s.platform(&host_arch))
                    .and_then(|p| cache.disk_dir(&p.manifest).ok())
                    .is_some_and(|d| d.join("disk.qcow2").exists())
            })
            .map(|e| e.reference.clone())
            .collect();
        (
            cua_vmm::storage::host_volume(&cua_vmm::storage::qemu_dir()),
            pulled,
        )
    };
    let container = async {
        let vol = tokio::time::timeout(PROBE, cua_vmm::storage::container_volume())
            .await
            .ok()
            .flatten();
        (vol, pulled_containers(&entries).await)
    };
    let ((lume, a), (qemu, b), (container, c)) = tokio::join!(lume, qemu, container);
    StorageReport {
        host_arch,
        reserve: cua_vmm::disk::CacheConfig::load().min_free,
        lume,
        qemu,
        container,
        pulled: a.into_iter().chain(b).chain(c).collect(),
    }
}

/// macOS images whose base VM (`cua-base-<sha(ref@digest)>`, what a create
/// clones) is in Lume's storage.
fn pulled_lume(entries: &[&cua_image::catalog::CatalogEntry]) -> Vec<String> {
    let dir = cua_vmm::storage::lume_dir();
    entries
        .iter()
        .filter(|e| e.local.as_deref() == Some("lume"))
        .filter(|e| {
            let base = cua_vmm::lume::base_vm_name(&format!(
                "{}@{}",
                e.reference,
                e.digest.as_deref().unwrap_or_default()
            ));
            let vm = dir.join(base);
            vm.join("config.json").exists() && vm.join("disk.img").exists()
        })
        .map(|e| e.reference.clone())
        .collect()
}

/// Container images the engine has at their published digest.
async fn pulled_containers(entries: &[&cua_image::catalog::CatalogEntry]) -> Vec<String> {
    let wanted: Vec<_> = entries
        .iter()
        .filter(|e| e.local.as_deref() == Some("container"))
        .collect();
    if wanted.is_empty() {
        return vec![];
    }
    let Ok(Ok(rt)) = tokio::time::timeout(
        PROBE,
        cua_vmm::container::ContainerRuntime::connect(Default::default()),
    )
    .await
    else {
        return vec![];
    };
    let mut out = vec![];
    for e in wanted {
        let repo = repo_of(&e.reference);
        let pinned = format!("{repo}@{}", e.digest.as_deref().unwrap_or_default());
        if let Ok(Ok(_)) = tokio::time::timeout(PROBE, rt.docker().inspect_image(&pinned)).await {
            out.push(e.reference.clone());
        }
    }
    out
}

/// `ghcr.io/trycua/linux:24.04` -> `ghcr.io/trycua/linux`.
fn repo_of(reference: &str) -> &str {
    let name = reference.split('@').next().unwrap_or(reference);
    let slash = name.rfind('/').map_or(0, |i| i + 1);
    match name[slash..].rfind(':') {
        Some(i) => &name[..slash + i],
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_drops_tag_and_digest() {
        assert_eq!(
            repo_of("ghcr.io/trycua/linux:24.04"),
            "ghcr.io/trycua/linux"
        );
        assert_eq!(repo_of("localhost:5000/app"), "localhost:5000/app");
        assert_eq!(
            repo_of("localhost:5000/app:1@sha256:00"),
            "localhost:5000/app"
        );
    }

    #[test]
    fn every_published_image_with_a_digest_has_current_sizes() {
        for e in cua_image::catalog::entries()
            .iter()
            .filter(|e| e.digest.is_some())
        {
            let s = e.sizes.as_ref().expect("sizes");
            assert_eq!(Some(&s.digest), e.digest.as_ref(), "{}", e.reference);
            assert!(s.platform("arm64").is_some(), "{}", e.reference);
        }
    }
}
