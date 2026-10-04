//! Sidecars: extra containers next to a sandbox, addressed by name.
//!
//! The sandbox reaches a sidecar at its name (on its declared ports) and a
//! sidecar reaches the sandbox at [`MAIN_CONTAINER_NAME`]. Local containers
//! run them in the sandbox's network namespace, so `localhost` works too.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Most sidecars a sandbox may carry.
pub const MAX_SIDECARS: usize = 8;

/// The hostname sidecars reach the sandbox at, and a name no sidecar may
/// take.
pub const MAIN_CONTAINER_NAME: &str = "main";

/// Service names a sandbox with sidecars may not use (reserved on every
/// runtime, so a spec stays portable).
pub const RESERVED_SERVICE_NAMES: [&str; 3] = ["main", "sidecars", "sc"];

/// Refuses [`RESERVED_SERVICE_NAMES`] in `services` when `has_sidecars`
/// (a sandbox without sidecars may use them).
pub fn check_reserved_service_names<'a>(
    services: impl IntoIterator<Item = &'a String>,
    has_sidecars: bool,
) -> Result<()> {
    if !has_sidecars {
        return Ok(());
    }
    if let Some(n) = services
        .into_iter()
        .find(|n| RESERVED_SERVICE_NAMES.contains(&n.as_str()))
    {
        return Err(Error::InvalidArgument(format!(
            "service name {n:?} is reserved in a sandbox with sidecars (reserved: {})",
            RESERVED_SERVICE_NAMES.join(", ")
        )));
    }
    Ok(())
}

/// An extra container next to a sandbox. The sandbox reaches it at its
/// `name` as a hostname (on its declared `ports`), and it reaches the
/// sandbox at [`MAIN_CONTAINER_NAME`]. Local containers share one network
/// namespace, so `localhost` works there too.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Sidecar {
    /// Name, unique within the sandbox (DNS label, not `main`); also its
    /// hostname.
    pub name: String,
    /// Image reference.
    pub image: String,
    /// argv replacing the image's ENTRYPOINT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Environment (plain values, not secrets).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Ports it listens on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    /// Arguments after `command` (or the image's entrypoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// CPU request as a Kubernetes quantity (`500m`, `1`). Not used by
    /// local runtimes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Memory request as a Kubernetes quantity (`256Mi`). Not used by
    /// local runtimes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

impl Sidecar {
    /// A sidecar running `image`, named after the image's repository.
    pub fn new(image: impl Into<String>) -> Self {
        let image = image.into();
        Self {
            name: default_sidecar_name(&image),
            image,
            command: None,
            env: BTreeMap::new(),
            ports: vec![],
            args: None,
            cpu: None,
            memory: None,
        }
    }
}

/// `redis` for `docker.io/library/redis:7-alpine`: the last path segment of
/// the repository, lowercased to a DNS label.
pub fn default_sidecar_name(image: &str) -> String {
    let repo = image.split('@').next().unwrap_or(image);
    let last = repo.rsplit('/').next().unwrap_or(repo);
    let last = last.split(':').next().unwrap_or(last);
    let mut name: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    name = name.trim_matches('-').to_string();
    name.truncate(50);
    let name = name.trim_end_matches('-').to_string();
    if name.is_empty() || name == "main" {
        "sidecar".into()
    } else {
        name
    }
}

/// An RFC 1123 label: 1-63 lowercase letters, digits and hyphens, not
/// starting or ending with a hyphen.
fn is_dns_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Checks sidecars (so a bad spec fails before anything starts): at most
/// [`MAX_SIDECARS`], unique DNS-label names other than `main`, an image
/// each, valid env names, ports in range and not claimed by two sidecars.
/// `reserved` ports belong to the sandbox itself (its spacesd); services
/// may target sidecar ports.
pub fn validate_sidecars(sidecars: &[Sidecar], reserved: &[u16]) -> Result<()> {
    if sidecars.len() > MAX_SIDECARS {
        return Err(Error::InvalidArgument(format!(
            "at most {MAX_SIDECARS} sidecars (got {})",
            sidecars.len()
        )));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut ports: BTreeMap<u16, String> = BTreeMap::new();
    for s in sidecars {
        if !is_dns_label(&s.name) || s.name == MAIN_CONTAINER_NAME {
            return Err(Error::InvalidArgument(format!(
                "sidecar name {:?} must be a DNS label other than \"main\"",
                s.name
            )));
        }
        if !names.insert(s.name.as_str()) {
            return Err(Error::InvalidArgument(format!(
                "sidecar name {:?} is used twice",
                s.name
            )));
        }
        if s.image.trim().is_empty() {
            return Err(Error::InvalidArgument(format!(
                "sidecar {:?} needs an image",
                s.name
            )));
        }
        if let Some(k) = s.env.keys().find(|k| !cua_image::spec::is_env_name(k)) {
            return Err(Error::InvalidArgument(format!(
                "sidecar {:?}: bad environment variable name {k:?}",
                s.name
            )));
        }
        for p in &s.ports {
            if *p == 0 {
                return Err(Error::InvalidArgument(format!(
                    "sidecar {:?}: port 0",
                    s.name
                )));
            }
            if reserved.contains(p) {
                return Err(Error::InvalidArgument(format!(
                    "sidecar {:?} port {p} is also a sandbox port (containers share one \
                     network namespace)",
                    s.name
                )));
            }
            if let Some(other) = ports.insert(*p, s.name.clone()) {
                return Err(Error::InvalidArgument(format!(
                    "port {p} is claimed by sidecars {other:?} and {:?}",
                    s.name
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_default_from_the_image() {
        assert_eq!(
            default_sidecar_name("docker.io/library/redis:7-alpine"),
            "redis"
        );
        assert_eq!(default_sidecar_name("ghcr.io/x/My_App@sha256:ab"), "my-app");
        assert_eq!(default_sidecar_name("main"), "sidecar");
    }

    #[test]
    fn sidecars_validate() {
        let ok = Sidecar {
            ports: vec![6379],
            ..Sidecar::new("redis:7")
        };
        validate_sidecars(std::slice::from_ref(&ok), &[3211]).unwrap();
        let bad_name = Sidecar {
            name: "Main".into(),
            ..ok.clone()
        };
        assert!(validate_sidecars(&[bad_name], &[]).is_err());
        assert!(validate_sidecars(&[ok.clone(), ok.clone()], &[]).is_err());
        assert!(validate_sidecars(std::slice::from_ref(&ok), &[6379]).is_err());
        let services = ["sc".to_string()];
        assert!(check_reserved_service_names(services.iter(), true).is_err());
        check_reserved_service_names(services.iter(), false).unwrap();
    }
}
