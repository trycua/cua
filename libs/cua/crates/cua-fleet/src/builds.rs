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
use cua_image::spec::{
    ImageFile, ImageFileReference, ImageLayer, ImageRecipe, ImageResource, RecipeKind,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

/// Prefix of the per-account build namespace.
pub const BUILD_NAMESPACE_PREFIX: &str = "cua-build-";
/// Prefix of content-named Image resources.
pub const BUILD_NAME_PREFIX: &str = "cua-b-";
/// Default build budget.
pub const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
/// Most environment variables a remote build takes (the rootfs builder's
/// limit, trycua/cloud#7887).
pub const MAX_BUILD_ENV: usize = 128;
/// Longest environment value a remote build takes.
pub const MAX_BUILD_ENV_VALUE: usize = 8192;
/// Largest file `copy` uploads (read into memory once).
pub const MAX_COPY_BYTES: u64 = 64 << 20;

/// What to build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildSpec {
    /// Registry base image.
    pub from: String,
    /// Build steps, in order.
    pub layers: Vec<ImageLayer>,
    /// Image environment.
    pub env: BTreeMap<String, String>,
    /// Ports the image exposes.
    pub ports: Vec<u16>,
    /// Local files copied into the image.
    pub files: Vec<BuildFile>,
    /// Build budget (default one hour).
    pub timeout: Option<Duration>,
}

/// A local file copied into the image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildFile {
    /// Local path.
    pub source: PathBuf,
    /// Absolute path in the image.
    pub destination: String,
}

impl BuildSpec {
    /// Whether there is anything to build on top of `from`.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty() && self.env.is_empty() && self.files.is_empty()
    }

    /// Checks the spec (a base, no VM-only layers, env names, absolute
    /// copy destinations).
    pub fn validate(&self) -> Result<()> {
        if self.from.trim().is_empty() || self.from.chars().any(char::is_whitespace) {
            return Err(Error::InvalidArgument(
                "a remote build needs a base image reference".into(),
            ));
        }
        if self
            .layers
            .iter()
            .any(|l| matches!(l, ImageLayer::AppInstall { .. }))
        {
            return Err(Error::InvalidArgument(
                "app_install layers need a VM image; a cloud build on a registry base makes a \
                 container image"
                    .into(),
            ));
        }
        if let Some(k) = self.env.keys().find(|k| !cua_image::spec::is_env_name(k)) {
            return Err(Error::InvalidArgument(format!(
                "bad environment variable name {k:?}"
            )));
        }
        // The rootfs builder writes each value as one Dockerfile ENV line
        // and refuses anything else; say so before uploading anything.
        if self.env.len() > MAX_BUILD_ENV {
            return Err(Error::InvalidArgument(format!(
                "a remote build takes at most {MAX_BUILD_ENV} environment variables (got {})",
                self.env.len()
            )));
        }
        for (k, v) in &self.env {
            if v.contains('\n') || v.contains('\r') {
                return Err(Error::InvalidArgument(format!(
                    "environment variable {k} has a multi-line value; a remote build stores \
                     each value as one line (encode it, or set it at runtime with env=)"
                )));
            }
            if v.len() > MAX_BUILD_ENV_VALUE {
                return Err(Error::InvalidArgument(format!(
                    "environment variable {k} is longer than {MAX_BUILD_ENV_VALUE} bytes"
                )));
            }
        }
        for f in &self.files {
            if !f.destination.starts_with('/') {
                return Err(Error::InvalidArgument(format!(
                    "copy destination {:?} must be an absolute path",
                    f.destination
                )));
            }
        }
        Ok(())
    }
}

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

/// The content-addressed Image name for a recipe.
pub fn build_name(recipe: &ImageRecipe) -> String {
    let canonical = serde_json::to_string(&json!({
        "domain": "cua.remote-build/v1",
        "recipe": recipe,
    }))
    .unwrap_or_default();
    format!(
        "{BUILD_NAME_PREFIX}{}",
        &hex::encode(Sha256::digest(canonical.as_bytes()))[..24]
    )
}

/// The recipe a build of `spec` on `base` (pinned by digest) makes. Remote
/// builds (with uploaded `files`) and local builds (with
/// [`local_build_file`] references) are named by the same
/// [`build_name`] of it, so an identical spec is one content hash.
pub fn build_recipe(
    spec: &BuildSpec,
    base: &str,
    files: Vec<ImageFile>,
    from_pull_secret: Option<String>,
) -> ImageRecipe {
    ImageRecipe {
        distro: "registry".into(),
        version: version_of(&spec.from),
        kind: RecipeKind::Container,
        from: Some(base.to_string()),
        from_pull_secret,
        layers: spec.layers.clone(),
        env: spec.env.clone(),
        files,
        ports: {
            let mut p = spec.ports.clone();
            p.sort_unstable();
            p.dedup();
            p
        },
        ..Default::default()
    }
}

/// A local build's content-addressed reference for a `copy` file (the
/// same shape as an upload: its sha256 and size), for [`build_recipe`].
pub fn local_build_file(f: &BuildFile) -> Result<ImageFile> {
    let meta = std::fs::metadata(&f.source)
        .map_err(|e| Error::InvalidArgument(format!("copy {}: {e}", f.source.display())))?;
    if !meta.is_file() {
        return Err(Error::InvalidArgument(format!(
            "copy {}: builds copy single files",
            f.source.display()
        )));
    }
    let bytes = std::fs::read(&f.source)
        .map_err(|e| Error::InvalidArgument(format!("copy {}: {e}", f.source.display())))?;
    let digest = hex::encode(Sha256::digest(&bytes));
    Ok(ImageFile {
        source: ImageFileReference {
            reference: format!("uploads/local/{}", &digest[..32]),
            digest: format!("sha256:{digest}"),
            size_bytes: meta.len(),
        },
        destination: f.destination.clone(),
    })
}

/// The tag of `reference` as a recipe `version` (1..64 chars), or `latest`.
fn version_of(reference: &str) -> String {
    let no_digest = reference.split('@').next().unwrap_or(reference);
    let tag = match no_digest.rsplit_once(':') {
        Some((_, t)) if !t.contains('/') => t,
        _ => "latest",
    };
    let mut v: String = tag.chars().take(64).collect();
    if v.is_empty() {
        v = "latest".into();
    }
    v
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

    #[test]
    fn local_builds_share_the_content_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, b"print(1)\n").unwrap();
        let spec = BuildSpec {
            from: "python:3.12-slim".into(),
            layers: vec![ImageLayer::PipInstall {
                packages: vec!["mcp".into()],
            }],
            files: vec![BuildFile {
                source: path.clone(),
                destination: "/srv/app.py".into(),
            }],
            ports: vec![8000, 8000],
            ..Default::default()
        };
        let base = "docker.io/library/python@sha256:ab";
        let name = |spec: &BuildSpec| {
            let files = spec
                .files
                .iter()
                .map(|f| local_build_file(f).unwrap())
                .collect();
            build_name(&build_recipe(spec, base, files, None))
        };
        let first = name(&spec);
        assert!(first.starts_with(BUILD_NAME_PREFIX) && first.len() == 6 + 24);
        assert_eq!(name(&spec.clone()), first, "same spec, same name");
        // File content is part of the hash; its path is not.
        std::fs::write(&path, b"print(2)\n").unwrap();
        assert_ne!(name(&spec), first);
        let f = local_build_file(&spec.files[0]).unwrap();
        assert!(f.source.digest.starts_with("sha256:"));
        assert_eq!(f.source.size_bytes, 9);
        assert!(
            local_build_file(&BuildFile {
                source: dir.path().into(),
                destination: "/x".into()
            })
            .is_err()
        );
    }

    #[test]
    fn names_are_content_addressed() {
        let r = |cmd: &str| ImageRecipe {
            distro: "registry".into(),
            version: "3.12-slim".into(),
            kind: RecipeKind::Container,
            from: Some("docker.io/library/python@sha256:ab".into()),
            layers: vec![ImageLayer::Run {
                command: cmd.into(),
            }],
            ..Default::default()
        };
        assert_eq!(build_name(&r("true")), build_name(&r("true")));
        assert_ne!(build_name(&r("true")), build_name(&r("false")));
        let n = build_name(&r("true"));
        assert!(n.starts_with("cua-b-") && n.len() == 6 + 24);
        assert!(cyclops_sdk::validate_dns_label(&n).is_ok());
        let ns = build_namespace("tenant-a");
        assert!(ns.starts_with("cua-build-") && cyclops_sdk::validate_dns_label(&ns).is_ok());
        assert_ne!(ns, build_namespace("tenant-b"));
        assert!(
            !ns.starts_with(crate::autopool::AUTO_POOL_PREFIX),
            "never GC'd as a pool"
        );
    }

    #[test]
    fn versions_and_pins() {
        assert_eq!(version_of("python:3.12-slim"), "3.12-slim");
        assert_eq!(version_of("localhost:5000/app"), "latest");
        assert_eq!(version_of("r.io/a@sha256:ab"), "latest");
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
    fn specs_validate() {
        let ok = BuildSpec {
            from: "python:3.12-slim".into(),
            layers: vec![ImageLayer::PipInstall {
                packages: vec!["mcp".into()],
            }],
            ..Default::default()
        };
        ok.validate().unwrap();
        assert!(!ok.is_empty());
        let app = BuildSpec {
            layers: vec![ImageLayer::AppInstall { app_id: "x".into() }],
            ..ok.clone()
        };
        assert!(app.validate().is_err());
        let rel = BuildSpec {
            files: vec![BuildFile {
                source: "a".into(),
                destination: "rel".into(),
            }],
            ..ok.clone()
        };
        assert!(rel.validate().is_err());
        let multi = BuildSpec {
            env: [("CERT".to_string(), "line1\nline2".to_string())].into(),
            ..ok.clone()
        };
        let e = multi.validate().unwrap_err().to_string();
        assert!(e.contains("multi-line") && e.contains("CERT"), "{e}");
        let cr = BuildSpec {
            env: [("A".to_string(), "x\ry".to_string())].into(),
            ..ok.clone()
        };
        assert!(cr.validate().is_err());
        let long = BuildSpec {
            env: [("A".to_string(), "x".repeat(MAX_BUILD_ENV_VALUE + 1))].into(),
            ..ok.clone()
        };
        assert!(long.validate().is_err());
        let many = BuildSpec {
            env: (0..=MAX_BUILD_ENV)
                .map(|i| (format!("V{i}"), "1".to_string()))
                .collect(),
            ..ok.clone()
        };
        assert!(many.validate().is_err());
        let single = BuildSpec {
            env: [("A".to_string(), "one line with spaces $X".to_string())].into(),
            ..ok
        };
        single.validate().unwrap();
    }
}
