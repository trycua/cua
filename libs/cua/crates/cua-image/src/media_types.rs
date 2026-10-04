//! OCI media types for VM and container images, and format detection.
//!
//! Port of `cua_sandbox/registry/{media_types,manifest}.py`, with the Lume
//! media types taken from `libs/lume/src/ContainerRegistry/ImageContainerRegistry.swift`.
//!
//! | format | how it is recognised |
//! |---|---|
//! | Lume OCI (`lume push --single-layer` / multi-part) | `application/vnd.trycua.lume.{config,disk,nvram}.v1*` |
//! | Lume legacy | `application/octet-stream+lz4` chunks, `…;part.number=N;part.total=M` |
//! | agoda macosvz | `application/vnd.agoda.macosvz.*` |
//! | Tart | `application/vnd.cirruslabs.tart.*` |
//! | QEMU (cua) | `application/vnd.trycua.qemu.*` |
//! | KubeVirt containerDisk | plain OCI image whose single layer holds `disk/disk.img` |
//! | container rootfs | plain OCI/Docker image layers |

use serde::{Deserialize, Serialize};

use crate::manifest::{Descriptor, ImageManifest};

// ── Lume (libs/lume OCIMediaType / OCIAnnotation) ─────────────────────────────
pub const LUME_CONFIG: &str = "application/vnd.trycua.lume.config.v1+json";
pub const LUME_DISK: &str = "application/vnd.trycua.lume.disk.v1";
pub const LUME_NVRAM: &str = "application/vnd.trycua.lume.nvram.v1";
pub const LUME_LEGACY_DISK_CHUNK: &str = "application/vnd.trycua.lume.disk.chunk.lz4";
pub const LUME_LEGACY_AUX: &str = "application/vnd.trycua.lume.aux.image.v1";
pub const LUME_LZ4_PART: &str = "application/octet-stream+lz4";
pub const LUME_LZFSE_PART_PREFIX: &str = "application/octet-stream+lzfse";
pub const LUME_ANNOTATION_PART_NUMBER: &str = "org.trycua.lume.part.number";
pub const LUME_ANNOTATION_PART_TOTAL: &str = "org.trycua.lume.part.total";
pub const LUME_ANNOTATION_PART_OFFSET: &str = "org.trycua.lume.part.offset";
pub const LUME_ANNOTATION_CHUNK_SIZE: &str = "org.trycua.lume.chunk.size";
pub const LUME_ANNOTATION_OS: &str = "org.trycua.lume.os";

// ── agoda macosvz (older ghcr images) ────────────────────────────────────────
pub const AGODA_CONFIG: &str = "application/vnd.agoda.macosvz.config.v1+json";
pub const AGODA_DISK: &str = "application/vnd.agoda.macosvz.disk.image.v1";
pub const AGODA_AUX: &str = "application/vnd.agoda.macosvz.aux.image.v1";

// ── Tart ─────────────────────────────────────────────────────────────────────
pub const TART_CONFIG: &str = "application/vnd.cirruslabs.tart.config.v1";
pub const TART_DISK: &str = "application/vnd.cirruslabs.tart.disk.v2";
pub const TART_NVRAM: &str = "application/vnd.cirruslabs.tart.nvram.v1";

// ── QEMU (cua) ───────────────────────────────────────────────────────────────
pub const QEMU_CONFIG: &str = "application/vnd.trycua.qemu.config.v1+json";
pub const QEMU_DISK: &str = "application/vnd.trycua.qemu.disk.v1";
pub const QEMU_DISK_GZIP: &str = "application/vnd.trycua.qemu.disk.v1+gzip";

// ── Standard OCI / Docker ────────────────────────────────────────────────────
pub const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const OCI_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
pub const OCI_LAYER_TAR: &str = "application/vnd.oci.image.layer.v1.tar";
pub const OCI_LAYER_GZIP: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
pub const OCI_LAYER_ZSTD: &str = "application/vnd.oci.image.layer.v1.tar+zstd";
pub const OCI_LAYER_NONDIST_GZIP: &str =
    "application/vnd.oci.image.layer.nondistributable.v1.tar+gzip";
pub const OCI_EMPTY: &str = "application/vnd.oci.empty.v1+json";
pub const DOCKER_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";
pub const DOCKER_MANIFEST_LIST: &str = "application/vnd.docker.distribution.manifest.list.v2+json";
pub const DOCKER_CONFIG: &str = "application/vnd.docker.container.image.v1+json";
pub const DOCKER_LAYER_GZIP: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";

/// Path of the disk inside a KubeVirt containerDisk layer.
pub const CONTAINER_DISK_PATH: &str = "disk/disk.img";
/// KubeVirt runs the disk as uid/gid 107 (qemu).
pub const CONTAINER_DISK_UID: u64 = 107;

/// Manifest media types accepted when pulling.
pub const ACCEPTED_MANIFESTS: &[&str] = &[
    OCI_INDEX,
    DOCKER_MANIFEST_LIST,
    OCI_MANIFEST,
    DOCKER_MANIFEST,
];

pub fn is_index(media_type: &str) -> bool {
    media_type == OCI_INDEX || media_type == DOCKER_MANIFEST_LIST
}

pub fn is_container_layer(media_type: &str) -> bool {
    matches!(
        media_type,
        OCI_LAYER_TAR
            | OCI_LAYER_GZIP
            | OCI_LAYER_ZSTD
            | OCI_LAYER_NONDIST_GZIP
            | DOCKER_LAYER_GZIP
    ) || media_type == "application/vnd.docker.image.rootfs.diff.tar"
}

pub fn is_vm_media_type(media_type: &str) -> bool {
    media_type.starts_with("application/vnd.trycua.lume.")
        || media_type.starts_with("application/vnd.agoda.macosvz.")
        || media_type.starts_with("application/vnd.cirruslabs.tart.")
        || media_type.starts_with("application/vnd.trycua.qemu.")
        || media_type.contains("part.number=")
        || media_type == LUME_LZ4_PART
}

/// Format of an image in the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageFormat {
    /// Lume OCI format (trycua lume media types; single or multi-part disk).
    LumeOci,
    /// Lume legacy LZ4/LZFSE chunked parts.
    LumeLegacy,
    /// agoda macosvz media types.
    Agoda,
    Tart,
    /// cua QEMU disk format.
    Qemu,
    /// Plain OCI image (container rootfs or KubeVirt containerDisk; tell them
    /// apart with [`is_container_disk_config`] or by listing the layer).
    Oci,
    Unknown,
}

impl ImageFormat {
    /// VM formats are booted from a disk; `Oci` may be either.
    pub fn is_vm(self) -> bool {
        matches!(
            self,
            Self::LumeOci | Self::LumeLegacy | Self::Agoda | Self::Tart | Self::Qemu
        )
    }
}

/// Classify a single-platform manifest (port of `detect_format`).
pub fn detect_format(m: &ImageManifest) -> ImageFormat {
    let config_mt = m
        .config
        .as_ref()
        .map(|c| c.media_type.as_str())
        .unwrap_or("");
    let any = |pred: &dyn Fn(&str) -> bool| m.layers.iter().any(|l| pred(&l.media_type));
    if config_mt == LUME_CONFIG || any(&|t| t == LUME_DISK || t == LUME_NVRAM) {
        return ImageFormat::LumeOci;
    }
    if config_mt == AGODA_CONFIG || any(&|t| t == AGODA_DISK) {
        return ImageFormat::Agoda;
    }
    if any(&|t| t == LUME_LEGACY_DISK_CHUNK || t == LUME_LZ4_PART || t.contains("part.number=")) {
        return ImageFormat::LumeLegacy;
    }
    if config_mt == QEMU_CONFIG || any(&|t| t == QEMU_DISK || t == QEMU_DISK_GZIP) {
        return ImageFormat::Qemu;
    }
    if any(&|t| t == TART_DISK || t == TART_CONFIG || t == TART_NVRAM) {
        return ImageFormat::Tart;
    }
    if config_mt == OCI_CONFIG || config_mt == DOCKER_CONFIG || any(&is_container_layer) {
        return ImageFormat::Oci;
    }
    ImageFormat::Unknown
}

/// Whether an OCI image *config* describes a KubeVirt containerDisk: no
/// entrypoint/cmd and a build history that copied `/disk/…`.
pub fn is_container_disk_config(config: &serde_json::Value) -> bool {
    let c = config.get("config");
    let no_cmd = c.is_none_or(|c| {
        c.get("Cmd").is_none_or(serde_json::Value::is_null)
            && c.get("Entrypoint").is_none_or(serde_json::Value::is_null)
    });
    let history_mentions_disk = config
        .get("history")
        .and_then(|h| h.as_array())
        .is_some_and(|h| {
            h.iter().any(|e| {
                e.get("created_by")
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s.contains("/disk/"))
            })
        });
    no_cmd && history_mentions_disk
}

/// Media-type prefix of trycua containerDisk artifacts (manifest, config or
/// layer), for registries that keep the variant in the media type.
pub const TRYCUA_CONTAINER_DISK_PREFIX: &str = "application/vnd.trycua.containerdisk";
/// Image config label naming the variant (`containerdisk` / `rootfs`), set by
/// `libs/images` builds.
pub const VARIANT_LABEL: &str = "ai.cua.image.variant";
/// OCI annotation carrying a layer's file name.
pub const ANNOTATION_TITLE: &str = "org.opencontainers.image.title";

/// Why a single-platform image is a KubeVirt containerDisk, or `None` when
/// nothing in its manifest or config says so (then a plain OCI image is a
/// container rootfs). Signals, in order:
///
/// 1. a trycua containerDisk media type (manifest, config or layer) or a layer titled `disk.img` / `disk/disk.img`;
/// 2. the config label `ai.cua.image.variant=containerdisk`;
/// 3. a config with no command whose history copied `/disk/…` (every
///    KubeVirt containerDisk build: `COPY disk.img /disk/disk.img`).
///
/// A config label `ai.cua.image.variant=rootfs` overrides the history.
pub fn container_disk_evidence(
    manifest: Option<&ImageManifest>,
    config: Option<&serde_json::Value>,
) -> Option<String> {
    if let Some(m) = manifest {
        let media_types = std::iter::once(m.media_type.as_str())
            .chain(m.config.iter().map(|c| c.media_type.as_str()))
            .chain(m.layers.iter().map(|l| l.media_type.as_str()));
        for mt in media_types {
            if mt.starts_with(TRYCUA_CONTAINER_DISK_PREFIX) {
                return Some(format!("media type {mt}"));
            }
        }
        for l in &m.layers {
            if let Some(title) = l.annotations.get(ANNOTATION_TITLE) {
                let t = title.trim_start_matches('/');
                if t == CONTAINER_DISK_PATH || t == "disk.img" {
                    return Some(format!("layer {} titled {title}", l.digest));
                }
            }
        }
    }
    let label = config
        .and_then(|c| c.get("config"))
        .and_then(|c| c.get("Labels"))
        .and_then(|l| l.get(VARIANT_LABEL))
        .and_then(|v| v.as_str())
        .map(str::to_ascii_lowercase);
    match label.as_deref() {
        Some("containerdisk") => return Some(format!("label {VARIANT_LABEL}=containerdisk")),
        Some("rootfs") => return None,
        _ => {}
    }
    if config.is_some_and(is_container_disk_config) {
        return Some("a /disk/disk.img layer (config history, no command)".into());
    }
    None
}

/// Guest OS hint from annotations/media types (port of `detect_os`).
pub fn detect_os(m: &ImageManifest) -> Option<&'static str> {
    if let Some(os) = m.annotations.get(LUME_ANNOTATION_OS) {
        let os = os.to_ascii_lowercase();
        if os.contains("mac") {
            return Some("macos");
        }
        if os.contains("windows") {
            return Some("windows");
        }
        if os.contains("linux") {
            return Some("linux");
        }
    }
    match detect_format(m) {
        ImageFormat::Agoda => Some("macos"),
        _ => None,
    }
}

/// A chunked disk part (Lume multi-part / legacy).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskPart {
    pub number: u32,
    pub total: Option<u32>,
    pub offset: Option<u64>,
    pub descriptor: Descriptor,
}

/// Disk parts in order, from annotations or `;part.number=` media types.
pub fn disk_parts(m: &ImageManifest) -> Vec<DiskPart> {
    let mut parts: Vec<DiskPart> = m
        .layers
        .iter()
        .filter_map(|l| {
            let ann = |k: &str| l.annotations.get(k).cloned();
            let mut number = ann(LUME_ANNOTATION_PART_NUMBER).and_then(|v| v.parse().ok());
            let mut total = ann(LUME_ANNOTATION_PART_TOTAL).and_then(|v| v.parse().ok());
            if number.is_none() && l.media_type.contains("part.number=") {
                for seg in l.media_type.split(';') {
                    if let Some(v) = seg.strip_prefix("part.number=") {
                        number = v.parse().ok();
                    } else if let Some(v) = seg.strip_prefix("part.total=") {
                        total = v.parse().ok();
                    }
                }
            }
            Some(DiskPart {
                number: number?,
                total,
                offset: ann(LUME_ANNOTATION_PART_OFFSET).and_then(|v| v.parse().ok()),
                descriptor: l.clone(),
            })
        })
        .collect();
    parts.sort_by_key(|p| p.number);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> ImageManifest {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    #[test]
    fn lume_macos_manifest_is_lume_oci_with_ordered_parts() {
        let m = fixture("lume-oci-macos.json");
        assert_eq!(detect_format(&m), ImageFormat::LumeOci);
        assert!(detect_format(&m).is_vm());
        assert_eq!(detect_os(&m), Some("macos"));
        let parts = disk_parts(&m);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.first().unwrap().number, 0);
        assert_eq!(parts.last().unwrap().number, 159);
        assert_eq!(parts[0].total, Some(160));
        assert_eq!(parts[1].offset, Some(536870912));
    }

    #[test]
    fn fleet_images_are_plain_oci() {
        assert_eq!(
            detect_format(&fixture("fleet-containerdisk-arm64.json")),
            ImageFormat::Oci
        );
        assert_eq!(
            detect_format(&fixture("fleet-rootfs-arm64.json")),
            ImageFormat::Oci
        );
        let cfg: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/fleet-containerdisk-arm64-config.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(is_container_disk_config(&cfg));
        let rootfs_cfg =
            serde_json::json!({"config": {"Cmd": ["/usr/bin/supervisord"]}, "history": []});
        assert!(!is_container_disk_config(&rootfs_cfg));
    }

    #[test]
    fn legacy_part_media_types_parse() {
        let m: ImageManifest = serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST,
            "config": {"mediaType": OCI_CONFIG, "digest": "sha256:00", "size": 1},
            "layers": [
                {"mediaType": "application/octet-stream+lzfse;part.number=2;part.total=2", "digest": "sha256:b", "size": 1},
                {"mediaType": "application/octet-stream+lzfse;part.number=1;part.total=2", "digest": "sha256:a", "size": 1}
            ]
        }))
        .unwrap();
        assert_eq!(detect_format(&m), ImageFormat::LumeLegacy);
        let p = disk_parts(&m);
        assert_eq!(
            (p[0].number, p[0].total, p[0].descriptor.digest.as_str()),
            (1, Some(2), "sha256:a")
        );
    }

    #[test]
    fn vm_media_type_predicate() {
        assert!(is_vm_media_type(LUME_DISK));
        assert!(is_vm_media_type(TART_NVRAM));
        assert!(!is_vm_media_type(OCI_LAYER_GZIP));
        assert!(is_container_layer(DOCKER_LAYER_GZIP));
    }
}
