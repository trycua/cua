//! OCI manifest / index types and platform resolution.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{ImageError, Result};
use crate::media_types::{self, is_index};

/// OCI content descriptor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}

impl Descriptor {
    pub fn title(&self) -> Option<&str> {
        self.annotations
            .get("org.opencontainers.image.title")
            .map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub architecture: String,
    pub os: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// A single-platform image manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifest {
    #[serde(default = "two")]
    pub schema_version: u32,
    #[serde(default)]
    pub media_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Descriptor>,
    #[serde(default)]
    pub layers: Vec<Descriptor>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

fn two() -> u32 {
    2
}

/// A multi-platform image index / manifest list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageIndex {
    #[serde(default = "two")]
    pub schema_version: u32,
    #[serde(default)]
    pub media_type: String,
    pub manifests: Vec<Descriptor>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

/// Either kind of manifest document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Manifest {
    Image(ImageManifest),
    Index(ImageIndex),
}

impl Manifest {
    /// Parse raw manifest bytes; the index/manifest decision uses the media
    /// type when present and the `manifests` key otherwise.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let v: serde_json::Value = serde_json::from_slice(raw)?;
        let mt = v.get("mediaType").and_then(|m| m.as_str()).unwrap_or("");
        if is_index(mt) || (mt.is_empty() && v.get("manifests").is_some()) {
            Ok(Manifest::Index(serde_json::from_value(v)?))
        } else {
            Ok(Manifest::Image(serde_json::from_value(v)?))
        }
    }
}

/// Normalise `x86_64`/`aarch64` spellings to OCI `amd64`/`arm64`.
pub fn oci_arch(arch: &str) -> &str {
    match arch {
        "x86_64" | "x86-64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => other,
    }
}

/// Pick the index entry for `os/arch`, skipping BuildKit attestation entries
/// (`vnd.docker.reference.type: attestation-manifest`).
pub fn select_platform<'a>(index: &'a ImageIndex, os: &str, arch: &str) -> Result<&'a Descriptor> {
    let arch = oci_arch(arch);
    index
        .manifests
        .iter()
        .filter(|d| !is_attestation(d))
        .find(|d| {
            d.platform
                .as_ref()
                .is_some_and(|p| p.os == os && oci_arch(&p.architecture) == arch)
        })
        .ok_or_else(|| {
            let available: Vec<String> = index
                .manifests
                .iter()
                .filter(|d| !is_attestation(d))
                .filter_map(|d| {
                    d.platform
                        .as_ref()
                        .map(|p| format!("{}/{}", p.os, p.architecture))
                })
                .collect();
            ImageError::PlatformNotFound {
                wanted: format!("{os}/{arch}"),
                available: available.join(", "),
            }
        })
}

fn is_attestation(d: &Descriptor) -> bool {
    d.annotations
        .get("vnd.docker.reference.type")
        .is_some_and(|t| t.contains("attestation"))
}

/// Minimal OCI image config for images we produce.
pub fn image_config(
    arch: &str,
    diff_ids: &[String],
    history: &str,
    cmd: Option<Vec<String>>,
) -> serde_json::Value {
    let mut config = serde_json::json!({
        "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
        "WorkingDir": "/",
    });
    if let Some(cmd) = cmd {
        config["Cmd"] = serde_json::json!(cmd);
    }
    serde_json::json!({
        "architecture": oci_arch(arch),
        "os": "linux",
        "config": config,
        "rootfs": { "type": "layers", "diff_ids": diff_ids },
        "history": [ { "created_by": history, "comment": "cua-image" } ],
    })
}

/// Build an image manifest for a config + layers.
pub fn image_manifest(config: Descriptor, layers: Vec<Descriptor>) -> ImageManifest {
    ImageManifest {
        schema_version: 2,
        media_type: media_types::OCI_MANIFEST.into(),
        config: Some(config),
        layers,
        annotations: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn fleet_index_resolves_each_arch_and_skips_attestations() {
        let Manifest::Index(idx) = Manifest::parse(&raw("fleet-index.json")).unwrap() else {
            panic!("index")
        };
        assert_eq!(idx.manifests.len(), 4);
        let arm = select_platform(&idx, "linux", "aarch64").unwrap();
        assert_eq!(arm.platform.as_ref().unwrap().architecture, "arm64");
        let amd = select_platform(&idx, "linux", "x86_64").unwrap();
        assert_eq!(amd.platform.as_ref().unwrap().architecture, "amd64");
        assert_ne!(arm.digest, amd.digest);
        let err = select_platform(&idx, "linux", "riscv64")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("linux/amd64") && err.contains("linux/arm64") && !err.contains("unknown"),
            "{err}"
        );
    }

    #[test]
    fn single_manifest_parses_layers() {
        let Manifest::Image(m) = Manifest::parse(&raw("fleet-containerdisk-arm64.json")).unwrap()
        else {
            panic!("manifest")
        };
        assert_eq!(m.layers.len(), 1);
        assert_eq!(m.layers[0].media_type, media_types::OCI_LAYER_GZIP);
        assert!(m.layers[0].size > 1_000_000_000);
        // Round-trips without inventing fields.
        let again: ImageManifest =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(again, m);
    }

    #[test]
    fn index_without_media_type_is_detected() {
        let m = Manifest::parse(br#"{"schemaVersion":2,"manifests":[]}"#).unwrap();
        assert!(matches!(m, Manifest::Index(_)));
    }
}
