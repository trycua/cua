//! What kind of image a registry reference is, so a Fleet image ref runs on
//! the right local backend without the caller saying how.
//!
//! | registry content | [`ImageKind`] | local backend |
//! |---|---|---|
//! | `FROM scratch` + `/disk/disk.img` or trycua containerDisk media types (KubeVirt containerDisk, Fleet `runtime: kubevirt`); see [`container_disk_evidence`] | `ContainerDisk` | QEMU |
//! | any other plain OCI image (Fleet `runtime: gvisor`) | `Rootfs` | container (gVisor) |
//! | Lume / Tart / agoda VM media types | `Lume` | Lume |
//! | any other darwin-platform image (Fleet `runtime: macos`) | `Macos` | none |

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::manifest::{ImageIndex, ImageManifest, Manifest, oci_arch};
use crate::media_types::{ImageFormat, container_disk_evidence, detect_format};
use crate::registry::{ManifestSource, RegistryClient, parse_ref};

/// Kind of image behind a reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    /// KubeVirt containerDisk: boot `/disk/disk.img` as a VM.
    ContainerDisk,
    /// Container rootfs (gVisor/runc).
    Rootfs,
    /// A Lume-family VM image (macOS or Linux on Apple Virtualization).
    Lume,
    /// A darwin-platform image in no Lume-family format. No local backend
    /// runs it.
    Macos,
    /// A cua QEMU disk artifact or anything else unrecognised.
    Other,
}

/// Result of [`inspect`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInspection {
    /// The reference that was inspected.
    pub reference: String,
    /// Kind.
    pub kind: ImageKind,
    /// Linux architectures the reference offers (OCI names, `amd64`/`arm64`).
    /// Empty for single-platform manifests whose config does not say.
    pub architectures: Vec<String>,
    /// The architecture that was inspected (host arch when offered).
    pub arch: Option<String>,
}

impl ImageInspection {
    /// Architecture to run: the host's when offered, else the first one.
    pub fn run_arch(&self, host: &str) -> Option<String> {
        let host = oci_arch(host);
        if self.architectures.iter().any(|a| a == host) {
            return Some(host.to_string());
        }
        self.architectures.first().cloned().or(self.arch.clone())
    }
}

const MACOS_OS: &[&str] = &["darwin", "macos"];

/// Classify from the documents (pure; see [`inspect`] for the fetching).
/// `index` is the index the reference resolved to, if any; `manifest` the
/// selected platform manifest; `config` its config JSON.
pub fn classify(
    _reference: &str,
    index: Option<&ImageIndex>,
    manifest: Option<&ImageManifest>,
    config: Option<&serde_json::Value>,
) -> ImageKind {
    let index_platforms = || {
        index
            .into_iter()
            .flat_map(|i| i.manifests.iter())
            .filter_map(|d| d.platform.as_ref())
    };
    if let Some(m) = manifest {
        match detect_format(m) {
            ImageFormat::LumeOci
            | ImageFormat::LumeLegacy
            | ImageFormat::Agoda
            | ImageFormat::Tart => {
                return ImageKind::Lume;
            }
            ImageFormat::Qemu => return ImageKind::Other,
            _ => {}
        }
    }
    let darwin_config = config
        .and_then(|c| c.get("os"))
        .and_then(|o| o.as_str())
        .is_some_and(|o| MACOS_OS.contains(&o.to_ascii_lowercase().as_str()));
    let darwin_index = index_platforms()
        .any(|p| MACOS_OS.contains(&p.os.to_ascii_lowercase().as_str()))
        && !index_platforms().any(|p| p.os == "linux");
    // A darwin image in no Lume-family format (those are recognised above).
    if darwin_config || darwin_index {
        return ImageKind::Macos;
    }
    if container_disk_evidence(manifest, config).is_some() {
        return ImageKind::ContainerDisk;
    }
    if config.is_some() || manifest.is_some() {
        return ImageKind::Rootfs;
    }
    // Nothing fetched: the reference (its tag included) is not evidence.
    ImageKind::Other
}

fn linux_arches(index: &ImageIndex) -> Vec<String> {
    let mut out: Vec<String> = index
        .manifests
        .iter()
        .filter(|d| {
            !d.annotations
                .get("vnd.docker.reference.type")
                .is_some_and(|t| t.contains("attestation"))
        })
        .filter_map(|d| d.platform.as_ref())
        .filter(|p| p.os == "linux")
        .map(|p| oci_arch(&p.architecture).to_string())
        .collect();
    out.dedup();
    out
}

/// Fetch the manifest (and config) of `reference` and classify it. For an
/// index, the `host_arch` platform is inspected when present, else the first
/// Linux one; a darwin-only index is reported without further fetches.
pub async fn inspect(
    client: &RegistryClient,
    reference: &str,
    host_arch: &str,
) -> Result<ImageInspection> {
    inspect_with(client, reference, host_arch).await
}

/// [`inspect`] over any [`ManifestSource`] (a fake registry in tests).
pub async fn inspect_with(
    client: &dyn ManifestSource,
    reference: &str,
    host_arch: &str,
) -> Result<ImageInspection> {
    Ok(inspect_full(client, reference, host_arch).await?.inspection)
}

/// Everything [`inspect_with`] read, for the resolver.
pub(crate) struct FullInspection {
    pub inspection: ImageInspection,
    /// Digest of the top-level document (index or manifest).
    pub digest: String,
    /// The top-level index, when the reference is one.
    pub index: Option<ImageIndex>,
    /// The inspected platform manifest.
    pub manifest: Option<ImageManifest>,
    /// Its config.
    pub config: Option<serde_json::Value>,
}

pub(crate) async fn inspect_full(
    client: &dyn ManifestSource,
    reference: &str,
    host_arch: &str,
) -> Result<FullInspection> {
    let r = parse_ref(reference)?;
    let repo = format!("{}/{}", r.resolve_registry(), r.repository());
    let (top, top_digest) = client.manifest(reference).await?;
    let (index, manifest, architectures) = match top {
        Manifest::Image(m) => (None, m, Vec::new()),
        Manifest::Index(idx) => {
            let arches = linux_arches(&idx);
            let host = oci_arch(host_arch);
            let pick = arches
                .iter()
                .find(|a| *a == host)
                .or(arches.first())
                .cloned();
            let Some(arch) = pick else {
                // No Linux platform at all (darwin or unknown).
                let kind = classify(reference, Some(&idx), None, None);
                let arch = idx
                    .manifests
                    .iter()
                    .filter_map(|d| d.platform.as_ref())
                    .map(|p| oci_arch(&p.architecture).to_string())
                    .next();
                return Ok(FullInspection {
                    inspection: ImageInspection {
                        reference: reference.into(),
                        kind,
                        architectures: arches,
                        arch,
                    },
                    digest: top_digest,
                    index: Some(idx),
                    manifest: None,
                    config: None,
                });
            };
            let entry = crate::manifest::select_platform(&idx, "linux", &arch)?;
            let (child, _) = client.manifest(&format!("{repo}@{}", entry.digest)).await?;
            let m = match child {
                Manifest::Image(m) => m,
                Manifest::Index(_) => ImageManifest::default(),
            };
            (Some(idx), m, arches)
        }
    };
    let config = match &manifest.config {
        Some(c) if c.media_type.contains("json") && c.size < (4 << 20) => {
            let bytes = client.blob(&format!("{repo}@{}", c.digest), c).await?;
            serde_json::from_slice::<serde_json::Value>(&bytes).ok()
        }
        _ => None,
    };
    let arch = config
        .as_ref()
        .and_then(|c| c.get("architecture"))
        .and_then(|a| a.as_str())
        .map(|a| oci_arch(a).to_string());
    let architectures = if architectures.is_empty() {
        arch.iter().cloned().collect()
    } else {
        architectures
    };
    let kind = classify(reference, index.as_ref(), Some(&manifest), config.as_ref());
    Ok(FullInspection {
        inspection: ImageInspection {
            reference: reference.into(),
            kind,
            architectures,
            arch,
        },
        digest: top_digest,
        index,
        manifest: Some(manifest),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Descriptor, Platform};
    use serde_json::json;

    fn plat(os: &str, arch: &str) -> Descriptor {
        Descriptor {
            media_type: "application/vnd.oci.image.manifest.v1+json".into(),
            digest: "sha256:0".into(),
            platform: Some(Platform {
                os: os.into(),
                architecture: arch.into(),
                variant: None,
            }),
            ..Default::default()
        }
    }

    fn manifest(config_mt: &str, layer_mt: &str) -> ImageManifest {
        ImageManifest {
            config: Some(Descriptor {
                media_type: config_mt.into(),
                ..Default::default()
            }),
            layers: vec![Descriptor {
                media_type: layer_mt.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    const OCI_CFG: &str = "application/vnd.oci.image.config.v1+json";
    const GZ: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

    #[test]
    fn fleet_containerdisk_is_detected_from_its_config() {
        // Verbatim shape of public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34.
        let cfg = json!({"os": "linux", "architecture": "amd64", "config": {},
            "history": [{"created_by": "COPY disk.img /disk/disk.img # buildkit"}]});
        let idx = ImageIndex {
            manifests: vec![plat("linux", "amd64"), plat("unknown", "unknown")],
            ..Default::default()
        };
        let k = classify(
            "r:main",
            Some(&idx),
            Some(&manifest(OCI_CFG, GZ)),
            Some(&cfg),
        );
        assert_eq!(k, ImageKind::ContainerDisk);
        // Windows images use ADD --chown=107:107.
        let cfg = json!({"os": "linux", "config": {},
            "history": [{"created_by": "ADD --chown=107:107 disk.img /disk/disk.img # buildkit"}]});
        assert_eq!(
            classify("r", None, Some(&manifest(OCI_CFG, GZ)), Some(&cfg)),
            ImageKind::ContainerDisk
        );
    }

    #[test]
    fn gvisor_rootfs_is_detected() {
        let cfg = json!({"os": "linux", "config": {"Cmd": ["/usr/bin/supervisord", "-n"]},
            "history": [{"created_by": "CMD [\"/usr/bin/supervisord\"]"}]});
        assert_eq!(
            classify(
                "r:docker-latest",
                None,
                Some(&manifest(OCI_CFG, GZ)),
                Some(&cfg)
            ),
            ImageKind::Rootfs
        );
        // Nothing fetched: the tag is not evidence either way.
        assert_eq!(
            classify("repo:docker-main-1", None, None, None),
            ImageKind::Other
        );
        assert_eq!(classify("repo:main-1", None, None, None), ImageKind::Other);
    }

    #[test]
    fn non_lume_darwin_images_are_macos() {
        // darwin-only index.
        let idx = ImageIndex {
            manifests: vec![plat("darwin", "arm64")],
            ..Default::default()
        };
        assert_eq!(classify("r", Some(&idx), None, None), ImageKind::Macos);
        // darwin config: not a container rootfs.
        let cfg = json!({"os": "darwin", "architecture": "arm64", "config": {"Cmd": ["/bin/zsh"]}});
        assert_eq!(
            classify("r", None, Some(&manifest(OCI_CFG, GZ)), Some(&cfg)),
            ImageKind::Macos
        );
    }

    #[test]
    fn lume_images_stay_lume() {
        let m = manifest(
            crate::media_types::LUME_CONFIG,
            crate::media_types::LUME_DISK,
        );
        // A darwin platform on a Lume image is still Lume.
        assert_eq!(classify("r", None, Some(&m), None), ImageKind::Lume);
    }

    #[test]
    fn run_arch_prefers_the_host() {
        let i = ImageInspection {
            reference: "r".into(),
            kind: ImageKind::ContainerDisk,
            architectures: vec!["amd64".into(), "arm64".into()],
            arch: Some("arm64".into()),
        };
        assert_eq!(i.run_arch("aarch64").as_deref(), Some("arm64"));
        assert_eq!(i.run_arch("x86_64").as_deref(), Some("amd64"));
        let only = ImageInspection {
            architectures: vec!["amd64".into()],
            ..i
        };
        assert_eq!(only.run_arch("aarch64").as_deref(), Some("amd64"));
    }

    /// Live, read-only: the public Fleet catalog refs classify as expected.
    #[tokio::test]
    async fn live_fleet_refs_classify() {
        if std::env::var("CUA_E2E_IMAGE").as_deref() != Ok("1") {
            eprintln!("skipping: set CUA_E2E_IMAGE=1");
            return;
        }
        let c = RegistryClient::default();
        let i = inspect(
            &c,
            "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:main-38352d34",
            "arm64",
        )
        .await
        .unwrap();
        assert_eq!(i.kind, ImageKind::ContainerDisk);
        assert_eq!(i.architectures, vec!["amd64"]);
        let i = inspect(
            &c,
            "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest",
            "arm64",
        )
        .await
        .unwrap();
        assert_eq!(i.kind, ImageKind::Rootfs);
        assert_eq!(i.arch.as_deref(), Some("arm64"));
    }
}
