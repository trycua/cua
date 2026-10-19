//! Remote image builds: Image layers (`pip_install`, `run`, `copy`, `env`,
//! ...) on a registry base, built by Fleet's images API
//! (`images.cua.ai/v1alpha1 Image`, `recipe.kind: container` with
//! `recipe.from`), so the same `Image` works with `local=False`.
//!
//! Builds are cached by content: the Image resource is named after a hash
//! of the recipe with the base pinned to its digest and uploaded files by
//! theirs. An identical spec finds the earlier Image (and its built
//! digest) instead of building again; a moved base tag builds anew.
//!
//! Resources live in a per-account build namespace, `cua-build-<16>`, never
//! in a pool namespace (pools are keyed by the image a build produces).

use crate::{Error, FleetClient, RegistryCredentials, Result, parity};
use cua_image::spec::{ImageFile, ImageFileReference, ImageResource};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

// The build spec and the content-addressed names and recipes are not Fleet's:
// they live in `cua-image` (`cua_image::build`), where local builds use them
// too. Re-exported here so every `cua_fleet::builds::*` path still resolves.
pub use cua_image::build::{
    BUILD_NAME_PREFIX, BuildFile, BuildSpec, DEFAULT_BUILD_TIMEOUT, MAX_BUILD_ENV,
    MAX_BUILD_ENV_VALUE, build_name, build_recipe, local_build_file,
};

/// Prefix of the per-account build namespace.
pub const BUILD_NAMESPACE_PREFIX: &str = "cua-build-";
/// Largest file `copy` uploads (read into memory once).
pub const MAX_COPY_BYTES: u64 = 64 << 20;

/// A built (or cached) image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltImage {
    /// `repo@sha256:…` to run.
    pub reference: String,
    /// Manifest digest.
    pub digest: String,
    /// The Image resource (`namespace/name`) that holds the build.
    pub namespace: String,
    /// Image resource name (`cua-b-<content hash>`).
    pub name: String,
    /// Found an identical earlier build.
    pub cached: bool,
}

/// Progress of a remote build, in the portable stage vocabulary: a build is
/// part of `provisioning`.
pub type BuildProgress<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// The build namespace for a tenant (`cua-build-<base32(sha256)[..16]>`).
pub fn build_namespace(tenant: &str) -> String {
    let h = Sha256::digest(format!("cua-build\0{tenant}").as_bytes());
    format!(
        "{BUILD_NAMESPACE_PREFIX}{}",
        &crate::autopool::base32(&h)[..16]
    )
}

fn memo() -> &'static Mutex<HashMap<String, BuiltImage>> {
    static M: OnceLock<Mutex<HashMap<String, BuiltImage>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// `repo@digest` for an OCI artifact reference (tag dropped).
fn pinned(reference: &str, digest: &str) -> String {
    let repo = reference.split('@').next().unwrap_or(reference);
    let repo = match repo.rsplit_once(':') {
        Some((r, t)) if !t.contains('/') => r,
        _ => repo,
    };
    format!("{repo}@{digest}")
}

impl FleetClient {
    /// Builds `spec` with Fleet's images API, or returns the identical
    /// earlier build. `creds` authenticate a private base (read locally to
    /// pin it, and stored as the build namespace's `cua-registry-*` pull
    /// Secret for the builder). `progress` receives short status lines.
    pub async fn build_image(
        &self,
        spec: &BuildSpec,
        creds: Option<&RegistryCredentials>,
        progress: Option<BuildProgress<'_>>,
    ) -> Result<BuiltImage> {
        // A spec the builder would refuse fails first, whatever the build.
        spec.validate()?;
        if !parity::REMOTE_BUILDS_SUPPORTED {
            return Err(parity::remote_builds_unsupported(
                "image layers (a remote image build)",
            ));
        }
        let say = |m: &str| {
            tracing::info!("{m}");
            if let Some(p) = progress {
                p(m);
            }
        };
        // The base must be a container rootfs (a VM disk cannot take
        // container layers); pinned by digest so a moved tag builds anew.
        let base =
            crate::resolve_fleet_image_with(Some(crate::RuntimeKind::Gvisor), &spec.from, creds)
                .await
                .map_err(|e| {
                    Error::InvalidArgument(format!(
                        "cloud image layers build on a container base: {e}"
                    ))
                })?
                .image;
        let token = self.access_token(false).await?;
        let namespace = build_namespace(&crate::autopool::tenant_from_token(&token));

        // Files are content-addressed uploads.
        self.ensure_build_namespace(&namespace).await?;
        let mut files = Vec::with_capacity(spec.files.len());
        for f in &spec.files {
            files.push(self.upload_build_file(&namespace, f).await?);
        }
        let pull_secret = match creds {
            Some(c) => {
                let registry = c
                    .registry
                    .clone()
                    .unwrap_or_else(|| cua_image::registry_of(&spec.from).to_string());
                let name = parity::registry_secret_name(&registry, &c.username);
                self.put_registry_secret(&namespace, &name, &registry, c)
                    .await?;
                Some(name)
            }
            None => None,
        };
        let recipe = build_recipe(spec, &base, files, pull_secret);
        let name = build_name(&recipe);
        let memo_key = format!("{namespace}/{name}");
        if let Some(b) = memo().lock().unwrap().get(&memo_key).cloned() {
            return Ok(b);
        }
        // Metadata stays name + namespace: the gateway's image admission
        // refuses any other metadata key.
        let resource = ImageResource::new(&name, &namespace, recipe);
        resource
            .validate()
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;

        let (cached, mut current) = match self.image_json(&namespace, &name).await? {
            Some(v) => (true, v),
            None => {
                say(&format!("building image {name} on {base}"));
                let manifest = serde_json::to_value(&resource)
                    .map_err(|e| Error::InvalidArgument(e.to_string()))?;
                match self.create_image(&namespace, manifest).await {
                    Ok(v) => (false, v),
                    // A concurrent identical build: follow it.
                    Err(Error::Sdk(cyclops_sdk::SdkError::Status { status: 409, .. })) => (
                        true,
                        self.image_json(&namespace, &name)
                            .await?
                            .unwrap_or(Value::Null),
                    ),
                    Err(Error::Sdk(cyclops_sdk::SdkError::Status { status: 404, .. })) => {
                        return Err(parity::remote_builds_unsupported(
                            "image layers (a remote image build)",
                        ));
                    }
                    Err(e) => return Err(e),
                }
            }
        };
        let timeout = spec.timeout.unwrap_or(DEFAULT_BUILD_TIMEOUT);
        let deadline = Instant::now() + timeout;
        let interval = Duration::from_millis(self.config().pool_poll_interval_ms.clamp(10, 5000));
        let max_polls = (timeout.as_millis() / interval.as_millis().max(1)) as u64 + 2;
        let mut last_phase = String::new();
        for _ in 0..max_polls {
            let phase = current["status"]["phase"].as_str().unwrap_or("Pending");
            if phase != last_phase {
                if phase != "Ready" && phase != "Failed" {
                    say(&format!("building image {name} ({phase})"));
                }
                last_phase = phase.to_string();
            }
            match phase {
                "Ready" => {
                    let oci = &current["status"]["artifacts"]["oci"];
                    let (Some(reference), Some(digest)) =
                        (oci["reference"].as_str(), oci["digest"].as_str())
                    else {
                        return Err(Error::InvalidArgument(format!(
                            "image build {namespace}/{name} is Ready without an OCI artifact"
                        )));
                    };
                    let built = BuiltImage {
                        reference: pinned(reference, digest),
                        digest: digest.to_string(),
                        namespace: namespace.clone(),
                        name: name.clone(),
                        cached,
                    };
                    memo().lock().unwrap().insert(memo_key, built.clone());
                    return Ok(built);
                }
                "Failed" => {
                    let why = current["status"]["conditions"]
                        .as_array()
                        .and_then(|c| {
                            c.iter()
                                .find_map(|c| c["message"].as_str().filter(|m| !m.is_empty()))
                        })
                        .unwrap_or("no message");
                    return Err(Error::InvalidArgument(format!(
                        "image build {namespace}/{name} failed: {why}"
                    )));
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(interval).await;
            current = self.image_json(&namespace, &name).await?.ok_or_else(|| {
                Error::InvalidArgument(format!("image build {namespace}/{name} disappeared"))
            })?;
        }
        Err(Error::Timeout(format!(
            "image build {namespace}/{name} did not finish within {timeout:?}"
        )))
    }

    /// The Image resource, `None` when absent.
    async fn image_json(&self, namespace: &str, name: &str) -> Result<Option<Value>> {
        match self.get_image(namespace, name).await {
            Ok(v) => Ok(Some(v)),
            Err(Error::Sdk(cyclops_sdk::SdkError::Status { status: 404, .. })) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn ensure_build_namespace(&self, namespace: &str) -> Result<()> {
        match self.sdk().create_namespace(namespace.to_string()).await {
            Ok(_) | Err(cyclops_sdk::SdkError::Status { status: 409, .. }) => Ok(()),
            Err(cyclops_sdk::SdkError::Status { status: 403, .. }) => Err(Error::InvalidArgument(
                format!("the build namespace {namespace} belongs to another account"),
            )),
            Err(e) => Err(e.into()),
        }
    }

    async fn upload_build_file(&self, namespace: &str, f: &BuildFile) -> Result<ImageFile> {
        let meta = std::fs::metadata(&f.source)
            .map_err(|e| Error::InvalidArgument(format!("copy {}: {e}", f.source.display())))?;
        if !meta.is_file() {
            return Err(Error::InvalidArgument(format!(
                "copy {}: remote builds copy single files",
                f.source.display()
            )));
        }
        if meta.len() > MAX_COPY_BYTES {
            return Err(Error::InvalidArgument(format!(
                "copy {}: {} bytes is more than the {MAX_COPY_BYTES}-byte limit for remote \
                 builds; bake large files into the base image",
                f.source.display(),
                meta.len()
            )));
        }
        let bytes = std::fs::read(&f.source)
            .map_err(|e| Error::InvalidArgument(format!("copy {}: {e}", f.source.display())))?;
        let file_name = f
            .source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let up = self
            .sdk()
            .upload_image_file(namespace.to_string(), file_name, bytes)
            .await?;
        Ok(ImageFile {
            source: ImageFileReference {
                reference: up.reference,
                digest: up.digest,
                size_bytes: up.size_bytes,
            },
            destination: f.destination.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_image::spec::ImageRecipe;

    #[test]
    fn namespaces_and_names_are_dns_labels() {
        let name = build_name(&ImageRecipe {
            distro: "registry".into(),
            version: "3.12-slim".into(),
            from: Some("docker.io/library/python@sha256:ab".into()),
            ..Default::default()
        });
        assert!(name.starts_with(BUILD_NAME_PREFIX));
        assert!(cyclops_sdk::validate_dns_label(&name).is_ok());
        let ns = build_namespace("tenant-a");
        assert!(ns.starts_with("cua-build-") && cyclops_sdk::validate_dns_label(&ns).is_ok());
        assert_ne!(ns, build_namespace("tenant-b"));
        assert!(
            !ns.starts_with(crate::autopool::AUTO_POOL_PREFIX),
            "never GC'd as a pool"
        );
    }

    #[test]
    fn pins() {
        assert_eq!(
            pinned("reg.test/builds/x:rootfs-1", "sha256:cd"),
            "reg.test/builds/x@sha256:cd"
        );
        assert_eq!(
            pinned("localhost:5000/x", "sha256:cd"),
            "localhost:5000/x@sha256:cd"
        );
    }

    #[test]
    fn a_refused_spec_is_an_invalid_argument() {
        // `BuildSpec::validate` lives in cua-image; its error becomes the
        // crate's `Error::InvalidArgument` with the same message.
        let e: Error = BuildSpec::default().validate().unwrap_err().into();
        assert!(
            matches!(&e, Error::InvalidArgument(m) if m.contains("base image")),
            "{e:?}"
        );
    }
}
