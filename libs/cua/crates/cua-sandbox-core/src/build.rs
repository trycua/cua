//! Image builds: image layers (`pip_install`, `run`, `copy`, `env`, ...) on
//! a container base, built into this machine's container engine
//! ([`crate::LocalRuntime::build_image`]).
//!
//! Builds are cached by content: the built image is named after a hash of
//! the recipe with the base pinned to its digest and copied files by
//! theirs ([`build_name`]), so an identical spec is not built again and a
//! moved base tag builds anew.

use crate::{Error, Result};
pub use cua_image::RegistryCredentials;
pub use cua_image::spec::ImageLayer;
use cua_image::spec::{ImageFile, ImageFileReference, ImageRecipe, RecipeKind};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

/// Prefix of content-named built images.
pub const BUILD_NAME_PREFIX: &str = "cua-b-";
/// Default build budget.
pub const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
/// Most environment variables a build takes.
pub const MAX_BUILD_ENV: usize = 128;
/// Longest environment value a build takes.
pub const MAX_BUILD_ENV_VALUE: usize = 8192;

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
                "an image build needs a base image reference".into(),
            ));
        }
        if self
            .layers
            .iter()
            .any(|l| matches!(l, ImageLayer::AppInstall { .. }))
        {
            return Err(Error::InvalidArgument(
                "app_install layers need a VM image; a build on a registry base makes a \
                 container image"
                    .into(),
            ));
        }
        if let Some(k) = self.env.keys().find(|k| !cua_image::spec::is_env_name(k)) {
            return Err(Error::InvalidArgument(format!(
                "bad environment variable name {k:?}"
            )));
        }
        // Each value is written as one Dockerfile ENV line; say so before
        // building anything.
        if self.env.len() > MAX_BUILD_ENV {
            return Err(Error::InvalidArgument(format!(
                "an image build takes at most {MAX_BUILD_ENV} environment variables (got {})",
                self.env.len()
            )));
        }
        for (k, v) in &self.env {
            if v.contains('\n') || v.contains('\r') {
                return Err(Error::InvalidArgument(format!(
                    "environment variable {k} has a multi-line value; an image build stores \
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

/// The content-addressed image name for a recipe.
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

/// The recipe a build of `spec` on `base` (pinned by digest) makes, named
/// by [`build_name`] (with [`local_build_file`] references for `copy`
/// files), so an identical spec is one content hash.
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

/// A local build's content-addressed reference for a `copy` file (its
/// sha256 and size), for [`build_recipe`].
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_builds_are_content_addressed() {
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
    fn versions() {
        assert_eq!(version_of("python:3.12-slim"), "3.12-slim");
        assert_eq!(version_of("localhost:5000/app"), "latest");
        assert_eq!(version_of("r.io/a@sha256:ab"), "latest");
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
        let long = BuildSpec {
            env: [("A".to_string(), "x".repeat(MAX_BUILD_ENV_VALUE + 1))].into(),
            ..ok.clone()
        };
        assert!(long.validate().is_err());
        let many = BuildSpec {
            env: (0..=MAX_BUILD_ENV)
                .map(|i| (format!("V{i}"), "1".to_string()))
                .collect(),
            ..ok
        };
        assert!(many.validate().is_err());
    }
}
