//! What `cua run --compat daytona --prewarm IMAGE[=N]` needs to know about
//! Daytona.
//!
//! The CLI keeps capacity warm for a Cua image; this module says which Cua
//! image a Daytona `snapshot` name or image reference starts from (the same
//! resolution a create makes), so a sandbox created afterwards lands on warm
//! capacity and the SDK's own 60 s create timeout holds on a cold host. A
//! Daytona warm pool is the same thing declared on the server: its snapshot
//! resolves the same way and its `pool` size is the count to keep.

use super::SnapshotEntry;
use super::dockerfile;
use super::stores::WarmPoolEntry;
use crate::e2b::TemplateTarget;
use std::collections::BTreeMap;

/// The Cua image a Daytona reference starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The Cua image to keep warm.
    pub cua_image: String,
    /// Dockerfile steps the gateway builds on top (once, as cached layers;
    /// every sandbox then starts from the built image).
    pub layers: usize,
    /// The shape a create from it gets (vCPUs, MiB): the snapshot's own
    /// resources, else the gateway's default (Daytona's: 1 vCPU, 1 GiB). A
    /// prewarm of another shape would not be claimed by that create.
    pub cpus: u32,
    pub memory_mb: u64,
}

/// Resolves `reference` the way a create resolves its `snapshot`: empty or
/// `base` is the default image, a registered snapshot is its image (a
/// Dockerfile snapshot its base and layers), anything with a `/` or `:` is
/// an image reference. A snapshot of a sandbox is a copy-on-write fork
/// source that is already instant, so there is nothing to keep warm.
pub fn resolve(
    reference: &str,
    snapshots: &[SnapshotEntry],
    default_image: &str,
    default_shape: (u32, u64),
) -> Result<Target, String> {
    let reference = reference.trim();
    let shape = |cpu: u32, mem_gib: u32| {
        (
            if cpu > 0 { cpu } else { default_shape.0 },
            if mem_gib > 0 {
                u64::from(mem_gib) * 1024
            } else {
                default_shape.1
            },
        )
    };
    if reference.is_empty() || reference == "base" {
        return Ok(Target {
            cua_image: default_image.to_string(),
            layers: 0,
            cpus: default_shape.0,
            memory_mb: default_shape.1,
        });
    }
    if let Some(s) = snapshots
        .iter()
        .find(|s| s.name == reference || s.id == reference)
    {
        if s.state != "active" {
            return Err(format!("snapshot {reference} is {}", s.state));
        }
        if let Some(b) = &s.build {
            let plan = dockerfile::plan(&dockerfile::parse(&b.dockerfile)?, &BTreeMap::new())?;
            let (cpus, memory_mb) = shape(s.cpu, s.memory);
            return Ok(Target {
                cua_image: plan.from,
                layers: plan.steps.len(),
                cpus,
                memory_mb,
            });
        }
        return match &s.target {
            TemplateTarget::Image(i) => {
                let (cpus, memory_mb) = shape(s.cpu, s.memory);
                Ok(Target {
                    cua_image: i.clone(),
                    layers: 0,
                    cpus,
                    memory_mb,
                })
            }
            TemplateTarget::Checkpoint(_) => Err(format!(
                "snapshot {reference} is a checkpoint of a sandbox: creates from it are already copy-on-write forks"
            )),
        };
    }
    if reference.contains('/') || reference.contains(':') {
        return Ok(Target {
            cua_image: reference.to_string(),
            layers: 0,
            cpus: default_shape.0,
            memory_mb: default_shape.1,
        });
    }
    Err(format!(
        "{reference} is not a snapshot, and not an image reference (name a registry image such as python:3.12)"
    ))
}

/// What Daytona's warm pools ask to keep ready: `(snapshot, Cua image,
/// count)` per pool whose snapshot can be kept warm (the others are skipped).
pub fn warm_pool_targets(
    pools: &[WarmPoolEntry],
    snapshots: &[SnapshotEntry],
    default_image: &str,
    default_shape: (u32, u64),
) -> Vec<(String, Target, u32)> {
    pools
        .iter()
        .filter_map(|w| {
            resolve(&w.snapshot, snapshots, default_image, default_shape)
                .ok()
                .map(|mut t| {
                    // the pool's own resources, when it names them
                    if w.cpu > 0 {
                        t.cpus = w.cpu;
                    }
                    if w.mem > 0 {
                        t.memory_mb = u64::from(w.mem) * 1024;
                    }
                    (w.snapshot.clone(), t, w.pool)
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Daytona's default sandbox: 1 vCPU, 1 GiB.
    const S: (u32, u64) = (1, 1024);

    fn snap(name: &str, target: TemplateTarget) -> SnapshotEntry {
        serde_json::from_value(serde_json::json!({
            "id": format!("id-{name}"),
            "name": name,
            "target": serde_json::to_value(&target).unwrap(),
            "created_at_ms": 0
        }))
        .unwrap()
    }

    #[test]
    fn references_resolve_as_a_create_resolves_them() {
        let snaps = vec![
            snap("web", TemplateTarget::Image("ghcr.io/x/web:1".into())),
            snap("fork", TemplateTarget::Checkpoint("cp".into())),
        ];
        let d = "ghcr.io/trycua/linux:24.04";
        assert_eq!(resolve("", &snaps, d, S).unwrap().cua_image, d);
        let t = resolve("web", &snaps, d, S).unwrap();
        assert_eq!(
            (t.cpus, t.memory_mb),
            (1, 1024),
            "the default sandbox is 1 vCPU / 1 GiB"
        );
        assert_eq!(resolve("base", &snaps, d, S).unwrap().cua_image, d);
        assert_eq!(
            resolve("web", &snaps, d, S).unwrap().cua_image,
            "ghcr.io/x/web:1"
        );
        assert_eq!(
            resolve("python:3.12", &snaps, d, S).unwrap().cua_image,
            "python:3.12"
        );
        assert!(
            resolve("fork", &snaps, d, S)
                .unwrap_err()
                .contains("copy-on-write")
        );
        assert!(
            resolve("nope", &snaps, d, S)
                .unwrap_err()
                .contains("not a snapshot")
        );
    }

    #[test]
    fn a_dockerfile_snapshot_warms_its_base_with_its_layers() {
        let mut s = snap("df", TemplateTarget::Image(String::new()));
        s.build = Some(super::super::SnapshotBuild {
            dockerfile: "FROM python:3.12\nRUN pip install requests\nRUN echo hi\n".into(),
            context_hashes: vec![],
        });
        let t = resolve("df", &[s], "d", S).unwrap();
        assert_eq!((t.cua_image.as_str(), t.layers), ("python:3.12", 2));
    }
}
