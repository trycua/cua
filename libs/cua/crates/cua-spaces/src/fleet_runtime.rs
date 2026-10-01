//! The Fleet runtime ↔ image pairing for Spaces.
//!
//! The two Fleet runtimes consume different artifacts: `kubevirt` (the
//! default) boots a VM from a **KubeVirt containerDisk**, `gvisor` runs a
//! runsc pod from a **container rootfs**. Crossing them fails on the cluster
//! only after a multi-GB pull, and then opaquely. What an image is comes from
//! its registry manifest (never its tag); the rule lives in
//! `cua_fleet::runtime` (shared with the SDK's sandboxes and cua-sandbox) and
//! this module maps the Spaces contract's runtime onto it. Every Spaces path
//! that claims on Fleet goes through [`resolve`].

use crate::error::{Error, Result};
use cua_fleet::RuntimeKind;
use cua_spaces_contract::inputs::FleetRuntime;

/// The default Space image: the canonical Linux image
/// (`ghcr.io/trycua/linux:24.04`, or `CUA_IMAGE_LINUX`). The one resolver
/// picks the variant `runtime` needs (`-disk` for kubevirt).
pub fn default_image(_runtime: FleetRuntime) -> String {
    cua_fleet::canonical_image("linux")
}

/// The tag of an image reference (see [`cua_fleet::runtime::image_tag`]).
/// Informational only: the runtime rule never reads tags.
pub fn image_tag(image: &str) -> Option<&str> {
    cua_fleet::runtime::image_tag(image)
}

/// Validates the runtime/image pairing against the image's registry manifest
/// and returns the Fleet runtime to send. Spaces default to `kubevirt` when
/// no runtime is given; the rule itself is [`cua_fleet::resolve_runtime`]
/// (the one copy).
pub async fn resolve(runtime: Option<FleetRuntime>, image: &str) -> Result<RuntimeKind> {
    Ok(
        resolve_image(runtime.or(Some(FleetRuntime::Kubevirt)), image)
            .await?
            .runtime,
    )
}

/// [`resolve`] returning the whole resolution (the pinned image and, when
/// the registry was read, its guest OS). `None` lets the image's manifest
/// pick the runtime.
pub async fn resolve_image(
    runtime: Option<FleetRuntime>,
    image: &str,
) -> Result<cua_fleet::FleetImage> {
    let kind = runtime.map(|r| match r {
        FleetRuntime::Kubevirt => RuntimeKind::Kubevirt,
        FleetRuntime::Gvisor => RuntimeKind::Gvisor,
    });
    cua_fleet::resolve_fleet_image(kind, image)
        .await
        .map_err(|e| match e {
            cua_fleet::Error::InvalidArgument(m) => Error::invalid(m),
            other => Error::invalid(other.to_string()),
        })
}

/// The guest OS (`linux`, `windows`, `macos`) of a Space image: the image
/// catalog's `os` first, else the resolved image's (its `ai.cua.image.os`
/// annotation or label, else its OCI platform), else, only when neither is
/// known, a `windows` in the reference.
pub fn guest_os(image: &str, resolved_os: Option<&str>) -> String {
    guest_os_with(image, cua_image::catalog::catalog_os(image), resolved_os)
}

/// [`guest_os`] with the catalog lookup already done (tests).
pub fn guest_os_with(image: &str, catalog_os: Option<String>, resolved_os: Option<&str>) -> String {
    if let Some(os) = catalog_os.filter(|o| !o.is_empty()) {
        return os;
    }
    if let Some(os) = resolved_os.filter(|o| !o.is_empty()) {
        return os.to_string();
    }
    if image.to_ascii_lowercase().contains("windows") {
        "windows".into()
    } else {
        "linux".into()
    }
}

/// [`resolve`] with the image evidence already read.
pub fn check(
    runtime: Option<FleetRuntime>,
    image: &str,
    evidence: &cua_fleet::ImageEvidence,
) -> Result<RuntimeKind> {
    let kind = match runtime.unwrap_or(FleetRuntime::Kubevirt) {
        FleetRuntime::Kubevirt => RuntimeKind::Kubevirt,
        FleetRuntime::Gvisor => RuntimeKind::Gvisor,
    };
    cua_fleet::check_runtime(Some(kind), image, evidence).map_err(|e| match e {
        cua_fleet::Error::InvalidArgument(m) => Error::invalid(m),
        other => Error::invalid(other.to_string()),
    })
}

/// Parses the wire spelling of a runtime (`kubevirt` / `gvisor`), refusing
/// anything else with the same message the Python server used.
pub fn parse_runtime(value: &str) -> Result<FleetRuntime> {
    match value.trim() {
        "" | "kubevirt" => Ok(FleetRuntime::Kubevirt),
        "gvisor" => Ok(FleetRuntime::Gvisor),
        other => Err(Error::invalid(format!(
            "unsupported runtime {other:?} — expected \"kubevirt\" or \"gvisor\""
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_fleet::{ImageEvidence, ImageVariant};

    #[test]
    fn guest_os_prefers_catalog_then_resolved_then_the_name() {
        // Catalog os=windows for a ref that does not say "windows".
        assert_eq!(
            guest_os_with(
                "registry.example/desk:2022",
                Some("windows".into()),
                Some("linux")
            ),
            "windows"
        );
        // The image's label (resolved os) says linux: a "windows" in the
        // name does not make it Windows.
        assert_eq!(
            guest_os_with("ghcr.io/me/windows-tools:1", None, Some("linux")),
            "linux"
        );
        // The resolved OCI platform os.
        assert_eq!(
            guest_os_with("registry.example/app:1", None, Some("windows")),
            "windows"
        );
        // Neither known: the reference heuristic.
        assert_eq!(guest_os_with("ghcr.io/me/windows:x", None, None), "windows");
        assert_eq!(guest_os_with("registry.example/app:1", None, None), "linux");
        // The built-in catalog decides for a pinned canonical Windows ref.
        let pinned = format!("ghcr.io/trycua/windows@sha256:{}", "b".repeat(64));
        assert_eq!(guest_os(&pinned, Some("linux")), "windows");
    }

    const E2E: &str = "123456789012.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace:cua-e2e-desktop-linux-docker-0123abcd";

    #[test]
    fn tags_are_read_from_the_last_segment_only() {
        assert_eq!(image_tag("repo/img:latest"), Some("latest"));
        assert_eq!(image_tag("registry:5000/img"), None);
        assert_eq!(image_tag("registry:5000/img:docker-1"), Some("docker-1"));
        assert_eq!(image_tag("img@sha256:abc"), None);
    }

    #[test]
    fn kubevirt_takes_a_containerdisk_whatever_the_tag() {
        let cd = ImageEvidence::Known(ImageVariant::ContainerDisk);
        assert_eq!(
            check(None, "r/img:latest", &cd).unwrap(),
            RuntimeKind::Kubevirt
        );
        assert_eq!(
            check(None, "r/img:docker-latest", &cd).unwrap(),
            RuntimeKind::Kubevirt
        );
        let e = check(Some(FleetRuntime::Gvisor), "r/img:docker-latest", &cd).unwrap_err();
        assert!(e.to_string().contains("containerDisk"), "{e}");
    }

    #[test]
    fn gvisor_takes_a_rootfs_whatever_the_tag() {
        let rootfs = ImageEvidence::Known(ImageVariant::Rootfs);
        // The e2e desktop tag has no `docker-` prefix; it used to be refused.
        assert_eq!(
            check(Some(FleetRuntime::Gvisor), E2E, &rootfs).unwrap(),
            RuntimeKind::Gvisor
        );
        let e = check(Some(FleetRuntime::Kubevirt), "r/img:latest", &rootfs).unwrap_err();
        assert!(e.to_string().contains("container rootfs"), "{e}");
    }

    #[test]
    fn an_unreadable_manifest_sends_the_runtime_unchecked() {
        let off = ImageEvidence::Unavailable("registry: 401".into());
        assert_eq!(
            check(Some(FleetRuntime::Gvisor), E2E, &off).unwrap(),
            RuntimeKind::Gvisor
        );
        assert_eq!(check(None, E2E, &off).unwrap(), RuntimeKind::Kubevirt);
    }

    #[test]
    fn defaults_pair_correctly() {
        let cd = ImageEvidence::Known(ImageVariant::ContainerDisk);
        let rootfs = ImageEvidence::Known(ImageVariant::Rootfs);
        assert!(
            check(
                Some(FleetRuntime::Kubevirt),
                &default_image(FleetRuntime::Kubevirt),
                &cd
            )
            .is_ok()
        );
        assert!(
            check(
                Some(FleetRuntime::Gvisor),
                &default_image(FleetRuntime::Gvisor),
                &rootfs
            )
            .is_ok()
        );
    }

    #[test]
    fn unknown_runtimes_are_refused() {
        assert!(parse_runtime("firecracker").is_err());
        assert_eq!(parse_runtime("").unwrap(), FleetRuntime::Kubevirt);
    }
}
