//! Thin wrapper over `oci-client` with credential resolution, retries for
//! registry rate limits, streaming blob download/upload and raw manifests.

use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
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
    /// Pulls from repositories that refused anonymous access, with the
    /// credential helper's credentials. A client of its own: oci-client keeps
    /// the first credentials it sees for a registry.
    authed: Client,
    /// The `registry/repository`s that refused anonymous access.
    gated: Arc<Mutex<HashSet<String>>>,
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
            authed: Client::new(config(false)),
            gated: Arc::default(),
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

    /// Credentials for pushing to `r`: the whole chain, helper included (a
    /// push is someone's deliberate act).
    async fn push_auth(&self, r: &Reference) -> Result<RegistryAuth> {
        let (a, source) = match &self.creds {
            Some(c) if c.applies_to(r.resolve_registry()) => (
                RegistryAuth::Basic(c.username.clone(), c.password.clone()),
                auth::AuthSource::Explicit,
            ),
            _ => auth::resolve(r.resolve_registry()).await,
        };
        tracing::debug!(registry = r.resolve_registry(), ?source, "registry auth");
        self.client.auth(r, &a, RegistryOperation::Push).await?;
        Ok(a)
    }

    fn gate_key(r: &Reference) -> String {
        format!("{}/{}", r.resolve_registry(), r.repository())
    }

    fn is_gated(&self, r: &Reference) -> bool {
        self.gated.lock().unwrap().contains(&Self::gate_key(r))
    }

    /// The client and credentials for a pull from `r`, and where the
    /// credentials came from. They never come from the credential helper
    /// (see [`auth::resolve_quiet`]) unless this repository already refused
    /// anonymous access: [`Self::pull_with`] is what asks the helper.
    async fn pull_route(&self, r: &Reference) -> (Client, RegistryAuth, auth::AuthSource) {
        let registry = r.resolve_registry();
        let (client, a, source) = match &self.creds {
            Some(c) if c.applies_to(registry) => (
                self.client.clone(),
                RegistryAuth::Basic(c.username.clone(), c.password.clone()),
                auth::AuthSource::Explicit,
            ),
            _ if self.is_gated(r) => {
                let (a, source) = auth::resolve(registry).await;
                (self.authed.clone(), a, source)
            }
            _ => {
                let (a, source) = auth::resolve_quiet(registry).await;
                (self.client.clone(), a, source)
            }
        };
        tracing::debug!(registry, ?source, "registry auth");
        client.store_auth_if_needed(registry, &a).await;
        (client, a, source)
    }

    /// Runs a pull with [`Self::pull_route`]'s client and credentials. A
    /// registry that refuses anonymous access (`Unauthorized`) gets one more
    /// try with the user's credential helper, if it has credentials for it,
    /// and the repository is remembered as needing them: a public image
    /// never reaches the helper.
    async fn pull_with<T, Fut>(
        &self,
        r: &Reference,
        op: impl Fn(Client, RegistryAuth) -> Fut,
    ) -> Result<T>
    where
        Fut: Future<Output = Result<T>>,
    {
        let (client, a, source) = self.pull_route(r).await;
        let refused = match op(client, a).await {
            Err(e @ ImageError::Unauthorized(_)) if source == auth::AuthSource::Anonymous => e,
            other => return other,
        };
        let registry = r.resolve_registry();
        let Some((a, source)) = auth::resolve_helper(registry).await else {
            return Err(refused);
        };
        tracing::debug!(
            registry,
            ?source,
            "anonymous pull refused; using the credential helper's"
        );
        self.gated.lock().unwrap().insert(Self::gate_key(r));
        self.authed.store_auth_if_needed(registry, &a).await;
        op(self.authed.clone(), a).await
    }

    /// Raw manifest (index or image) and its digest. Retries rate limits.
    pub async fn manifest(&self, reference: &str) -> Result<(Manifest, String)> {
        let r = pull_ref(reference)?;
        let r = &r;
        self.pull_with(r, |client, a| async move {
            let mut delay = Duration::from_secs(2);
            for attempt in 0..6 {
                match client.pull_manifest_raw(r, &a, ACCEPTED_MANIFESTS).await {
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
        })
        .await
    }

    /// The manifest document exactly as the registry serves it, with its
    /// digest (what a descriptor pointing at it must carry).
    pub async fn manifest_bytes(&self, reference: &str) -> Result<(Vec<u8>, String)> {
        let r = pull_ref(reference)?;
        let r = &r;
        self.pull_with(r, |client, a| async move {
            let (raw, digest) = client.pull_manifest_raw(r, &a, ACCEPTED_MANIFESTS).await?;
            Ok((raw.to_vec(), digest))
        })
        .await
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
    ///
    /// A connection that breaks mid-blob does not lose what arrived: the
    /// download continues from `<dest>.partial` with a `Range` request, up to
    /// [`BLOB_ATTEMPTS`] times, and a pull that still fails leaves the
    /// partial file for the next pull to continue. A 6 GB single-layer
    /// image (the Windows containerDisk) on a slow or flaky link used to
    /// start from zero on every try. The whole file is checked against the
    /// descriptor's digest before it is renamed to `dest`.
    pub async fn blob_to_file_with_progress(
        &self,
        reference: &str,
        desc: &Descriptor,
        dest: &Path,
        mut on_bytes: impl FnMut(u64) + Send + Unpin,
    ) -> Result<()> {
        let r = pull_ref(reference)?;
        // The blob's manifest was read first, which found out whether this
        // repository wants credentials (`pull_with`).
        let (client, _, _) = self.pull_route(&r).await;
        if let Some(p) = dest.parent() {
            tokio::fs::create_dir_all(p).await?;
        }
        let partial = dest.with_extension("partial");
        // Two processes may fetch the same blob: one at a time continues
        // the partial file (held until this pull ends), and one that waited
        // finds the blob already in place.
        let lock_path = dest.with_extension("partial.lock");
        let _lock = tokio::task::spawn_blocking(move || {
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)?;
            f.lock()?;
            Ok::<_, std::io::Error>(f)
        })
        .await
        .map_err(|e| ImageError::Registry(e.to_string()))??;
        if tokio::fs::metadata(dest).await.is_ok() {
            return Ok(());
        }
        let layer = oci_client::manifest::OciDescriptor {
            media_type: desc.media_type.clone(),
            digest: desc.digest.clone(),
            size: desc.size as i64,
            ..Default::default()
        };
        let mut attempt = 1;
        loop {
            match download_blob(&client, &r, &layer, desc.size, &partial, &mut on_bytes).await {
                Ok(()) => break,
                Err(e) if attempt < BLOB_ATTEMPTS && continues_after(&e) => {
                    let have = tokio::fs::metadata(&partial).await.map_or(0, |m| m.len());
                    tracing::warn!(reference, digest = %desc.digest, attempt, have, size = desc.size, error = %e, "blob download broke; continuing it");
                    tokio::time::sleep(blob_retry_delay(attempt)).await;
                    attempt += 1;
                }
                Err(e) => {
                    // Kept when the next pull can continue it.
                    if !continues_after(&e) {
                        let _ = tokio::fs::remove_file(&partial).await;
                    }
                    return Err(e);
                }
            }
        }
        // oci-client checks a whole-blob response only; a continued one is
        // checked here, from the file.
        let check = partial.clone();
        let (actual, _) = tokio::task::spawn_blocking(move || crate::digest::sha256_file(&check))
            .await
            .map_err(|e| ImageError::Registry(e.to_string()))??;
        if desc.digest.starts_with("sha256:") && actual != desc.digest {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(ImageError::Digest {
                what: format!("blob of {reference}"),
                expected: desc.digest.clone(),
                actual,
            });
        }
        tokio::fs::rename(&partial, dest).await?;
        Ok(())
    }

    /// Small blob into memory (configs).
    pub async fn blob_bytes(&self, reference: &str, desc: &Descriptor) -> Result<Vec<u8>> {
        let r = pull_ref(reference)?;
        let r = &r;
        self.pull_with(r, |client, _| async move {
            let layer = oci_client::manifest::OciDescriptor {
                media_type: desc.media_type.clone(),
                digest: desc.digest.clone(),
                size: desc.size as i64,
                ..Default::default()
            };
            let mut out = Vec::with_capacity(desc.size as usize);
            client.pull_blob(r, &layer, &mut out).await?;
            Ok(out)
        })
        .await
    }

    /// Upload a blob from a file (streamed, skipped if already present).
    pub async fn push_blob_file(&self, reference: &str, path: &Path, digest: &str) -> Result<()> {
        let r = parse_ref(reference)?;
        self.push_auth(&r).await?;
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
        self.push_auth(&r).await?;
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
        self.push_auth(&r).await?;
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
        let r = &r;
        self.pull_with(r, |client, a| async move {
            Ok(client.list_tags(r, &a, None, None).await?.tags)
        })
        .await
    }
}

/// An `AsyncWrite` that reports the bytes written through it.
/// How many times one pull asks for a blob: the first request, then a
/// continuation (`Range`) after each break.
pub const BLOB_ATTEMPTS: u32 = 5;

/// The pause before continuation `attempt` (1 s, 2 s, 4 s, ...; at most 15 s).
fn blob_retry_delay(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.saturating_sub(1).min(4)).min(15))
}

/// Whether a failed blob download may continue from what arrived: a broken
/// connection, a short body, a server error. Not a refusal (not found, not
/// authorized) or wrong content.
fn continues_after(e: &ImageError) -> bool {
    matches!(e, ImageError::Io(_) | ImageError::Registry(_))
}

/// One request for the blob into `partial`: continues after the bytes it
/// already holds (a `206` answer) or starts again (a `200` answer, or a
/// file longer than the blob). `on_bytes` gets the blob bytes on disk so far.
async fn download_blob(
    client: &Client,
    r: &Reference,
    layer: &oci_client::manifest::OciDescriptor,
    size: u64,
    partial: &Path,
    on_bytes: &mut (impl FnMut(u64) + Send),
) -> Result<()> {
    use oci_client::client::BlobResponse;
    let mut have = tokio::fs::metadata(partial).await.map_or(0, |m| m.len());
    if size > 0 && have > size {
        have = 0;
    }
    if size > 0 && have == size {
        on_bytes(have);
        return Ok(());
    }
    let (stream, append) = if have == 0 {
        (client.pull_blob_stream(r, layer).await?, false)
    } else {
        match client
            .pull_blob_stream_partial(r, layer, have, None)
            .await?
        {
            BlobResponse::Partial(s) => (s, true),
            // The registry sent the whole blob: start the file again.
            BlobResponse::Full(s) => (s, false),
        }
    };
    let mut written = if append { have } else { 0 };
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(partial)
        .await?;
    let mut out = tokio::io::BufWriter::with_capacity(1 << 20, file);
    on_bytes(written);
    let mut stream = stream.stream;
    let res: Result<()> = async {
        while let Some(chunk) = stream.try_next().await? {
            out.write_all(&chunk).await?;
            written += chunk.len() as u64;
            on_bytes(written);
        }
        Ok(())
    }
    .await;
    // What arrived stays on disk for the next request, also on a break.
    out.flush().await?;
    res?;
    if size > 0 && written < size {
        return Err(ImageError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "blob {} ended after {written} of {size} bytes",
                layer.digest
            ),
        )));
    }
    Ok(())
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
