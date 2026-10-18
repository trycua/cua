//! The Fleet runtime ↔ image pairing, validated in exactly one place.
//!
//! The two Linux Fleet runtimes consume different artifacts: `kubevirt`
//! boots a VM from a **KubeVirt containerDisk** (`/disk/disk.img`), `gvisor`
//! runs a runsc pod from a **container rootfs**. Crossing them fails on the
//! cluster only after a multi-GB pull, and then opaquely: an unexplained
//! `CreateContainerError`, or a VMI that never boots because the image has
//! no `/disk/*.img`.
//!
//! **The rule.** What an image is comes from its registry manifest, never
//! from its tag: an image is a containerDisk when its manifest or config
//! shows the `/disk/disk.img` layer or a trycua containerDisk media type
//! ([`cua_image::media_types::container_disk_evidence`]), and a container
//! rootfs otherwise. The runtime follows: containerDisk → `kubevirt`, rootfs
//! → `gvisor`; a crossed pairing is refused. A macOS image (Lume or a darwin
//! platform) and the `macos` runtime are [`Error::Unsupported`]
//! ([`MACOS_UNSUPPORTED`]): Fleet does not offer macOS in this SDK. Pools
//! that already have `macos` can still be listed and read.
//!
//! The manifest is read by the one resolver ([`cua_image::resolve`], with
//! the same credentials as `cua image pull`: docker config, credential
//! helpers, `GITHUB_TOKEN` for ghcr.io, `aws ecr get-login-password` for
//! private ECR), which also picks the variant a runtime needs (the `-disk`
//! sibling of a canonical image for `kubevirt`) and pins it by digest
//! ([`resolve_fleet_image`]). Unset runtimes follow the image; an
//! unreadable registry falls back to [`runtime_from_reference`].
//!
//! When the manifest cannot be read (offline, no registry credentials, a
//! registry that refuses anonymous reads), an explicit runtime is sent
//! unchecked and an unset one falls back to [`runtime_from_reference`], both
//! with a warning. An unreadable manifest is never a hard error on its own.
//!
//! Every path that creates a Fleet template goes through
//! [`resolve_runtime`] or [`check_runtime`]: [`crate::FleetClient::apply_pool`],
//! the cua SDK (`fleet_resolve_runtime`, which cua-sandbox calls),
//! cua-sandbox-core's managed pools and cua-spaces.

use std::{
    sync::{Arc, OnceLock, RwLock},
    time::Duration,
};

use cua_image::{
    ImageError,
    detect::ImageKind,
    manifest::{ImageIndex, ImageManifest, Manifest},
    resolve::{Backend, MACOS_FOUND, ResolvedImage, Variant},
};

use crate::{Error, Result, RuntimeKind};

/// Set to `0`/`off`/`false` to skip registry reads; every image is then
/// [`ImageEvidence::Unavailable`] (explicit runtimes only).
pub const INSPECT_ENV: &str = "CUA_FLEET_IMAGE_INSPECT";

/// What a Fleet image is, read from its manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageVariant {
    /// KubeVirt containerDisk (`/disk/disk.img`): `kubevirt` only.
    ContainerDisk,
    /// Container rootfs: `gvisor` only.
    Rootfs,
    /// Anything else (Lume/Tart VM bundles, cua QEMU disks, unknown
    /// artifacts): no Fleet runtime is known to run it.
    Other,
}

impl ImageVariant {
    /// Stable spelling (`container-disk`, `rootfs`, `other`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContainerDisk => "container-disk",
            Self::Rootfs => "rootfs",
            Self::Other => "other",
        }
    }

    /// Parses [`ImageVariant::as_str`]. `macos` is [`Error::Unsupported`]:
    /// Fleet does not offer macOS in this SDK.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "container-disk" | "containerdisk" => Ok(Self::ContainerDisk),
            "rootfs" => Ok(Self::Rootfs),
            "macos" => Err(macos_unsupported()),
            "other" => Ok(Self::Other),
            other => Err(Error::InvalidArgument(format!(
                "unknown image variant {other:?}: expected \"container-disk\", \"rootfs\" or \
                 \"other\""
            ))),
        }
    }

    /// The runtime this variant runs on, if any.
    pub fn runtime(self) -> Option<RuntimeKind> {
        match self {
            Self::ContainerDisk => Some(RuntimeKind::Kubevirt),
            Self::Rootfs => Some(RuntimeKind::Gvisor),
            Self::Other => None,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::ContainerDisk => {
                "a KubeVirt containerDisk (its manifest has the /disk/disk.img layer)"
            }
            Self::Rootfs => "a container rootfs (its manifest has no /disk/disk.img layer)",
            Self::Other => "not a Fleet image (neither a containerDisk nor a container rootfs)",
        }
    }
}

/// Why a macOS image or the `macos` runtime is refused.
pub const MACOS_UNSUPPORTED: &str = cua_image::resolve::FLEET_MACOS_UNSUPPORTED;

fn macos_unsupported() -> Error {
    Error::Unsupported(MACOS_UNSUPPORTED.into())
}

/// Refuses a runtime this SDK does not create or claim on: `macos`. Pools
/// that already have it can still be listed and read.
pub fn ensure_runtime_offered(runtime: &RuntimeKind) -> Result<()> {
    match runtime {
        RuntimeKind::Macos => Err(macos_unsupported()),
        RuntimeKind::Kubevirt | RuntimeKind::Gvisor => Ok(()),
    }
}

/// What a classified image is for Fleet: a macOS image (Lume or not) is
/// [`ImageEvidence::Unsupported`].
fn evidence_of_kind(kind: ImageKind) -> ImageEvidence {
    match kind {
        ImageKind::ContainerDisk => ImageEvidence::Known(ImageVariant::ContainerDisk),
        ImageKind::Rootfs => ImageEvidence::Known(ImageVariant::Rootfs),
        ImageKind::Macos | ImageKind::Lume => ImageEvidence::Unsupported(MACOS_UNSUPPORTED.into()),
        ImageKind::Other => ImageEvidence::Known(ImageVariant::Other),
    }
}

/// What is known about an image before choosing its runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageEvidence {
    /// Read from the registry manifest.
    Known(ImageVariant),
    /// Read from the registry manifest: an image Fleet does not run in this
    /// SDK (a macOS image); why.
    Unsupported(String),
    /// The manifest could not be read; why.
    Unavailable(String),
}

/// Classifies already-fetched documents: `index` is the index the reference
/// resolved to (if any), `manifest` the selected platform manifest, `config`
/// its config JSON. Pure; [`inspect_image`] does the fetching. A macOS image
/// is [`Error::Unsupported`]: Fleet does not offer macOS in this SDK.
pub fn classify_manifest(
    index: Option<&ImageIndex>,
    manifest: Option<&ImageManifest>,
    config: Option<&serde_json::Value>,
) -> Result<ImageVariant> {
    match evidence_of_kind(cua_image::detect::classify("", index, manifest, config)) {
        ImageEvidence::Known(v) => Ok(v),
        ImageEvidence::Unsupported(why) | ImageEvidence::Unavailable(why) => {
            Err(Error::Unsupported(why))
        }
    }
}

/// [`classify_manifest`] over raw JSON: `manifest` is an image manifest or
/// an index, `config` the platform manifest's config blob.
pub fn classify_manifest_json(manifest: &str, config: Option<&str>) -> Result<ImageVariant> {
    let parsed = Manifest::parse(manifest.as_bytes())
        .map_err(|e| Error::InvalidArgument(format!("unreadable image manifest: {e}")))?;
    let config = config
        .map(serde_json::from_str::<serde_json::Value>)
        .transpose()
        .map_err(|e| Error::InvalidArgument(format!("unreadable image config: {e}")))?;
    match &parsed {
        Manifest::Image(m) => classify_manifest(None, Some(m), config.as_ref()),
        Manifest::Index(i) => classify_manifest(Some(i), None, config.as_ref()),
    }
}

/// Validates the runtime/image pairing against `evidence` and returns the
/// runtime to send. The one rule (see the module docs).
pub fn check_runtime(
    runtime: Option<RuntimeKind>,
    image: &str,
    evidence: &ImageEvidence,
) -> Result<RuntimeKind> {
    if let Some(rt) = &runtime {
        ensure_runtime_offered(rt)?;
    }
    let variant = match evidence {
        ImageEvidence::Known(v) => *v,
        ImageEvidence::Unsupported(why) => return Err(Error::Unsupported(why.clone())),
        ImageEvidence::Unavailable(why) => {
            return match runtime {
                Some(rt) => {
                    tracing::warn!(image, runtime = runtime_name(&rt), reason = %why,
                        "could not read the image manifest; sending the runtime unchecked");
                    Ok(rt)
                }
                None => {
                    let rt = runtime_from_reference(image);
                    tracing::warn!(image, runtime = runtime_name(&rt), reason = %why,
                        "could not read the image manifest; guessed the runtime from the \
                         reference (a `docker-` tag runs on gvisor, anything else on kubevirt). \
                         Pass the runtime explicitly, or make the registry readable \
                         (docker login), if this is wrong");
                    Ok(rt)
                }
            };
        }
    };
    let Some(needed) = variant.runtime() else {
        return match runtime {
            Some(rt) => {
                tracing::warn!(
                    image,
                    runtime = runtime_name(&rt),
                    "image is neither a containerDisk nor a rootfs; sending the runtime unchecked"
                );
                Ok(rt)
            }
            None => Err(Error::InvalidArgument(format!(
                "{image:?} is {}; pass the runtime explicitly if Fleet can run it",
                variant.describe()
            ))),
        };
    };
    match runtime {
        None => Ok(needed),
        Some(rt) if rt == needed => Ok(rt),
        Some(rt) => Err(Error::InvalidArgument(format!(
            "runtime {:?} cannot run {image:?}: it is {}, which runs on runtime {:?}. {}",
            runtime_name(&rt),
            variant.describe(),
            runtime_name(&needed),
            crossed_hint(&rt, variant),
        ))),
    }
}

fn crossed_hint(runtime: &RuntimeKind, variant: ImageVariant) -> &'static str {
    match (runtime, variant) {
        (RuntimeKind::Gvisor, ImageVariant::ContainerDisk) => {
            "gVisor cannot run a disk image (the pull fails as an opaque CreateContainerError); \
             use runtime \"kubevirt\", or the image's rootfs build (the canonical images publish \
             `<tag>` as the rootfs and `<tag>-disk` as the containerDisk)"
        }
        (RuntimeKind::Kubevirt, ImageVariant::Rootfs) => {
            "A container image has no bootable disk; use runtime \"gvisor\", or the image's \
             containerDisk build"
        }
        _ => "Use the runtime the image was built for",
    }
}

/// Reads an image's manifest for [`resolve_runtime`]. The default is the
/// one resolver ([`cua_image::resolve`]); tests install fakes with
/// [`set_image_inspector`] (then the image is sent as given).
#[async_trait::async_trait]
pub trait ImageInspector: Send + Sync {
    /// What `image` is.
    async fn inspect(&self, image: &str) -> ImageEvidence;
}

/// The default [`ImageInspector`]: [`cua_image::resolve`] (registry auth
/// chain, five-minute cache) for amd64 with a timeout.
pub struct RegistryInspector {
    timeout: Duration,
}

impl Default for RegistryInspector {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
        }
    }
}

#[async_trait::async_trait]
impl ImageInspector for RegistryInspector {
    async fn inspect(&self, image: &str) -> ImageEvidence {
        match resolve_with_timeout(image, Backend::Auto, self.timeout, None).await {
            Ok(r) => evidence_of_variant(r.variant),
            Err(e) => evidence_of_error(e),
        }
    }
}

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(20);

async fn resolve_with_timeout(
    image: &str,
    backend: Backend,
    timeout: Duration,
    creds: Option<&cua_image::RegistryCredentials>,
) -> std::result::Result<ResolvedImage, ImageError> {
    // Fleet nodes are amd64.
    match tokio::time::timeout(
        timeout,
        cua_image::resolve::resolve_with_credentials(image, backend, "amd64", creds),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err(ImageError::Registry(format!(
            "registry did not answer within {}s",
            timeout.as_secs()
        ))),
    }
}

fn variant_of(v: Variant) -> ImageVariant {
    match v {
        Variant::Rootfs => ImageVariant::Rootfs,
        Variant::Containerdisk => ImageVariant::ContainerDisk,
        Variant::Lume => ImageVariant::Other,
    }
}

fn evidence_of_variant(v: Variant) -> ImageEvidence {
    match v {
        Variant::Lume => ImageEvidence::Unsupported(MACOS_UNSUPPORTED.into()),
        v => ImageEvidence::Known(variant_of(v)),
    }
}

/// Evidence from a resolver error: an image with no variant Fleet runs is
/// known (so a crossed or unrunnable pairing is refused); anything else
/// (offline, no credentials, not found) is unavailable.
fn evidence_of_error(e: ImageError) -> ImageEvidence {
    match e {
        ImageError::UnsupportedVariant { found, .. } => {
            let linux = found
                .iter()
                .filter_map(|f| Variant::parse(f))
                .map(variant_of)
                .find(|v| *v != ImageVariant::Other);
            match linux {
                Some(v) => ImageEvidence::Known(v),
                None if found
                    .iter()
                    .any(|f| f == MACOS_FOUND || f == Variant::Lume.as_str()) =>
                {
                    ImageEvidence::Unsupported(MACOS_UNSUPPORTED.into())
                }
                None => ImageEvidence::Known(ImageVariant::Other),
            }
        }
        other => ImageEvidence::Unavailable(format!("registry: {other}")),
    }
}

fn inspector_slot() -> &'static RwLock<Option<Arc<dyn ImageInspector>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn ImageInspector>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Replaces the process-wide [`ImageInspector`] (`None` restores the
/// resolver). Hosts with their own registry client, and tests, use this.
pub fn set_image_inspector(inspector: Option<Arc<dyn ImageInspector>>) {
    *inspector_slot().write().unwrap() = inspector;
}

fn installed_inspector() -> Option<Arc<dyn ImageInspector>> {
    inspector_slot().read().unwrap().clone()
}

fn inspection_disabled() -> bool {
    std::env::var(INSPECT_ENV).is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        )
    })
}

/// Reads what `image` is through the process-wide [`ImageInspector`].
pub async fn inspect_image(image: &str) -> ImageEvidence {
    if inspection_disabled() {
        return ImageEvidence::Unavailable(format!("manifest inspection is off ({INSPECT_ENV})"));
    }
    match installed_inspector() {
        Some(i) => i.inspect(image).await,
        None => RegistryInspector::default().inspect(image).await,
    }
}

/// An image resolved for Fleet: the reference to put in the template and
/// the runtime that runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct FleetImage {
    /// `repo@sha256:…` of the variant the runtime needs (the `-disk`
    /// sibling for `kubevirt`, ...), or the reference as given when the
    /// registry could not be read.
    pub image: String,
    /// The runtime.
    pub runtime: RuntimeKind,
    /// The resolution, when the registry was read.
    pub resolved: Option<ResolvedImage>,
}

fn backend_for(runtime: Option<&RuntimeKind>) -> Backend {
    match runtime {
        // `macos` is refused before resolving.
        None | Some(RuntimeKind::Macos) => Backend::Fleet,
        Some(RuntimeKind::Gvisor) => Backend::Container,
        Some(RuntimeKind::Kubevirt) => Backend::Vm,
    }
}

fn runtime_for(v: Variant) -> Option<RuntimeKind> {
    variant_of(v).runtime()
}

/// The one Fleet image rule: resolve `image` with [`cua_image::resolve`]
/// for the runtime (or any Fleet runtime when unset), digest-pinned, on
/// amd64. `linux:24.04` with `kubevirt` becomes the pinned `linux:24.04-disk`.
/// A crossed pairing is refused ([`check_runtime`] wording); an unreadable
/// registry keeps the reference and falls back like [`check_runtime`] (an
/// explicit runtime unchecked, else [`runtime_from_reference`]), with a
/// warning. With a test inspector installed ([`set_image_inspector`]) the
/// reference is sent as given.
pub async fn resolve_fleet_image(runtime: Option<RuntimeKind>, image: &str) -> Result<FleetImage> {
    resolve_fleet_image_with(runtime, image, None).await
}

/// [`resolve_fleet_image`] reading a private image with explicit
/// credentials (a `RegistrySecret`): they join the resolver's auth chain
/// ([`cua_image::resolve::resolve_with_credentials`]).
pub async fn resolve_fleet_image_with(
    runtime: Option<RuntimeKind>,
    image: &str,
    creds: Option<&cua_image::RegistryCredentials>,
) -> Result<FleetImage> {
    if let Some(rt) = &runtime {
        ensure_runtime_offered(rt)?;
    }
    let as_given = |runtime: Option<RuntimeKind>, evidence: ImageEvidence| {
        check_runtime(runtime, image, &evidence).map(|rt| FleetImage {
            image: image.to_string(),
            runtime: rt,
            resolved: None,
        })
    };
    if inspection_disabled() {
        return as_given(
            runtime,
            ImageEvidence::Unavailable(format!("manifest inspection is off ({INSPECT_ENV})")),
        );
    }
    if let Some(i) = installed_inspector() {
        let evidence = i.inspect(image).await;
        return as_given(runtime, evidence);
    }
    match resolve_with_timeout(image, backend_for(runtime.as_ref()), RESOLVE_TIMEOUT, creds).await {
        Ok(r) => {
            let Some(rt) = runtime_for(r.variant) else {
                return as_given(runtime, evidence_of_variant(r.variant));
            };
            if runtime.as_ref().is_some_and(|given| *given != rt) {
                return as_given(runtime, evidence_of_variant(r.variant));
            }
            if r.emulated {
                tracing::warn!(
                    image,
                    arch = ?r.arch,
                    "image has no linux/amd64 build; Fleet nodes are amd64"
                );
            }
            Ok(FleetImage {
                image: r.pinned_ref.clone(),
                runtime: rt,
                resolved: Some(r),
            })
        }
        Err(e) => as_given(runtime, evidence_of_error(e)),
    }
}

/// The canonical image for `os` (`linux`, `windows`, `macos`), honouring
/// `CUA_IMAGE_<OS>` ([`cua_image::canonical`]). Unknown OS names get Linux.
pub fn canonical_image(os: &str) -> String {
    cua_image::canonical::canonical_image(os, None).unwrap_or_else(|_| {
        cua_image::canonical::canonical_for(cua_image::canonical::CanonicalOs::Linux, None)
    })
}

/// A canonical alias (`linux`, `ubuntu`, `windows`, `macos[:version]`) to
/// `(image, os)`, or `None` for a plain reference; `env` looks up the
/// `CUA_IMAGE_<OS>` overrides ([`cua_image::canonical::alias_with`]).
pub fn canonical_alias(
    name: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, String)> {
    cua_image::canonical::alias_with(name, env).map(|(r, os)| (r, os.as_str().to_string()))
}

/// Whether `image` is one of the canonical images (`ghcr.io/trycua/{linux,
/// windows,macos}` in any tag or digest, an alias, or the current
/// `CUA_IMAGE_<OS>` override), which carry cua-spacesd.
pub fn is_canonical_image(image: &str) -> bool {
    cua_image::canonical::is_canonical(image)
}

/// [`resolve_fleet_image`], returning only the runtime.
pub async fn resolve_runtime(runtime: Option<RuntimeKind>, image: &str) -> Result<RuntimeKind> {
    Ok(resolve_fleet_image(runtime, image).await?.runtime)
}

/// The fallback runtime for an image whose manifest cannot be read, from
/// its reference alone: a tag that starts with `docker-` or contains
/// `-docker-` (the libs/images convention for the container rootfs build,
/// e.g. `docker-latest`, `cua-e2e-desktop-linux-docker-<sha>`) runs on
/// `gvisor`; anything else, including a bare `@sha256:` digest, on
/// `kubevirt`. A `repo:tag@sha256:…` reference is judged by its tag.
///
/// Only [`check_runtime`] uses it, and only when the manifest is unreadable
/// and no runtime was given. Shared vectors:
/// `tests/fixtures/image-variants.json` (`fallbacks`).
pub fn runtime_from_reference(image: &str) -> RuntimeKind {
    let last = image.rsplit('/').next().unwrap_or(image);
    let name_tag = last.split_once('@').map_or(last, |(n, _)| n);
    let tag = name_tag.split_once(':').map(|(_, t)| t).unwrap_or("");
    let tag = tag.to_ascii_lowercase();
    if tag.starts_with("docker-") || tag.contains("-docker-") {
        RuntimeKind::Gvisor
    } else {
        RuntimeKind::Kubevirt
    }
}

/// The tag of an image reference, or `None` when it carries none we can read.
/// Informational only: the runtime rule reads tags only as the
/// [`runtime_from_reference`] fallback.
///
/// A registry host port is not a tag, so only the final path segment counts,
/// and a digest pin (`repo@sha256:…`) has no tag at all.
pub fn image_tag(image: &str) -> Option<&str> {
    let last = image.rsplit('/').next().unwrap_or(image);
    if last.contains('@') {
        return None;
    }
    last.split_once(':').map(|(_, tag)| tag)
}

/// Parses a runtime name to create or claim with (`kubevirt`, `gvisor`,
/// case-insensitive). Empty is `None` (default from the image). `macos` is
/// [`Error::Unsupported`]: Fleet does not offer macOS in this SDK.
pub fn parse_runtime(value: &str) -> Result<Option<RuntimeKind>> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" => Ok(None),
        "kubevirt" => Ok(Some(RuntimeKind::Kubevirt)),
        "gvisor" => Ok(Some(RuntimeKind::Gvisor)),
        "macos" => Err(macos_unsupported()),
        other => Err(Error::InvalidArgument(format!(
            "unsupported runtime {other:?}: expected \"kubevirt\" or \"gvisor\""
        ))),
    }
}

/// The wire spelling of a runtime (`macos` included, for pools that already
/// have it).
pub fn runtime_name(runtime: &RuntimeKind) -> &'static str {
    match runtime {
        RuntimeKind::Kubevirt => "kubevirt",
        RuntimeKind::Gvisor => "gvisor",
        RuntimeKind::Macos => "macos",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str = "ghcr.io/trycua/cua-desktop-linux";

    fn known(v: ImageVariant) -> ImageEvidence {
        ImageEvidence::Known(v)
    }

    #[test]
    fn tags_are_read_from_the_last_segment_only() {
        assert_eq!(image_tag("repo/img:latest"), Some("latest"));
        assert_eq!(image_tag("registry:5000/img"), None);
        assert_eq!(image_tag("registry:5000/img:docker-1"), Some("docker-1"));
        assert_eq!(image_tag("img@sha256:abc"), None);
    }

    #[test]
    fn unset_runtime_follows_the_manifest_not_the_tag() {
        let cd = known(ImageVariant::ContainerDisk);
        let rootfs = known(ImageVariant::Rootfs);
        // A `docker-` tag that is really a containerDisk boots on kubevirt.
        assert_eq!(
            check_runtime(None, &format!("{DESKTOP}:docker-latest"), &cd).unwrap(),
            RuntimeKind::Kubevirt
        );
        // A plain tag that is really a rootfs runs on gVisor.
        assert_eq!(
            check_runtime(None, &format!("{DESKTOP}:latest"), &rootfs).unwrap(),
            RuntimeKind::Gvisor
        );
        let mac = ImageEvidence::Unsupported(MACOS_UNSUPPORTED.into());
        for rt in [None, Some(RuntimeKind::Kubevirt), Some(RuntimeKind::Gvisor)] {
            let e = check_runtime(rt, "r/mac:latest", &mac).unwrap_err();
            assert!(matches!(e, Error::Unsupported(_)), "{e}");
            assert!(e.to_string().contains("run locally with Lume"), "{e}");
        }
    }

    #[test]
    fn the_spaces_e2e_rootfs_tag_runs_on_gvisor() {
        // The tag that used to be refused for lacking a `docker-` prefix.
        let image = "123.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace:cua-e2e-desktop-linux-docker-abc123";
        let rootfs = known(ImageVariant::Rootfs);
        assert_eq!(
            check_runtime(Some(RuntimeKind::Gvisor), image, &rootfs).unwrap(),
            RuntimeKind::Gvisor
        );
        assert_eq!(
            check_runtime(None, image, &rootfs).unwrap(),
            RuntimeKind::Gvisor
        );
    }

    #[test]
    fn crossed_pairings_are_refused_from_the_manifest() {
        let e = check_runtime(
            Some(RuntimeKind::Gvisor),
            &format!("{DESKTOP}:docker-latest"),
            &known(ImageVariant::ContainerDisk),
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("containerDisk") && e.contains("\"kubevirt\""),
            "{e}"
        );
        let e = check_runtime(
            Some(RuntimeKind::Kubevirt),
            &format!("{DESKTOP}:latest"),
            &known(ImageVariant::Rootfs),
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("container rootfs") && e.contains("\"gvisor\""),
            "{e}"
        );
        // The `macos` runtime is refused whatever the image, even unread.
        for evidence in [
            known(ImageVariant::Rootfs),
            ImageEvidence::Unavailable("offline".into()),
        ] {
            let e = check_runtime(Some(RuntimeKind::Macos), "r/mac", &evidence).unwrap_err();
            assert!(matches!(e, Error::Unsupported(_)), "{e}");
        }
    }

    #[test]
    fn an_unreadable_manifest_falls_back_to_the_reference() {
        let off = ImageEvidence::Unavailable("registry: 401 unauthorized".into());
        // An explicit runtime wins and is sent unchecked.
        for rt in [RuntimeKind::Kubevirt, RuntimeKind::Gvisor] {
            assert_eq!(
                check_runtime(Some(rt.clone()), "r/img:docker-1", &off).unwrap(),
                rt
            );
        }
        // An unset one is guessed from the reference, never an error.
        assert_eq!(
            check_runtime(None, "r/img:docker-1", &off).unwrap(),
            RuntimeKind::Gvisor
        );
        assert_eq!(
            check_runtime(None, "img:plain", &off).unwrap(),
            RuntimeKind::Kubevirt
        );
        // The manifest, when readable, beats the tag.
        assert_eq!(
            check_runtime(None, "r/img:docker-1", &known(ImageVariant::ContainerDisk)).unwrap(),
            RuntimeKind::Kubevirt
        );
        let other = known(ImageVariant::Other);
        assert!(check_runtime(None, "r/lume:latest", &other).is_err());
        assert_eq!(
            check_runtime(Some(RuntimeKind::Kubevirt), "r/lume:latest", &other).unwrap(),
            RuntimeKind::Kubevirt
        );
    }

    #[test]
    fn variants_round_trip() {
        for v in [
            ImageVariant::ContainerDisk,
            ImageVariant::Rootfs,
            ImageVariant::Other,
        ] {
            assert_eq!(ImageVariant::parse(v.as_str()).unwrap(), v);
        }
        assert!(matches!(
            ImageVariant::parse("macos"),
            Err(Error::Unsupported(_))
        ));
        assert!(ImageVariant::parse("vm").is_err());
    }

    #[test]
    fn runtime_names_round_trip() {
        for rt in [RuntimeKind::Kubevirt, RuntimeKind::Gvisor] {
            assert_eq!(parse_runtime(runtime_name(&rt)).unwrap(), Some(rt));
        }
        // Existing `macos` pools still read and print; creating is refused.
        assert_eq!(runtime_name(&RuntimeKind::Macos), "macos");
        assert!(matches!(parse_runtime("macos"), Err(Error::Unsupported(_))));
        assert!(ensure_runtime_offered(&RuntimeKind::Macos).is_err());
        let pool: RuntimeKind = serde_json::from_str("\"macos\"").unwrap();
        assert_eq!(pool, RuntimeKind::Macos);
        assert_eq!(
            parse_runtime(" GVISOR ").unwrap(),
            Some(RuntimeKind::Gvisor)
        );
        assert_eq!(parse_runtime("").unwrap(), None);
        assert!(parse_runtime("firecracker").is_err());
    }
}
