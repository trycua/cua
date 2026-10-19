//! What an image build is: [`BuildSpec`] (layers, environment, ports and
//! files on a registry base) and the content-addressed names and recipes a
//! build gets, so an identical spec is one content hash everywhere.
//!
//! A local build (`cua-daemon` into the container engine) names its image
//! with [`build_name`] of the [`build_recipe`] of the spec with the base
//! pinned by digest and each copied file referenced by its hash
//! ([`local_build_file`]). The same spec therefore finds its earlier build
//! instead of building again, and a moved base tag builds anew.

use crate::error::InvalidArgument;
use crate::spec::{
    ImageFile, ImageFileReference, ImageLayer, ImageRecipe, RecipeKind, is_env_name,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

/// Prefix of content-named builds (`cua-b-<24 hex>`).
pub const BUILD_NAME_PREFIX: &str = "cua-b-";
/// Default build budget.
pub const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
/// Most environment variables a build takes (the rootfs builder's limit).
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
    pub fn validate(&self) -> Result<(), InvalidArgument> {
        if self.from.trim().is_empty() || self.from.chars().any(char::is_whitespace) {
            return Err(InvalidArgument(
                "an image build needs a base image reference".into(),
            ));
        }
        if self
            .layers
            .iter()
            .any(|l| matches!(l, ImageLayer::AppInstall { .. }))
        {
            return Err(InvalidArgument(
                "app_install layers need a VM image; a build on a registry base makes a \
                 container image"
                    .into(),
            ));
        }
        if let Some(k) = self.env.keys().find(|k| !is_env_name(k)) {
            return Err(InvalidArgument(format!(
                "bad environment variable name {k:?}"
            )));
        }
        // The rootfs builder writes each value as one Dockerfile ENV line
        // and refuses anything else; say so before building anything.
        if self.env.len() > MAX_BUILD_ENV {
            return Err(InvalidArgument(format!(
                "an image build takes at most {MAX_BUILD_ENV} environment variables (got {})",
                self.env.len()
            )));
        }
        for (k, v) in &self.env {
            if v.contains('\n') || v.contains('\r') {
                return Err(InvalidArgument(format!(
                    "environment variable {k} has a multi-line value; an image build stores \
                     each value as one line (encode it, or set it at runtime with env=)"
                )));
            }
            if v.len() > MAX_BUILD_ENV_VALUE {
                return Err(InvalidArgument(format!(
                    "environment variable {k} is longer than {MAX_BUILD_ENV_VALUE} bytes"
                )));
            }
        }
        for f in &self.files {
            if !f.destination.starts_with('/') {
                return Err(InvalidArgument(format!(
                    "copy destination {:?} must be an absolute path",
                    f.destination
                )));
            }
        }
        Ok(())
    }
}

/// The content-addressed build name for a recipe (`cua-b-<24 hex>`).
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

/// The recipe a build of `spec` on `base` (pinned by digest) makes. Builds
/// that upload their `files` and builds that reference them with
/// [`local_build_file`] are named by the same [`build_name`] of it, so an
/// identical spec is one content hash.
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
pub fn local_build_file(f: &BuildFile) -> Result<ImageFile, InvalidArgument> {
    let meta = std::fs::metadata(&f.source)
        .map_err(|e| InvalidArgument(format!("copy {}: {e}", f.source.display())))?;
    if !meta.is_file() {
        return Err(InvalidArgument(format!(
            "copy {}: builds copy single files",
            f.source.display()
        )));
    }
    let bytes = std::fs::read(&f.source)
        .map_err(|e| InvalidArgument(format!("copy {}: {e}", f.source.display())))?;
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
        assert!(crate::spec::is_dns_label(&n));
    }

    #[test]
    fn versions_come_from_the_tag() {
        assert_eq!(version_of("python:3.12-slim"), "3.12-slim");
        assert_eq!(version_of("localhost:5000/app"), "latest");
        assert_eq!(version_of("r.io/a@sha256:ab"), "latest");
        assert_eq!(version_of("python:"), "latest");
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
        assert!(BuildSpec::default().validate().is_err(), "no base image");
    }
}
