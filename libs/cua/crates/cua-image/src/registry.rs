//! Thin wrapper over `oci-client` with credential resolution, retries for
//! registry rate limits, streaming blob download/upload and raw manifests.

use std::path::Path;
use std::time::Duration;

use futures::TryStreamExt;
use oci_client::client::{ClientConfig, ClientProtocol};
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference, RegistryOperation};
use tokio::io::AsyncWriteExt;

use crate::auth;
use crate::error::{ImageError, Result};
use crate::manifest::{Descriptor, ImageIndex, ImageManifest, Manifest, select_platform};
use crate::media_types::ACCEPTED_MANIFESTS;

/// Parse an image reference (`docker.io` defaults like the docker CLI).
pub fn parse_ref(reference: &str) -> Result<Reference> {
    reference
        .parse::<Reference>()
        .map_err(|e| ImageError::Reference(reference.into(), e.to_string()))
}

/// `CUA_REGISTRY_MIRRORS`: pull-through mirrors, `registry=mirror` pairs
/// separated by commas (`docker.io=mirror.example:5000`; `*` matches every
/// registry). A mirror written `http://host:port` is spoken to over plain
/// HTTP. Pulls (manifests, blobs, tag lists) go to the mirror, which gets
/// the original registry as `?ns=`; references, digests and pushes are
/// unchanged. Hermetic tests point this at a local fixture registry.
pub const MIRRORS_ENV: &str = "CUA_REGISTRY_MIRRORS";

/// The configured mirrors: (registry or `*`, mirror host, plain HTTP).
pub fn mirrors() -> Vec<(String, String, bool)> {
    parse_mirrors(&std::env::var(MIRRORS_ENV).unwrap_or_default())
}

fn parse_mirrors(value: &str) -> Vec<(String, String, bool)> {
    value
        .split(',')
        .filter_map(|pair| {
            let (registry, mirror) = pair.trim().split_once('=')?;
            let mirror = mirror.trim().trim_end_matches('/');
            let (host, http) = match mirror.strip_prefix("http://") {
                Some(h) => (h, true),
                None => (mirror.strip_prefix("https://").unwrap_or(mirror), false),
            };
            (!registry.trim().is_empty() && !host.is_empty())
                .then(|| (registry.trim().to_string(), host.to_string(), http))
        })
        .collect()
}

/// The mirror host serving `registry`, if any.
fn mirror_for(registry: &str, mirrors: &[(String, String, bool)]) -> Option<String> {
    let canonical = |r: &str| match r {
        "index.docker.io" | "registry-1.docker.io" => "docker.io".to_string(),
        other => other.to_string(),
    };
    let registry = canonical(registry);
    mirrors
        .iter()
        .find(|(r, _, _)| canonical(r) == registry)
        .or_else(|| mirrors.iter().find(|(r, _, _)| r == "*"))
        .map(|(_, host, _)| host.clone())
}

/// [`parse_ref`] for a pull: routed to its mirror when one is configured.
fn pull_ref(reference: &str) -> Result<Reference> {
    let mut r = parse_ref(reference)?;
    if let Some(host) = mirror_for(r.registry(), &mirrors()) {
        r.set_mirror_registry(host);
    }
    Ok(r)
}

/// Where the resolver and [`crate::detect`] read registry documents from:
/// the real registry ([`RegistryClient`]) or a fake one in tests.
#[async_trait::async_trait]
pub trait ManifestSource: Send + Sync {
    /// Raw manifest (index or image) and the digest of its bytes.
    async fn manifest(&self, reference: &str) -> Result<(Manifest, String)>;
    /// A small blob (image configs).
    async fn blob(&self, reference: &str, desc: &Descriptor) -> Result<Vec<u8>>;
}

#[async_trait::async_trait]
impl ManifestSource for RegistryClient {
    async fn manifest(&self, reference: &str) -> Result<(Manifest, String)> {
        RegistryClient::manifest(self, reference).await
    }
    async fn blob(&self, reference: &str, desc: &Descriptor) -> Result<Vec<u8>> {
        self.blob_bytes(reference, desc).await
    }
}

/// A registry client.
#[derive(Clone)]
pub struct RegistryClient {
    client: Client,
    /// Same settings with `use_monolithic_push`: the fallback for registries
    /// that break the chunked-upload status codes (Amazon ECR answers a
    /// chunk PATCH with 201 instead of 202).
    monolithic: Client,
    /// Explicit credentials; they win over [`auth::resolve`] for the
    /// registry they apply to.
    creds: Option<cua_vmm::RegistryCredentials>,
}

impl Default for RegistryClient {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl RegistryClient {
    /// `insecure` hosts (e.g. `localhost:5000`) are spoken to over plain HTTP.
    /// `CUA_INSECURE_REGISTRIES` (comma separated) is appended.
    pub fn new(mut insecure: Vec<String>) -> Self {
        // oci-client's reqwest links aws-lc-rs next to ring elsewhere in the
        // graph; pin the process default so rustls does not panic.
        let _ = rustls::crypto::ring::default_provider().install_default();
        if let Ok(v) = std::env::var("CUA_INSECURE_REGISTRIES") {
            insecure.extend(
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
        }
        insecure.extend(
            mirrors()
                .into_iter()
                .filter(|(_, _, http)| *http)
                .map(|(_, host, _)| host),
        );
        let protocol = if insecure.is_empty() {
            ClientProtocol::Https
        } else {
            ClientProtocol::HttpsExcept(insecure)
        };
        let config = |monolithic: bool| ClientConfig {
            protocol: protocol.clone(),
            read_timeout: Some(Duration::from_secs(300)),
            connect_timeout: Some(Duration::from_secs(20)),
            use_monolithic_push: monolithic,
            ..Default::default()
        };
        Self {
            client: Client::new(config(false)),
            monolithic: Client::new(config(true)),
            creds: None,
        }
    }

    /// A client that authenticates with `creds` (for the registry they are
    /// scoped to, or any registry when unscoped) before the usual chain.
    pub fn with_credentials(creds: cua_vmm::RegistryCredentials) -> Self {
        Self {
            creds: Some(creds),
            ..Self::default()
        }
    }

    pub fn inner(&self) -> &Client {
        &self.client
    }

    async fn auth_for(&self, r: &Reference, op: RegistryOperation) -> Result<RegistryAuth> {
        let (a, source) = match &self.creds {
            Some(c) if c.applies_to(r.resolve_registry()) => (
                RegistryAuth::Basic(c.username.clone(), c.password.clone()),
                auth::AuthSource::Explicit,
            ),
            _ => auth::resolve(r.resolve_registry()).await,
        };
        tracing::debug!(registry = r.resolve_registry(), ?source, "registry auth");
        if matches!(op, RegistryOperation::Push) {
            self.client.auth(r, &a, op).await?;
        } else {
            self.client
                .store_auth_if_needed(r.resolve_registry(), &a)
                .await;
        }
        Ok(a)
    }

    /// Raw manifest (index or image) and its digest. Retries rate limits.
    pub async fn manifest(&self, reference: &str) -> Result<(Manifest, String)> {
        let r = pull_ref(reference)?;
        let a = self.auth_for(&r, RegistryOperation::Pull).await?;
        let mut delay = Duration::from_secs(2);
        for attempt in 0..6 {
            match self
                .client
                .pull_manifest_raw(&r, &a, ACCEPTED_MANIFESTS)
                .await
            {
                Ok((raw, digest)) => return Ok((Manifest::parse(&raw)?, digest)),
                Err(e) => {
                    let msg = e.to_string();
                    let retryable = msg.contains("TOOMANYREQUESTS")
                        || msg.contains("429")
                        || msg.contains("Rate exceeded");
                    if !retryable || attempt == 5 {
                        return Err(e.into());
                    }
                    tracing::warn!(reference, "registry rate limited; retrying in {delay:?}");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
            }
        }
        unreachable!()
    }

    /// The manifest document exactly as the registry serves it, with its
    /// digest (what a descriptor pointing at it must carry).
    pub async fn manifest_bytes(&self, reference: &str) -> Result<(Vec<u8>, String)> {
        let r = pull_ref(reference)?;
        let a = self.auth_for(&r, RegistryOperation::Pull).await?;
        let (raw, digest) = self
            .client
            .pull_manifest_raw(&r, &a, ACCEPTED_MANIFESTS)
            .await?;
        Ok((raw.to_vec(), digest))
    }

    /// Resolve `reference` to the single-platform manifest for `linux/<arch>`.
    /// Returns `(repository@digest reference, manifest, manifest digest)`.
    pub async fn resolve_platform(
        &self,
        reference: &str,
        arch: &str,
    ) -> Result<(String, ImageManifest, String)> {
        let r = parse_ref(reference)?;
        let repo = format!("{}/{}", r.resolve_registry(), r.repository());
        let (mut m, mut digest) = self.manifest(reference).await?;
        let mut hops = 0;
        loop {
            match m {
                Manifest::Image(img) => return Ok((format!("{repo}@{digest}"), img, digest)),
                Manifest::Index(idx) => {
                    hops += 1;
                    if hops > 4 {
                        return Err(ImageError::Registry(format!(
                            "{reference}: nested image indexes too deep"
                        )));
                    }
                    let entry = select_platform(&idx, "linux", arch)?;
                    let child = format!("{repo}@{}", entry.digest);
                    let (cm, cd) = self.manifest(&child).await?;
                    m = cm;
                    digest = cd;
                }
            }
        }
    }

    /// Stream a blob to `dest` (atomically via `<dest>.partial`), verifying its digest.
    pub async fn blob_to_file(
        &self,
        reference: &str,
        desc: &Descriptor,
        dest: &Path,
    ) -> Result<()> {
        self.blob_to_file_with_progress(reference, desc, dest, |_| {})
            .await
    }

    /// [`RegistryClient::blob_to_file`], calling `on_bytes` with the bytes
    /// written so far as the blob arrives.
    pub async fn blob_to_file_with_progress(
        &self,
        reference: &str,
        desc: &Descriptor,
        dest: &Path,
        on_bytes: impl FnMut(u64) + Send + Unpin,
    ) -> Result<()> {
        let r = pull_ref(reference)?;
        self.auth_for(&r, RegistryOperation::Pull).await?;
        if let Some(p) = dest.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        let partial = dest.with_extension("partial");
        let file = tokio::fs::File::create(&partial).await?;
        let mut w = Counting {
            inner: tokio::io::BufWriter::with_capacity(1 << 20, file),
            written: 0,
            on_bytes,
        };
        let layer = oci_client::manifest::OciDescriptor {
            media_type: desc.media_type.clone(),
            digest: desc.digest.clone(),
            size: desc.size as i64,
            ..Default::default()
        };
        let res = self.client.pull_blob(&r, &layer, &mut w).await;
        if let Err(e) = res {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(e.into());
        }
        w.flush().await?;
        tokio::fs::rename(&partial, dest).await?;
        Ok(())
    }

    /// Small blob into memory (configs).
    pub async fn blob_bytes(&self, reference: &str, desc: &Descriptor) -> Result<Vec<u8>> {
        let r = pull_ref(reference)?;
        self.auth_for(&r, RegistryOperation::Pull).await?;
        let layer = oci_client::manifest::OciDescriptor {
            media_type: desc.media_type.clone(),
            digest: desc.digest.clone(),
            size: desc.size as i64,
            ..Default::default()
        };
        let mut out = Vec::with_capacity(desc.size as usize);
        self.client.pull_blob(&r, &layer, &mut out).await?;
        Ok(out)
    }

    /// Upload a blob from a file (streamed, skipped if already present).
    pub async fn push_blob_file(&self, reference: &str, path: &Path, digest: &str) -> Result<()> {
        let r = parse_ref(reference)?;
        self.auth_for(&r, RegistryOperation::Push).await?;
        if self.client.blob_exists(&r, digest).await.unwrap_or(false) {
            return Ok(());
        }
        let size = tokio::fs::metadata(path).await?.len();
        let open = || async {
            let file = tokio::fs::File::open(path).await?;
            Ok::<_, std::io::Error>(
                tokio_util::io::ReaderStream::with_capacity(file, 8 << 20).map_err(|e| {
                    oci_client::errors::OciDistributionError::GenericError(Some(e.to_string()))
                }),
            )
        };
        match self
            .client
            .push_blob_stream(&r, open().await?, digest, Some(size))
            .await
        {
            Ok(_) => Ok(()),
            // A file can be replayed (a stream cannot): retry as one
            // monolithic upload, like oci-client's in-memory push_blob does.
            Err(oci_client::errors::OciDistributionError::SpecViolationError(violation)) => {
                tracing::warn!(%violation, registry = r.resolve_registry(), "chunked blob push refused; retrying monolithically");
                let (a, _) = auth::resolve(r.resolve_registry()).await;
                self.monolithic
                    .auth(&r, &a, RegistryOperation::Push)
                    .await?;
                self.monolithic
                    .push_blob_stream(&r, open().await?, digest, Some(size))
                    .await?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Upload a small in-memory blob.
    pub async fn push_blob_bytes(
        &self,
        reference: &str,
        data: Vec<u8>,
        digest: &str,
    ) -> Result<()> {
        let r = parse_ref(reference)?;
        self.auth_for(&r, RegistryOperation::Push).await?;
        if self.client.blob_exists(&r, digest).await.unwrap_or(false) {
            return Ok(());
        }
        self.client.push_blob(&r, data, digest).await?;
        Ok(())
    }

    /// Push a manifest document; returns its digest.
    pub async fn push_manifest_json(
        &self,
        reference: &str,
        body: Vec<u8>,
        media_type: &str,
    ) -> Result<String> {
        let r = parse_ref(reference)?;
        self.auth_for(&r, RegistryOperation::Push).await?;
        let digest = crate::digest::sha256_bytes(&body);
        let ct = http::HeaderValue::from_str(media_type)
            .map_err(|e| ImageError::Registry(e.to_string()))?;
        self.client.push_manifest_raw(&r, body, ct).await?;
        Ok(digest)
    }

    pub async fn push_manifest(&self, reference: &str, m: &ImageManifest) -> Result<String> {
        self.push_manifest_json(reference, serde_json::to_vec(m)?, &m.media_type)
            .await
    }

    pub async fn push_index(&self, reference: &str, idx: &ImageIndex) -> Result<String> {
        self.push_manifest_json(reference, serde_json::to_vec(idx)?, &idx.media_type)
            .await
    }

    pub async fn list_tags(&self, reference: &str) -> Result<Vec<String>> {
        let r = pull_ref(reference)?;
        let a = self.auth_for(&r, RegistryOperation::Pull).await?;
        Ok(self.client.list_tags(&r, &a, None, None).await?.tags)
    }
}

/// An `AsyncWrite` that reports the bytes written through it.
struct Counting<W, F> {
    inner: W,
    written: u64,
    on_bytes: F,
}

impl<W: tokio::io::AsyncWrite + Unpin, F: FnMut(u64) + Unpin> tokio::io::AsyncWrite
    for Counting<W, F>
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &polled {
            this.written += *n as u64;
            (this.on_bytes)(this.written);
        }
        polled
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn mirrors_parse_and_match() {
        let m = parse_mirrors("docker.io=http://127.0.0.1:5000/, ghcr.io=mirror.example, bad, =x");
        assert_eq!(
            m,
            vec![
                ("docker.io".into(), "127.0.0.1:5000".into(), true),
                ("ghcr.io".into(), "mirror.example".into(), false),
            ]
        );
        assert_eq!(
            mirror_for("index.docker.io", &m).as_deref(),
            Some("127.0.0.1:5000")
        );
        assert_eq!(mirror_for("ghcr.io", &m).as_deref(), Some("mirror.example"));
        assert_eq!(mirror_for("quay.io", &m), None);
        let all = parse_mirrors("*=http://127.0.0.1:1");
        assert_eq!(mirror_for("quay.io", &all).as_deref(), Some("127.0.0.1:1"));
    }

    #[test]
    fn a_mirrored_pull_keeps_the_original_reference() {
        let mut r = parse_ref("python:3.12-slim").unwrap();
        r.set_mirror_registry("127.0.0.1:5000".into());
        assert_eq!(r.resolve_registry(), "127.0.0.1:5000");
        assert_eq!(r.namespace(), Some("docker.io"));
        assert_eq!(r.repository(), "library/python");
    }
}
