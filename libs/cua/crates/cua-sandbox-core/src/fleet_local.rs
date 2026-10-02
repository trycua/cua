//! Run a Fleet template (or the template behind a Fleet pool) locally.
//!
//! A Fleet `OSGymSandboxTemplate`'s `vmTemplate` already says everything a
//! local runtime needs: the image, the runtime (`kubevirt` containerDisk,
//! `gvisor` rootfs), firmware, CPU and memory, the guest
//! services and a readiness probe. [`plan_from_template`] turns it into
//! [`CreateOptions`] for the local provider:
//!
//! | `vmTemplate` | local |
//! |---|---|
//! | `runtime: kubevirt`, `containerDiskImage` | `vm:<image>` (QEMU) |
//! | `runtime: gvisor`, pod image | `container:<image>` (gVisor) |
//! | `runtime: macos` | [`Error::UnsupportedImage`] |
//! | `firmware: efi \| bios` | QEMU firmware |
//! | `cpuCores`, `memory` | caps on the create options' vCPUs / memory |
//! | `services[].targetPort` | published guest ports (by service name) |
//! | `probes.readinessProbe` `tcpSocket` / `httpGet` | readiness probes |
//!
//! [`Sandboxes::create`](crate::Sandboxes::create) with the local provider
//! resolves `image = "pool:<name>"` (deprecated spelling `fleet:<name>`), or
//! an empty image with `fleet.pool = <name>`, through
//! [`local_from_fleet_template`].

use crate::{CreateOptions, Error, Probe, Result};
use cua_fleet::{FleetClient, RuntimeKind, SdkError, Template, schema::Firmware};
use serde_json::Value;
use std::collections::BTreeMap;

/// The error for Fleet `macos` templates on local runtimes.
pub const MACOS_FLEET_UNSUPPORTED: &str = "Fleet macOS templates can't run locally; macOS images run locally with Lume (e.g. ghcr.io/trycua/macos-*-cua)";

/// Image-string prefix naming a Fleet pool (or template) to run locally.
pub const POOL_PREFIX: &str = "pool:";

/// Deprecated spelling of [`POOL_PREFIX`] (`fleet:` never names an image
/// any more; it is a legacy sandbox ref spelling).
pub const FLEET_PREFIX: &str = "fleet:";

/// The pool an image string names (`pool:<name>`, or the deprecated
/// `fleet:<name>`, which logs a warning).
pub fn pool_image(image: &str) -> Option<&str> {
    let image = image.trim();
    let name = match image.strip_prefix(POOL_PREFIX) {
        Some(n) => n,
        None => {
            let n = image.strip_prefix(FLEET_PREFIX)?;
            tracing::warn!("image \"{FLEET_PREFIX}{n}\" is deprecated; write \"{POOL_PREFIX}{n}\"");
            n
        }
    };
    Some(name.trim()).filter(|n| !n.is_empty())
}

/// A Fleet template translated for the local provider.
#[derive(Clone, Debug, PartialEq)]
pub struct LocalFleetPlan {
    /// `namespace/name` of the template.
    pub template: String,
    /// Image reference from the template.
    pub reference: String,
    /// Fleet runtime.
    pub runtime: RuntimeKind,
    /// Local image string (`vm:<ref>` or `container:<ref>`).
    pub image: String,
    /// `bios` / `efi`, when the template says.
    pub firmware: Option<String>,
    /// Template vCPUs.
    pub cpus: Option<u32>,
    /// Template memory (MiB).
    pub memory_mb: Option<u64>,
    /// Service name → guest port.
    pub services: BTreeMap<String, u16>,
    /// Readiness probes.
    pub probes: Vec<Probe>,
}

impl LocalFleetPlan {
    /// Applies the plan to local create options. The template's CPU and
    /// memory are capped by the options' values (a laptop default of 2 vCPU
    /// / 4 GiB is not silently raised to a cloud node's 8 GiB); services and
    /// probes are added to the caller's.
    pub fn apply(&self, o: &mut CreateOptions) {
        o.image = self.image.clone();
        if let Some(c) = self.cpus {
            o.cpus = o.cpus.min(c).max(1);
        }
        if let Some(m) = self.memory_mb {
            o.memory_mb = o.memory_mb.min(m).max(256);
        }
        for (k, v) in &self.services {
            o.services.entry(k.clone()).or_insert(*v);
        }
        for p in &self.probes {
            if !o.wait_for.contains(p) {
                o.wait_for.push(p.clone());
            }
        }
        o.firmware = self.firmware.clone();
    }
}

/// The template name an image string / options refer to, if any.
pub fn template_name(o: &CreateOptions) -> Option<&str> {
    if o.image.starts_with(POOL_PREFIX) || o.image.starts_with(FLEET_PREFIX) {
        return pool_image(&o.image);
    }
    if o.image.trim().is_empty() {
        return o.fleet.pool.as_deref().filter(|p| !p.is_empty());
    }
    None
}

/// Parses Kubernetes quantities as MiB (`6Gi`, `8192Mi`, `4G`, `512M`,
/// bytes).
pub fn parse_memory_mb(q: &str) -> Option<u64> {
    let q = q.trim();
    let split = q
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(q.len());
    let (num, unit) = q.split_at(split);
    let n: f64 = num.parse().ok()?;
    let bytes = match unit {
        "" => n,
        "Ki" => n * 1024.0,
        "Mi" => n * 1024.0 * 1024.0,
        "Gi" => n * 1024.0 * 1024.0 * 1024.0,
        "Ti" => n * 1024.0f64.powi(4),
        "k" | "K" => n * 1e3,
        "M" => n * 1e6,
        "G" => n * 1e9,
        "T" => n * 1e12,
        _ => return None,
    };
    Some((bytes / (1024.0 * 1024.0)).round() as u64)
}

/// A named or numeric Kubernetes port, resolved against the services.
fn port_of(v: &Value, services: &BTreeMap<String, u16>) -> Option<u16> {
    match v {
        Value::Number(n) => n.as_u64().and_then(|p| u16::try_from(p).ok()),
        Value::String(s) => s.parse().ok().or_else(|| services.get(s.as_str()).copied()),
        _ => None,
    }
}

/// Readiness probes in a template's `probes` JSON
/// (`{"readinessProbe": {"tcpSocket": {"port": N}}}` or `httpGet`).
pub fn probes_from_json(v: &Value, services: &BTreeMap<String, u16>) -> Vec<Probe> {
    let mut out = Vec::new();
    for key in ["readinessProbe", "startupProbe"] {
        let Some(p) = v.get(key) else { continue };
        if let Some(port) = p
            .get("tcpSocket")
            .and_then(|t| t.get("port"))
            .and_then(|x| port_of(x, services))
        {
            out.push(Probe::Tcp(port));
        } else if let Some(h) = p.get("httpGet")
            && let Some(port) = h.get("port").and_then(|x| port_of(x, services))
        {
            out.push(Probe::Http {
                port,
                path: h
                    .get("path")
                    .and_then(|x| x.as_str())
                    .unwrap_or("/")
                    .to_string(),
                status: None,
            });
        }
    }
    out
}

/// Translates a template for the local provider. Pure; see
/// [`local_from_fleet_template`] for the lookup.
pub fn plan_from_template(t: &Template) -> Result<LocalFleetPlan> {
    let vm = &t.spec.vm_template;
    let reference = vm.container_disk_image.trim().to_string();
    let name = format!("{}/{}", t.metadata.namespace, t.metadata.name);
    if reference.is_empty() {
        return Err(Error::InvalidArgument(format!(
            "Fleet template {name} has no image"
        )));
    }
    let runtime = vm.runtime.clone().unwrap_or(RuntimeKind::Kubevirt);
    let image = match runtime {
        RuntimeKind::Kubevirt => format!("vm:{reference}"),
        RuntimeKind::Gvisor => format!("container:{reference}"),
        RuntimeKind::Macos => {
            return Err(Error::UnsupportedImage(MACOS_FLEET_UNSUPPORTED.into()));
        }
    };
    let services: BTreeMap<String, u16> = vm
        .services
        .iter()
        .flatten()
        .map(|s| (s.name.clone(), s.target_port))
        .collect();
    let probes = vm
        .probes
        .as_ref()
        .map(|p| probes_from_json(p.as_value(), &services))
        .unwrap_or_default();
    Ok(LocalFleetPlan {
        template: name,
        reference,
        runtime: runtime.clone(),
        image,
        firmware: match (runtime, &vm.firmware) {
            (RuntimeKind::Kubevirt, Some(Firmware::Efi)) => Some("efi".into()),
            (RuntimeKind::Kubevirt, Some(Firmware::Bios)) => Some("bios".into()),
            _ => None,
        },
        cpus: vm.cpu_cores,
        memory_mb: vm.memory.as_deref().and_then(parse_memory_mb),
        services,
        probes,
    })
}

fn not_found(e: &cua_fleet::Error) -> bool {
    matches!(
        e,
        cua_fleet::Error::Sdk(SdkError::Status {
            status: 403 | 404,
            ..
        })
    )
}

/// Looks up `name` on Fleet (read-only) as a pool (its template) or, failing
/// that, a template in the namespace of the same name, and translates it
/// with [`plan_from_template`].
pub async fn local_from_fleet_template(fleet: &FleetClient, name: &str) -> Result<LocalFleetPlan> {
    let name = name.trim();
    let template = match fleet.get_pool(name).await {
        Ok(handle) => {
            let ns = if handle.pool.metadata.namespace.is_empty() {
                name.to_string()
            } else {
                handle.pool.metadata.namespace.clone()
            };
            let tname = handle.pool.spec.sandbox_template_ref.name.clone();
            fleet
                .sdk()
                .get_template(ns, tname)
                .await
                .map_err(|e| Error::Fleet(cua_fleet::Error::Sdk(e)))?
        }
        Err(e) if not_found(&e) => match fleet
            .sdk()
            .get_template(name.to_string(), name.to_string())
            .await
        {
            Ok(t) => t,
            Err(SdkError::Status {
                status: 403 | 404, ..
            }) => {
                return Err(Error::NotFound(format!(
                    "no Fleet pool or template named {name:?}"
                )));
            }
            Err(e) => return Err(Error::Fleet(cua_fleet::Error::Sdk(e))),
        },
        Err(e) => return Err(e.into()),
    };
    plan_from_template(&template)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderKind;
    use serde_json::json;

    fn template(vm: Value) -> Template {
        serde_json::from_value(json!({
            "apiVersion": "osgym.cua.ai/v1alpha1",
            "kind": "OSGymSandboxTemplate",
            "metadata": {"namespace": "ns", "name": "t"},
            "spec": {"vmTemplate": vm}
        }))
        .unwrap()
    }

    #[test]
    fn kubevirt_windows_template_maps_to_an_efi_vm() {
        let t = template(json!({
            "containerDiskImage": "public.ecr.aws/k5j5w0x5/cua-windows-2022:main-bac7daa3",
            "runtime": "kubevirt", "firmware": "efi", "cpuCores": 4, "memory": "8Gi",
            "services": [{"name": "server", "targetPort": 8000}, {"name": "mcp", "targetPort": 3000}],
            "probes": {"readinessProbe": {"tcpSocket": {"port": "server"}}}
        }));
        let p = plan_from_template(&t).unwrap();
        assert_eq!(
            p.image,
            "vm:public.ecr.aws/k5j5w0x5/cua-windows-2022:main-bac7daa3"
        );
        assert_eq!(p.firmware.as_deref(), Some("efi"));
        assert_eq!((p.cpus, p.memory_mb), (Some(4), Some(8192)));
        assert_eq!(p.services["server"], 8000);
        assert_eq!(p.probes, vec![Probe::Tcp(8000)]);

        let mut o = CreateOptions::new(ProviderKind::Local, "pool:t");
        p.apply(&mut o);
        assert_eq!(o.image, p.image);
        assert_eq!(
            (o.cpus, o.memory_mb),
            (2, 4096),
            "capped by the local defaults"
        );
        assert_eq!(o.services["mcp"], 3000);
        assert_eq!(o.wait_for, vec![Probe::Tcp(8000)]);
        assert_eq!(o.firmware.as_deref(), Some("efi"));
    }

    #[test]
    fn default_runtime_is_a_bios_containerdisk_and_gvisor_is_a_container() {
        let p = plan_from_template(&template(json!({
            "containerDiskImage": "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest",
            "probes": {"readinessProbe": {"httpGet": {"port": 8000, "path": "/status"}}}
        })))
        .unwrap();
        assert_eq!(
            p.image,
            "vm:public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest"
        );
        assert_eq!(p.firmware, None);
        assert_eq!(
            p.probes,
            vec![Probe::Http {
                port: 8000,
                path: "/status".into(),
                status: None
            }]
        );
        let g = plan_from_template(&template(json!({
            "containerDiskImage": "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest",
            "runtime": "gvisor", "firmware": "efi"
        })))
        .unwrap();
        assert_eq!(
            g.image,
            "container:public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:docker-latest"
        );
        assert_eq!(g.firmware, None, "firmware is advisory for pod runtimes");
    }

    #[test]
    fn macos_template_is_a_typed_unsupported_error() {
        let e = plan_from_template(&template(json!({
            "containerDiskImage": "127.0.0.1:5000/cua/macos-desktop-workspace:latest",
            "runtime": "macos"
        })))
        .unwrap_err();
        assert!(matches!(e, Error::UnsupportedImage(_)));
        assert_eq!(e.to_string(), MACOS_FLEET_UNSUPPORTED);
        assert!(e.to_string().contains("ghcr.io/trycua/macos-*-cua"));
    }

    #[test]
    fn memory_quantities() {
        assert_eq!(parse_memory_mb("6Gi"), Some(6144));
        assert_eq!(parse_memory_mb("8192Mi"), Some(8192));
        assert_eq!(parse_memory_mb("2G"), Some(1907));
        assert_eq!(parse_memory_mb("1073741824"), Some(1024));
        assert_eq!(parse_memory_mb("lots"), None);
    }

    #[test]
    fn template_names_come_from_the_prefix_or_the_pool() {
        let o = CreateOptions::new(ProviderKind::Local, "pool:my-pool");
        assert_eq!(template_name(&o), Some("my-pool"));
        // The deprecated spelling still works.
        let o = CreateOptions::new(ProviderKind::Local, "fleet:my-pool");
        assert_eq!(template_name(&o), Some("my-pool"));
        assert_eq!(pool_image("pool: "), None);
        let mut o = CreateOptions::new(ProviderKind::Local, "");
        o.fleet.pool = Some("p".into());
        assert_eq!(template_name(&o), Some("p"));
        let o = CreateOptions::new(ProviderKind::Local, "nginx:alpine");
        assert_eq!(template_name(&o), None);
    }
}
