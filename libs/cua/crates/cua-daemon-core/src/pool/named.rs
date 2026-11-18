//! Pools (`PoolService`, the SDK's `create_pool`): sandboxes of one shape
//! kept ready on this machine, `min` at all times (one starts again as soon
//! as one is created from the pool) and up to `max` while creates from the
//! pool come faster than sandboxes start; after `idle` without a create it
//! keeps none ready until the next one. A create that names the pool
//! (`pool = NAME`) runs its shape and claims one of its sandboxes. Kept in
//! `<state>/.pool/pools.json` (0600: the spec may carry environment
//! values), and applied again whenever the daemon starts.

use crate::{CreateRequest, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One named pool as stored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedPool {
    /// Its name.
    pub name: String,
    /// The Spaces it keeps: a create request in its protocol JSON form.
    pub spec: serde_json::Value,
    /// Sandboxes kept ready at all times.
    pub min: u32,
    /// The most kept ready (at least `min`), not counting those created
    /// from the pool.
    pub max: u32,
    /// Seconds without a create after which it keeps none ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_secs: Option<u64>,
}

impl NamedPool {
    /// A pool `name` of `spec`'s Spaces.
    pub fn new(
        name: &str,
        spec: CreateRequest,
        min: u32,
        max: u32,
        idle: Option<std::time::Duration>,
    ) -> Result<Self> {
        validate_name(name)?;
        let pb = crate::convert::create_request_to_pb(CreateRequest {
            name: None,
            pool: None,
            token: None,
            labels: Default::default(),
            owner_pid: None,
            ..spec
        });
        Ok(Self {
            name: name.to_string(),
            spec: serde_json::to_value(&pb).map_err(|e| Error::Internal(e.to_string()))?,
            min,
            max: max.max(min),
            idle_secs: idle.map(|d| d.as_secs().max(1)),
        })
    }

    /// Its idle time, when it has one.
    pub fn idle(&self) -> Option<std::time::Duration> {
        self.idle_secs.map(std::time::Duration::from_secs)
    }

    /// The create request its Spaces start from.
    pub fn request(&self) -> Result<CreateRequest> {
        let pb: cua_proto::daemon::v1::CreateSandboxRequest =
            serde_json::from_value(self.spec.clone())
                .map_err(|e| Error::Internal(format!("pool {}: {e}", self.name)))?;
        crate::convert::create_request_from_pb(pb)
    }
}

/// A pool name: a DNS label (lower-case letters, digits and `-`).
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "pool name {name:?}: use lower-case letters, digits and '-' (at most 63)"
        )))
    }
}

/// `create` with the shape of pool `np`: its image, OS, size, ports,
/// services, environment, command, kind, runtime and network; the
/// create's own name, token, labels and readiness stay. A create that asks
/// for another image or size than the pool's is refused.
pub fn merge_into(create: CreateRequest, np: &NamedPool) -> Result<CreateRequest> {
    let pool = np.request()?;
    let differs = |what: &str, given: String, pool_has: String| -> Result<()> {
        if given.is_empty() || given == pool_has {
            Ok(())
        } else {
            Err(Error::InvalidArgument(format!(
                "pool {} runs {what} {pool_has}, not {given}; omit it or use another pool",
                np.name
            )))
        }
    };
    // A backend prefix (`container:`, `vm:`, ...) a client adds is not
    // another image.
    let bare = |i: &str| -> String {
        let i = i.trim();
        match i.split_once(':') {
            Some((p, rest))
                if ["container", "docker", "vm", "lume"].contains(&p)
                    && !rest.starts_with("//") =>
            {
                rest.to_string()
            }
            _ => i.to_string(),
        }
    };
    differs("image", bare(&create.image), bare(&pool.image))?;
    differs(
        "cpus",
        create
            .cpus
            .filter(|c| *c > 0)
            .map(|c| c.to_string())
            .unwrap_or_default(),
        pool.cpus
            .filter(|c| *c > 0)
            .unwrap_or(super::DEFAULT_SIZE.0)
            .to_string(),
    )?;
    differs(
        "memory_mb",
        create
            .memory_mb
            .filter(|m| *m > 0)
            .map(|m| m.to_string())
            .unwrap_or_default(),
        pool.memory_mb
            .filter(|m| *m > 0)
            .unwrap_or(super::DEFAULT_SIZE.1)
            .to_string(),
    )?;
    let mut env: BTreeMap<String, String> = pool.env.clone();
    // The create's token (if any) is its own; nothing else of its
    // environment may change the shape.
    for k in ["CUA_ENV_TOKEN", "CUA_SPACESD_TOKEN"] {
        if let Some(v) = create.env.get(k) {
            env.insert(k.to_string(), v.clone());
        }
    }
    Ok(CreateRequest {
        image: pool.image,
        os: pool.os,
        cpus: pool.cpus,
        memory_mb: pool.memory_mb,
        ports: pool.ports,
        services: pool.services,
        env,
        command: pool.command,
        kind: pool.kind,
        runtime: pool.runtime,
        network: pool.network,
        gpu: pool.gpu,
        headless: pool.headless,
        container_runtime: pool.container_runtime,
        pool: None,
        ..create
    })
}

/// The pools file under `dir`.
pub fn file(dir: &Path) -> PathBuf {
    dir.join("pools.json")
}

/// Every stored pool.
pub fn load(dir: &Path) -> Vec<NamedPool> {
    std::fs::read(file(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Stores `pools` (0600).
pub fn save(dir: &Path, pools: &[NamedPool]) -> std::io::Result<()> {
    let path = file(dir);
    cua_home::guard_write(&path)?;
    std::fs::create_dir_all(dir)?;
    cua_home::write_private(&path, &serde_json::to_vec_pretty(pools)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(image: &str) -> CreateRequest {
        CreateRequest {
            image: image.into(),
            cpus: Some(4),
            memory_mb: Some(8192),
            env: [("A".to_string(), "1".to_string())].into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_pool_round_trips_without_what_belongs_to_one_space() {
        let mut spec = req("ghcr.io/trycua/linux:24.04-slim");
        spec.name = Some("x".into());
        spec.token = Some("secret".into());
        let np = NamedPool::new("agents", spec, 2, 1, None).unwrap();
        assert_eq!((np.min, np.max), (2, 2), "max is at least min");
        let r = np.request().unwrap();
        assert_eq!(r.image, "ghcr.io/trycua/linux:24.04-slim");
        assert_eq!((r.cpus, r.memory_mb), (Some(4), Some(8192)));
        assert_eq!(r.name, None);
        assert_eq!(r.token, None, "no token is stored");
        assert!(NamedPool::new("Bad_Name", req("i"), 1, 1, None).is_err());
    }

    #[test]
    fn a_create_from_a_pool_takes_its_shape_and_keeps_its_own_identity() {
        let np = NamedPool::new("agents", req("img:1"), 1, 1, None).unwrap();
        let create = CreateRequest {
            name: Some("mine".into()),
            pool: Some("agents".into()),
            env: [("CUA_ENV_TOKEN".to_string(), "t".to_string())].into(),
            ..Default::default()
        };
        let m = merge_into(create, &np).unwrap();
        assert_eq!(m.image, "img:1");
        assert_eq!((m.cpus, m.memory_mb), (Some(4), Some(8192)));
        assert_eq!(m.name.as_deref(), Some("mine"));
        assert_eq!(m.pool, None);
        assert_eq!(m.env.get("A").map(String::as_str), Some("1"));
        assert_eq!(m.env.get("CUA_ENV_TOKEN").map(String::as_str), Some("t"));
        let other = CreateRequest {
            image: "img:2".into(),
            ..Default::default()
        };
        assert!(merge_into(other, &np).is_err());
        let prefixed = CreateRequest {
            image: "container:img:1".into(),
            ..Default::default()
        };
        assert_eq!(merge_into(prefixed, &np).unwrap().image, "img:1");
    }

    #[test]
    fn pools_are_stored_owner_only() {
        let d = tempfile::tempdir().unwrap();
        let np = NamedPool::new(
            "agents",
            req("img:1"),
            1,
            3,
            Some(std::time::Duration::from_secs(600)),
        )
        .unwrap();
        save(d.path(), std::slice::from_ref(&np)).unwrap();
        assert_eq!(load(d.path()), vec![np]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(file(d.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
