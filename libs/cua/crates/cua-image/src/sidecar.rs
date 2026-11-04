//! Sidecars: extra containers that run next to a sandbox, and the rules a
//! sandbox with sidecars follows on every provider.
//!
//! Sidecars are addressed by name: the sandbox reaches a sidecar at its name
//! (on its declared ports) and a sidecar reaches the sandbox at
//! [`MAIN_CONTAINER_NAME`]. Containers of one sandbox share a network
//! namespace, so `localhost` works there too.

use crate::error::InvalidArgument;
use crate::spec::{is_dns_label, is_env_name};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Most sidecars a sandbox may carry.
pub const MAX_SIDECARS: usize = 8;

/// The hostname sidecars reach the sandbox at, and a name no sidecar may
/// take.
pub const MAIN_CONTAINER_NAME: &str = "main";

/// Service names a sandbox with sidecars may not use (reserved on every
/// provider, so a spec stays portable).
pub const RESERVED_SERVICE_NAMES: [&str; 3] = ["main", "sidecars", "sc"];

/// Refuses [`RESERVED_SERVICE_NAMES`] in `services` when `has_sidecars` (a
/// sandbox without sidecars may use them).
pub fn check_reserved_service_names<'a>(
    services: impl IntoIterator<Item = &'a String>,
    has_sidecars: bool,
) -> Result<(), InvalidArgument> {
    if !has_sidecars {
        return Ok(());
    }
    if let Some(n) = services
        .into_iter()
        .find(|n| RESERVED_SERVICE_NAMES.contains(&n.as_str()))
    {
        return Err(InvalidArgument(format!(
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
    /// CPU request as a Kubernetes quantity (`500m`, `1`). Cloud only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Memory request as a Kubernetes quantity (`256Mi`). Cloud only.
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
    if name.is_empty() || name == MAIN_CONTAINER_NAME {
        "sidecar".into()
    } else {
        name
    }
}

/// Checks sidecars before anything is created: at most [`MAX_SIDECARS`],
/// unique DNS-label names other than `main`, an image each, valid env names,
/// ports in range and not claimed by two sidecars. `reserved` ports belong to
/// the sandbox itself (its spacesd); services may target sidecar ports.
pub fn validate_sidecars(sidecars: &[Sidecar], reserved: &[u16]) -> Result<(), InvalidArgument> {
    if sidecars.len() > MAX_SIDECARS {
        return Err(InvalidArgument(format!(
            "at most {MAX_SIDECARS} sidecars (got {})",
            sidecars.len()
        )));
    }
    let mut names = BTreeSet::new();
    let mut ports: BTreeMap<u16, String> = BTreeMap::new();
    for s in sidecars {
        if !is_dns_label(&s.name) || s.name == MAIN_CONTAINER_NAME {
            return Err(InvalidArgument(format!(
                "sidecar name {:?} must be a DNS label other than \"main\"",
                s.name
            )));
        }
        if !names.insert(s.name.as_str()) {
            return Err(InvalidArgument(format!(
                "sidecar name {:?} is used twice",
                s.name
            )));
        }
        if s.image.trim().is_empty() {
            return Err(InvalidArgument(format!(
                "sidecar {:?} needs an image",
                s.name
            )));
        }
        if let Some(k) = s.env.keys().find(|k| !is_env_name(k)) {
            return Err(InvalidArgument(format!(
                "sidecar {:?}: bad environment variable name {k:?}",
                s.name
            )));
        }
        for p in &s.ports {
            if *p == 0 {
                return Err(InvalidArgument(format!("sidecar {:?}: port 0", s.name)));
            }
            if reserved.contains(p) {
                return Err(InvalidArgument(format!(
                    "sidecar {:?} port {p} is also a sandbox port (containers share one \
                     network namespace)",
                    s.name
                )));
            }
            if let Some(other) = ports.insert(*p, s.name.clone()) {
                return Err(InvalidArgument(format!(
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
    fn default_names_are_dns_labels() {
        assert_eq!(default_sidecar_name("redis:7-alpine"), "redis");
        assert_eq!(
            default_sidecar_name("docker.io/library/postgres@sha256:ab"),
            "postgres"
        );
        assert_eq!(default_sidecar_name("ghcr.io/me/My_Svc:1"), "my-svc");
        assert_eq!(default_sidecar_name("reg:5000/main"), "sidecar");
    }

    #[test]
    fn sidecars_validate() {
        let db = Sidecar {
            ports: vec![6379],
            ..Sidecar::new("redis:7-alpine")
        };
        validate_sidecars(std::slice::from_ref(&db), &[8000]).unwrap();
        assert!(
            validate_sidecars(&[db.clone(), db.clone()], &[]).is_err(),
            "dup name"
        );
        assert!(
            validate_sidecars(std::slice::from_ref(&db), &[6379]).is_err(),
            "port clash"
        );
        let twin = Sidecar {
            name: "db2".into(),
            ..db.clone()
        };
        assert!(
            validate_sidecars(&[db.clone(), twin], &[]).is_err(),
            "shared port"
        );
        let main = Sidecar {
            name: "main".into(),
            ..db.clone()
        };
        assert!(validate_sidecars(&[main], &[]).is_err());
        let many: Vec<Sidecar> = (0..9)
            .map(|i| Sidecar {
                name: format!("s{i}"),
                ports: vec![],
                ..db.clone()
            })
            .collect();
        assert!(validate_sidecars(&many, &[]).is_err());
        let bad_env = Sidecar {
            env: [("1X".to_string(), "v".to_string())].into(),
            ..db.clone()
        };
        assert!(validate_sidecars(&[bad_env], &[]).is_err());
        let no_image = Sidecar {
            image: " ".into(),
            ..db.clone()
        };
        assert!(validate_sidecars(&[no_image], &[]).is_err());
        let upper = Sidecar {
            name: "Redis".into(),
            ..db.clone()
        };
        assert!(validate_sidecars(&[upper], &[]).is_err(), "not a DNS label");
        let port0 = Sidecar {
            ports: vec![0],
            ..db
        };
        assert!(validate_sidecars(&[port0], &[]).is_err());
    }

    #[test]
    fn reserved_service_names_need_sidecars() {
        let services = ["web".to_string(), "sc".to_string()];
        check_reserved_service_names(services.iter(), false).unwrap();
        let e = check_reserved_service_names(services.iter(), true).unwrap_err();
        assert!(e.0.contains("\"sc\""), "{e}");
        check_reserved_service_names(["web".to_string()].iter(), true).unwrap();
    }

    #[test]
    fn serde_skips_empty_fields() {
        let s = Sidecar {
            ports: vec![6379],
            ..Sidecar::new("redis:7-alpine")
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"name": "redis", "image": "redis:7-alpine", "ports": [6379]})
        );
        let back: Sidecar = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
