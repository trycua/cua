//! The [`ImageInfo`] of a named (user-owned) Fleet pool: its template's
//! image, pinned by the one resolver ([`cua_image::resolve`]).
//!
//! A managed pool's key is its resolved image, so its claims know the
//! digest already. A named pool is claimed as-is, so its template is read
//! back and its image resolved here, once per pool and template image.
//! Resolution never fails a claim: an unreadable registry (a private image
//! without credentials) still reports the template reference, and the
//! digest when the reference is digest-pinned.

use crate::runtime::ImageInfo;
use cua_fleet::{FleetClient, RegistryCredentials, RuntimeKind};
use cua_image::resolve::Backend;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

/// How long a registry read may take before the reference is recorded as
/// given.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(20);

/// `(pool, template image, runtime)` → resolved info.
type Cache = HashMap<(String, String, String), ImageInfo>;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Pool `pool`'s template image as an [`ImageInfo`]: resolved and pinned
/// when the registry can be read (with `creds`, when given), else the
/// template reference (see [`template_image_info`]). `Ok(None)` when the
/// template names no image; `Err` only when the template cannot be read.
pub async fn pool_image_info(
    fleet: &FleetClient,
    pool: &str,
    creds: Option<&RegistryCredentials>,
) -> crate::Result<Option<ImageInfo>> {
    let (spec, runtime, _) = fleet.pool_template(pool).await?;
    let image = spec.image.trim();
    if image.is_empty() {
        return Ok(None);
    }
    let key = (
        pool.to_string(),
        image.to_string(),
        cua_fleet::runtime_name(&runtime).to_string(),
    );
    if let Some(hit) = cache().lock().unwrap().get(&key).cloned() {
        return Ok(Some(hit));
    }
    let info = template_image_info(image, &runtime, creds).await;
    // Only a registry answer is cached: an unresolved reference is retried
    // on the next claim (the registry may answer then).
    if !info.digest.is_empty() {
        cache().lock().unwrap().insert(key, info.clone());
    }
    Ok(Some(info))
}

/// A template image as an [`ImageInfo`] for a pool on `runtime`:
///
/// - digest-pinned (`repo@sha256:…`): used as is, no registry read;
/// - otherwise resolved for the runtime's backend (gVisor: container,
///   KubeVirt: VM) on the node architecture; macOS templates are recorded
///   as given (no registry read);
/// - unresolvable: the reference, with empty `pinned_ref` / `digest`.
pub async fn template_image_info(
    image: &str,
    runtime: &RuntimeKind,
    creds: Option<&RegistryCredentials>,
) -> ImageInfo {
    let image = image.trim();
    if let Some(info) = pinned_info(image, runtime) {
        return info;
    }
    let arch = node_arch(runtime);
    let Some(backend) = backend(runtime) else {
        return unresolved_info(image, runtime);
    };
    let read = tokio::time::timeout(
        RESOLVE_TIMEOUT,
        cua_image::resolve::resolve_with_credentials(image, backend, arch, creds),
    )
    .await;
    match read {
        Ok(Ok(r)) => ImageInfo::from(&r),
        Ok(Err(e)) => {
            tracing::debug!(image, error = %e, "pool template image not resolved; recording the reference");
            unresolved_info(image, runtime)
        }
        Err(_) => {
            tracing::debug!(
                image,
                "pool template image: registry timed out; recording the reference"
            );
            unresolved_info(image, runtime)
        }
    }
}

/// The resolver backend for a Fleet runtime (none for macOS: its template
/// is recorded as given).
fn backend(runtime: &RuntimeKind) -> Option<Backend> {
    match runtime {
        RuntimeKind::Gvisor => Some(Backend::Container),
        RuntimeKind::Kubevirt => Some(Backend::Vm),
        RuntimeKind::Macos => None,
    }
}

/// Fleet's node architecture for a runtime (Linux nodes are amd64; macOS
/// hosts are Apple silicon).
fn node_arch(runtime: &RuntimeKind) -> &'static str {
    match runtime {
        RuntimeKind::Macos => "arm64",
        _ => "amd64",
    }
}

/// The variant a runtime runs.
fn variant(runtime: &RuntimeKind) -> &'static str {
    match runtime {
        RuntimeKind::Gvisor => "rootfs",
        RuntimeKind::Kubevirt => "containerdisk",
        RuntimeKind::Macos => "vm",
    }
}

/// The guest OS a runtime implies, when it does (a KubeVirt disk may be
/// Linux or Windows).
fn os(runtime: &RuntimeKind) -> &'static str {
    match runtime {
        RuntimeKind::Gvisor => "linux",
        RuntimeKind::Kubevirt => "",
        RuntimeKind::Macos => "macos",
    }
}

/// `repo@sha256:…` as is. The tag, if any, is dropped from `pinned_ref`.
fn pinned_info(image: &str, runtime: &RuntimeKind) -> Option<ImageInfo> {
    let at = image.find("@sha256:")?;
    let digest = &image[at + 1..];
    if digest.len() <= "sha256:".len() {
        return None;
    }
    let (repo, _tag) = split_tag(&image[..at]);
    Some(ImageInfo {
        reference: image.to_string(),
        pinned_ref: format!("{repo}@{digest}"),
        digest: digest.to_string(),
        variant: variant(runtime).into(),
        arch: Some(node_arch(runtime).into()),
        os: os(runtime).into(),
        emulated: false,
        // Unknown without a registry read of the image.
        spacesd: None,
    })
}

fn unresolved_info(image: &str, runtime: &RuntimeKind) -> ImageInfo {
    ImageInfo {
        reference: image.to_string(),
        pinned_ref: String::new(),
        digest: String::new(),
        variant: variant(runtime).into(),
        arch: Some(node_arch(runtime).into()),
        os: os(runtime).into(),
        emulated: false,
        spacesd: None,
    }
}

/// `repo[:tag]` → `(repo, tag)`; a registry port is not a tag.
fn split_tag(name: &str) -> (&str, Option<&str>) {
    let slash = name.rfind('/').map_or(0, |i| i + 1);
    match name[slash..].rfind(':') {
        Some(i) => (&name[..slash + i], Some(&name[slash + i + 1..])),
        None => (name, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_pinned_references_are_used_as_is() {
        let d = format!("sha256:{}", "a".repeat(64));
        let i = pinned_info(
            &format!("registry:5000/team/app:v1@{d}"),
            &RuntimeKind::Kubevirt,
        )
        .unwrap();
        assert_eq!(i.pinned_ref, format!("registry:5000/team/app@{d}"));
        assert_eq!(i.digest, d);
        assert_eq!(i.variant, "containerdisk");
        assert!(pinned_info("ghcr.io/trycua/linux:24.04", &RuntimeKind::Gvisor).is_none());
        assert_eq!(split_tag("registry:5000/app"), ("registry:5000/app", None));
    }
}
