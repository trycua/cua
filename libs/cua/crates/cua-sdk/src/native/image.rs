//! The one image resolver and the canonical images, for every binding.
//!
//! `resolve_image("python:3.12-slim", "local", None)` normalises the ref
//! (short refs are docker.io), reads its registry documents with the
//! registry auth chain, picks the variant the backend runs (`rootfs`,
//! `containerdisk`, `lume`), following the `ai.cua.image.variants`
//! annotation and the `-disk` sibling tags, and pins it by digest.
//! `canonical_image("linux", None)` is `ghcr.io/trycua/linux:24.04` (or
//! `CUA_IMAGE_LINUX`); Python's `Image.linux()`, the TS/Swift/Kotlin
//! helpers and the CLI aliases all come from here. `canonical_image_tier`
//! picks a tier (`slim`, full, macOS `xcode`) and `omarchy_image` the
//! Omarchy distribution image; both refuse catalog entries CI has not
//! published (`ImageNotPublished`).

use crate::{CuaError, Result};

/// A reference resolved for a backend.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ResolvedImage {
    /// The reference as given, normalised (`docker.io/library/python:3.12-slim`).
    pub reference: String,
    /// The chosen variant by tag (the input or its sibling). Lume pulls this.
    pub variant_ref: String,
    /// `registry/repo@sha256:…` of the chosen variant.
    pub pinned_ref: String,
    /// The digest in `pinned_ref`.
    pub digest: String,
    /// `rootfs`, `containerdisk` or `lume`.
    pub variant: String,
    /// Architecture to run (`amd64`/`arm64`).
    pub arch: Option<String>,
    /// Architectures the variant offers.
    pub architectures: Vec<String>,
    /// Whether `arch` differs from the requested one (runs emulated).
    pub emulated: bool,
    /// Guest OS: `linux`, `windows` or `macos`.
    pub os: String,
    /// Whether the image carries cua-spacesd (`None`: cannot tell).
    pub spacesd: Option<bool>,
}

impl From<cua_image::ResolvedImage> for ResolvedImage {
    fn from(r: cua_image::ResolvedImage) -> Self {
        Self {
            reference: r.reference,
            variant_ref: r.variant_ref,
            pinned_ref: r.pinned_ref,
            digest: r.digest,
            variant: r.variant.as_str().into(),
            arch: r.arch,
            architectures: r.architectures,
            emulated: r.emulated,
            os: r.os,
            spacesd: r.spacesd,
        }
    }
}

fn image_error(e: cua_image::ImageError) -> CuaError {
    use cua_image::ImageError as E;
    match e {
        E::NotFound(m) => CuaError::NotFound(m),
        E::Unauthorized(m) => CuaError::Unauthenticated(m),
        E::UnsupportedVariant { .. } => CuaError::Unsupported(e.to_string()),
        E::Reference(..) => CuaError::InvalidArgument(e.to_string()),
        E::NotPublished { .. } => CuaError::ImageNotPublished(e.to_string()),
        other => CuaError::Runtime(other.to_string()),
    }
}

/// Resolves `reference` for `backend` (`auto`, `local`, `fleet`,
/// `container`/`docker`/`gvisor`, `vm`/`qemu`/`kubevirt`, `lume`)
/// on `arch` (`amd64`/`arm64`; `None` = this host, or amd64 for `fleet`).
/// `reference` is literal (`ubuntu:24.04` is Docker Hub's image): no
/// alias applies here; use [`image_alias`] or [`canonical_image`] first.
/// Raises `NotFound` (no such image: e.g. a tag only the local engine has),
/// `Unauthenticated` (the registry refused the credentials), `Unsupported`
/// (no variant the backend runs, e.g. a macOS image on Fleet).
/// Blocks while the registry is read (cached for five minutes).
#[uniffi::export]
pub fn resolve_image(
    reference: String,
    backend: String,
    arch: Option<String>,
) -> Result<ResolvedImage> {
    let backend = cua_image::Backend::parse(&backend).map_err(image_error)?;
    let arch = arch.unwrap_or_else(|| {
        if backend == cua_image::Backend::Fleet {
            "amd64".into()
        } else {
            host_arch().into()
        }
    });
    // A plain thread: the caller may itself be on a Tokio runtime.
    let resolved = std::thread::spawn(move || {
        super::runtime().block_on(cua_image::resolve::resolve(&reference, backend, &arch))
    })
    .join()
    .map_err(|_| CuaError::Internal("resolve_image panicked".into()))?
    .map_err(image_error)?;
    Ok(resolved.into())
}

/// [`resolve_image`] for a private image: `secret` joins the head of the
/// registry auth chain (it is read in this process and never cached or
/// logged). Same errors as [`resolve_image`].
#[uniffi::export]
pub fn resolve_image_with_secret(
    reference: String,
    backend: String,
    arch: Option<String>,
    secret: super::RegistrySecret,
) -> Result<ResolvedImage> {
    let backend = cua_image::Backend::parse(&backend).map_err(image_error)?;
    let arch = arch.unwrap_or_else(|| {
        if backend == cua_image::Backend::Fleet {
            "amd64".into()
        } else {
            host_arch().into()
        }
    });
    let resolved = std::thread::spawn(move || {
        super::runtime().block_on(async {
            let creds = secret.credentials(&reference).await?;
            cua_image::resolve::resolve_with_credentials(&reference, backend, &arch, Some(&creds))
                .await
                .map_err(image_error)
        })
    })
    .join()
    .map_err(|_| CuaError::Internal("resolve_image panicked".into()))??;
    Ok(resolved.into())
}

/// Normalises a reference like the docker CLI (`python` is
/// `docker.io/library/python:latest`). No registry read.
#[uniffi::export]
pub fn normalize_image(reference: String) -> Result<String> {
    cua_image::resolve::normalize(&reference).map_err(image_error)
}

/// The canonical image for `os` (`linux`, `windows`, `macos`) and
/// `version` (`None` = default: `ghcr.io/trycua/linux:24.04`,
/// `ghcr.io/trycua/windows:2022`, `ghcr.io/trycua/macos:26`; macOS accepts
/// `tahoe`/`sequoia`). `CUA_IMAGE_<OS>` overrides the default version.
#[uniffi::export]
pub fn canonical_image(os: String, version: Option<String>) -> Result<String> {
    cua_image::canonical::canonical_image(&os, version.as_deref()).map_err(image_error)
}

/// The canonical image for `os`, `version` and `tier`: `slim` (the minimum
/// that passes `cua-spacesd doctor --strict`; CI and benchmarks use it),
/// `full` or `None` (the default, with dev tooling) or, on macOS, `xcode`
/// / `xcode-<X.Y>` (full plus one pinned Xcode). The tag is
/// `<os-version>[-<tier>]` (`ghcr.io/trycua/linux:24.04-slim`); the
/// resolver finds its `-disk` sibling. `CUA_IMAGE_<OS>` overrides only the
/// default full image. Raises `ImageNotPublished` for a tier CI has not
/// published yet, `InvalidArgument` for an unknown tier or `xcode`
/// outside macOS.
#[uniffi::export]
pub fn canonical_image_tier(
    os: String,
    version: Option<String>,
    tier: Option<String>,
) -> Result<String> {
    cua_image::canonical::tier_image(&os, version.as_deref(), tier.as_deref()).map_err(image_error)
}

/// The Omarchy image, `ghcr.io/trycua/omarchy:<channel>` (default `edge`;
/// also `rc`, `stable`): Omarchy (Arch Linux, Hyprland) with cua-spacesd,
/// as an amd64 VM (QEMU locally, KubeVirt in the cloud; emulated on arm64
/// hosts). Raises `ImageNotPublished` until CI publishes the channel; pass
/// the reference explicitly to use it before then.
#[uniffi::export]
pub fn omarchy_image(channel: Option<String>) -> Result<String> {
    cua_image::canonical::omarchy_image(channel.as_deref()).map_err(image_error)
}

/// The canonical image a CLI alias (`linux`, `windows`, `macos[:version]`,
/// bare `ubuntu`) names, or `None` for a plain reference (`ubuntu:24.04`
/// is Docker Hub's image).
#[uniffi::export]
pub fn image_alias(name: String) -> Option<String> {
    cua_image::canonical::alias(&name)
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_uses_the_installed_registry() {
        let mut r = cua_image::testing::FakeRegistry::default();
        let disk = r.index(
            "ghcr.io/trycua/linux:24.04-disk",
            &["amd64", "arm64"],
            true,
            None,
        );
        r.index(
            "ghcr.io/trycua/linux:24.04",
            &["amd64", "arm64"],
            false,
            None,
        );
        let hub = r.index(
            "docker.io/library/ubuntu:24.04",
            &["amd64", "arm64"],
            false,
            None,
        );
        cua_image::resolve::set_source(Some(std::sync::Arc::new(r)));
        unsafe { std::env::remove_var("CUA_IMAGE_LINUX") };
        let got = resolve_image(
            "ghcr.io/trycua/linux:24.04".into(),
            "vm".into(),
            Some("arm64".into()),
        )
        .unwrap();
        assert_eq!(got.variant, "containerdisk");
        assert_eq!(got.pinned_ref, format!("ghcr.io/trycua/linux@{disk}"));
        // Literal: `ubuntu:24.04` is Docker Hub's image, never the alias.
        let got = resolve_image("ubuntu:24.04".into(), "container".into(), None).unwrap();
        assert_eq!(got.pinned_ref, format!("docker.io/library/ubuntu@{hub}"));
        assert!(resolve_image("linux".into(), "local".into(), None).is_err());
        let e = resolve_image("nope:1".into(), "local".into(), None).unwrap_err();
        assert!(matches!(e, CuaError::NotFound(_)), "{e:?}");
        assert!(matches!(
            resolve_image("x".into(), "kvm".into(), None),
            Err(CuaError::InvalidArgument(_))
        ));
        cua_image::resolve::set_source(None);
        assert_eq!(
            normalize_image("python:3.12-slim".into()).unwrap(),
            "docker.io/library/python:3.12-slim"
        );
        assert_eq!(
            canonical_image_tier("linux".into(), None, None).unwrap(),
            "ghcr.io/trycua/linux:24.04"
        );
        assert!(matches!(
            canonical_image_tier("linux".into(), None, Some("tiny".into())),
            Err(CuaError::InvalidArgument(_))
        ));
        assert!(matches!(
            canonical_image_tier("windows".into(), None, Some("xcode".into())),
            Err(CuaError::InvalidArgument(_))
        ));
        let published = |r: &str| cua_image::catalog::find(r).is_none_or(|e| e.published);
        for (got, want) in [
            (
                canonical_image_tier("linux".into(), None, Some("slim".into())),
                "ghcr.io/trycua/linux:24.04-slim",
            ),
            (
                canonical_image_tier("macos".into(), None, Some("xcode".into())),
                "ghcr.io/trycua/macos:26-xcode",
            ),
            (omarchy_image(None), "ghcr.io/trycua/omarchy:edge"),
        ] {
            if published(want) {
                assert_eq!(got.unwrap(), want);
            } else {
                let e = got.unwrap_err();
                assert!(
                    matches!(&e, CuaError::ImageNotPublished(m) if m.contains(want)),
                    "{e:?}"
                );
            }
        }
        assert_eq!(image_alias("python".into()), None);
        assert_eq!(image_alias("ubuntu:24.04".into()), None);
    }
}
