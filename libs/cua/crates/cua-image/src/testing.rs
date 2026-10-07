//! Test helpers: [`FakeRegistry`], an in-memory [`ManifestSource`] with
//! builders for the canonical layouts (multi-arch rootfs and containerDisk
//! indexes, Lume and other darwin artifacts). Used by this crate's resolver
//! tests and by the daemon, cua-fleet and SDK tests, so none of them needs a
//! network.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::digest::sha256_bytes;
use crate::error::{ImageError, Result};
use crate::manifest::{Descriptor, Manifest};
use crate::media_types::{
    LUME_CONFIG, LUME_DISK, OCI_CONFIG, OCI_INDEX, OCI_LAYER_GZIP, OCI_MANIFEST,
};
use crate::registry::ManifestSource;
use crate::resolve::NormalizedRef;

/// An in-memory registry for hermetic tests of the resolver and its callers.
#[derive(Default)]
pub struct FakeRegistry {
    /// full ref (tag or digest) -> raw manifest
    pub manifests: BTreeMap<String, Vec<u8>>,
    pub blobs: BTreeMap<String, Vec<u8>>,
    /// full ref -> error kind ("unauthorized")
    pub refuse: BTreeMap<String, &'static str>,
}

#[async_trait::async_trait]
impl ManifestSource for FakeRegistry {
    async fn manifest(&self, reference: &str) -> Result<(Manifest, String)> {
        let full = NormalizedRef::parse(reference)?.full();
        if let Some(kind) = self.refuse.get(&full) {
            assert_eq!(*kind, "unauthorized");
            return Err(ImageError::Unauthorized(format!("url https://x/v2/{full}")));
        }
        let raw = self
            .manifests
            .get(&full)
            .ok_or_else(|| ImageError::NotFound(full.clone()))?;
        Ok((Manifest::parse(raw)?, sha256_bytes(raw)))
    }
    async fn blob(&self, _reference: &str, desc: &Descriptor) -> Result<Vec<u8>> {
        self.blobs
            .get(&desc.digest)
            .cloned()
            .ok_or_else(|| ImageError::NotFound(desc.digest.clone()))
    }
}

impl FakeRegistry {
    pub fn put_blob(&mut self, v: &Value) -> Descriptor {
        let bytes = serde_json::to_vec(v).unwrap();
        let d = sha256_bytes(&bytes);
        self.blobs.insert(d.clone(), bytes.clone());
        Descriptor {
            media_type: OCI_CONFIG.into(),
            digest: d,
            size: bytes.len() as u64,
            ..Default::default()
        }
    }

    /// Stores a manifest under `repo@digest` (and `tag_ref` when given).
    pub fn put_manifest(&mut self, repo: &str, tag_ref: Option<&str>, m: &Value) -> Descriptor {
        let bytes = serde_json::to_vec(m).unwrap();
        let d = sha256_bytes(&bytes);
        self.manifests.insert(format!("{repo}@{d}"), bytes.clone());
        if let Some(t) = tag_ref {
            self.manifests
                .insert(NormalizedRef::parse(t).unwrap().full(), bytes.clone());
        }
        Descriptor {
            media_type: m["mediaType"].as_str().unwrap_or(OCI_MANIFEST).into(),
            digest: d,
            size: bytes.len() as u64,
            ..Default::default()
        }
    }

    pub fn image(
        &mut self,
        repo: &str,
        tag_ref: Option<&str>,
        arch: &str,
        disk: bool,
    ) -> Descriptor {
        let cfg = if disk {
            json!({"os": "linux", "architecture": arch, "config": {},
                "history": [{"created_by": "COPY disk.img /disk/disk.img # buildkit"}]})
        } else {
            json!({"os": "linux", "architecture": arch, "config": {"Cmd": ["/bin/sh"]}})
        };
        let c = self.put_blob(&cfg);
        let m = json!({"schemaVersion": 2, "mediaType": OCI_MANIFEST,
            "config": {"mediaType": c.media_type, "digest": c.digest, "size": c.size},
            "layers": [{"mediaType": OCI_LAYER_GZIP, "digest": "sha256:00", "size": 1}]});
        self.put_manifest(repo, tag_ref, &m)
    }

    /// A multi-arch index at `tag_ref`; returns its digest.
    pub fn index(
        &mut self,
        tag_ref: &str,
        arches: &[&str],
        disk: bool,
        annotations: Option<Value>,
    ) -> String {
        let repo = NormalizedRef::parse(tag_ref).unwrap().repo();
        let manifests: Vec<Value> = arches
            .iter()
            .map(|a| {
                let d = self.image(&repo, None, a, disk);
                json!({"mediaType": OCI_MANIFEST, "digest": d.digest, "size": d.size,
                    "platform": {"os": "linux", "architecture": a}})
            })
            .collect();
        let mut idx = json!({"schemaVersion": 2, "mediaType": OCI_INDEX, "manifests": manifests});
        if let Some(a) = annotations {
            idx["annotations"] = a;
        }
        self.put_manifest(&repo, Some(tag_ref), &idx).digest
    }

    pub fn lume(&mut self, tag_ref: &str) -> String {
        let repo = NormalizedRef::parse(tag_ref).unwrap().repo();
        let c = self.put_blob(&json!({"os": "darwin"}));
        let m = json!({"schemaVersion": 2, "mediaType": OCI_MANIFEST,
            "config": {"mediaType": LUME_CONFIG, "digest": c.digest, "size": c.size},
            "layers": [{"mediaType": LUME_DISK, "digest": "sha256:22", "size": 1}]});
        self.put_manifest(&repo, Some(tag_ref), &m).digest
    }

    /// A darwin-only index in no Lume format: nothing here runs it.
    pub fn darwin_index(&mut self, tag_ref: &str) -> String {
        let repo = NormalizedRef::parse(tag_ref).unwrap().repo();
        let idx = json!({"schemaVersion": 2, "mediaType": OCI_INDEX, "manifests": [
            {"mediaType": OCI_MANIFEST, "digest": "sha256:33", "size": 1,
             "platform": {"os": "darwin", "architecture": "arm64"}}]});
        self.put_manifest(&repo, Some(tag_ref), &idx).digest
    }
}
