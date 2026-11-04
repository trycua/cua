//! Locally packed images (layer files + config + manifest) and pushing them,
//! including multi-arch image indexes.

use std::path::{Path, PathBuf};

use crate::digest::sha256_bytes;
use crate::error::Result;
use crate::manifest::{Descriptor, ImageIndex, ImageManifest, Platform, image_manifest, oci_arch};
use crate::media_types::{OCI_CONFIG, OCI_INDEX, OCI_MANIFEST};
use crate::registry::RegistryClient;

/// A single-platform image ready to push.
#[derive(Clone, Debug)]
pub struct PackedImage {
    /// OCI arch (`amd64`/`arm64`).
    pub arch: String,
    pub config: Vec<u8>,
    /// Layer descriptors and the local files holding their bytes.
    pub layers: Vec<(Descriptor, PathBuf)>,
}

impl PackedImage {
    pub fn new(
        arch: &str,
        config: serde_json::Value,
        layers: Vec<(Descriptor, PathBuf)>,
    ) -> Result<Self> {
        Ok(Self {
            arch: oci_arch(arch).to_string(),
            config: serde_json::to_vec(&config)?,
            layers,
        })
    }

    pub fn config_descriptor(&self) -> Descriptor {
        Descriptor {
            media_type: OCI_CONFIG.into(),
            digest: sha256_bytes(&self.config),
            size: self.config.len() as u64,
            ..Default::default()
        }
    }

    pub fn manifest(&self) -> ImageManifest {
        image_manifest(
            self.config_descriptor(),
            self.layers.iter().map(|(d, _)| d.clone()).collect(),
        )
    }

    /// Push blobs + manifest to `reference`; returns the manifest descriptor.
    pub async fn push(&self, client: &RegistryClient, reference: &str) -> Result<Descriptor> {
        for (desc, path) in &self.layers {
            tracing::info!(reference, digest = %desc.digest, size = desc.size, "pushing layer");
            client.push_blob_file(reference, path, &desc.digest).await?;
        }
        let cfg = self.config_descriptor();
        client
            .push_blob_bytes(reference, self.config.clone(), &cfg.digest)
            .await?;
        let m = self.manifest();
        let body = serde_json::to_vec(&m)?;
        let digest = client
            .push_manifest_json(reference, body.clone(), OCI_MANIFEST)
            .await?;
        Ok(Descriptor {
            media_type: OCI_MANIFEST.into(),
            digest,
            size: body.len() as u64,
            platform: Some(Platform {
                architecture: self.arch.clone(),
                os: "linux".into(),
                variant: None,
            }),
            ..Default::default()
        })
    }

    /// Write an OCI image layout directory (`oci-layout`, `index.json`,
    /// `blobs/sha256/*`) — handy for `crane`/`skopeo`/`oras` interop.
    pub fn write_oci_layout(&self, dir: &Path) -> Result<()> {
        let blobs = dir.join("blobs").join("sha256");
        std::fs::create_dir_all(&blobs)?;
        std::fs::write(dir.join("oci-layout"), br#"{"imageLayoutVersion":"1.0.0"}"#)?;
        let put = |digest: &str, bytes: &[u8]| std::fs::write(blobs.join(&digest[7..]), bytes);
        for (d, p) in &self.layers {
            let dst = blobs.join(&d.digest[7..]);
            if std::fs::hard_link(p, &dst).is_err() {
                std::fs::copy(p, &dst)?;
            }
        }
        let cfg = self.config_descriptor();
        put(&cfg.digest, &self.config)?;
        let m = serde_json::to_vec(&self.manifest())?;
        let md = sha256_bytes(&m);
        put(&md, &m)?;
        let index = ImageIndex {
            schema_version: 2,
            media_type: OCI_INDEX.into(),
            manifests: vec![Descriptor {
                media_type: OCI_MANIFEST.into(),
                digest: md,
                size: m.len() as u64,
                platform: Some(Platform {
                    architecture: self.arch.clone(),
                    os: "linux".into(),
                    variant: None,
                }),
                ..Default::default()
            }],
            annotations: Default::default(),
        };
        std::fs::write(dir.join("index.json"), serde_json::to_vec_pretty(&index)?)?;
        Ok(())
    }
}

/// Push several single-arch images under one tag as an OCI image index.
/// Each arch is also pushed to `<reference>-<arch>` for direct pulls.
pub async fn push_multiarch(
    client: &RegistryClient,
    reference: &str,
    images: &[PackedImage],
) -> Result<String> {
    let mut entries = Vec::new();
    for img in images {
        let arch_ref = format!("{reference}-{}", img.arch);
        let desc = img.push(client, &arch_ref).await?;
        entries.push(desc);
    }
    let index = ImageIndex {
        schema_version: 2,
        media_type: OCI_INDEX.into(),
        manifests: entries,
        annotations: Default::default(),
    };
    client.push_index(reference, &index).await
}
