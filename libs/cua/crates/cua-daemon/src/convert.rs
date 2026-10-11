//! Conversions between the runtime types and `cua.daemon.v1`.

use crate::{Error, Result, SandboxRecord};
use cua_proto::daemon::v1 as pb;
use cua_sandbox_core::{Probe, ProviderKind, Status};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Provider → wire.
pub fn provider_to_pb(p: ProviderKind) -> pb::SandboxProvider {
    match p {
        ProviderKind::Fleet => pb::SandboxProvider::Fleet,
        ProviderKind::Local => pb::SandboxProvider::Local,
        ProviderKind::Direct => pb::SandboxProvider::Direct,
        ProviderKind::Contrib => pb::SandboxProvider::Contrib,
    }
}

/// Wire → provider (`None` for unspecified).
pub fn provider_from_pb(p: i32) -> Option<ProviderKind> {
    match pb::SandboxProvider::try_from(p).unwrap_or_default() {
        pb::SandboxProvider::Fleet => Some(ProviderKind::Fleet),
        pb::SandboxProvider::Local => Some(ProviderKind::Local),
        pb::SandboxProvider::Direct => Some(ProviderKind::Direct),
        pb::SandboxProvider::Contrib => Some(ProviderKind::Contrib),
        pb::SandboxProvider::Unspecified => None,
    }
}

pub(crate) fn state_to_pb(s: &Status) -> pb::SandboxState {
    match s {
        Status::Running => pb::SandboxState::Running,
        Status::Suspended | Status::Stopped => pb::SandboxState::Stopped,
        Status::Provisioning => pb::SandboxState::Provisioning,
        Status::Unknown(_) => pb::SandboxState::Unspecified,
    }
}

pub(crate) fn state_from_pb(s: i32, detail: &str) -> Status {
    match pb::SandboxState::try_from(s).unwrap_or_default() {
        pb::SandboxState::Running => Status::Running,
        pb::SandboxState::Stopped => Status::Stopped,
        pb::SandboxState::Provisioning => Status::Provisioning,
        _ => Status::Unknown(detail.to_string()),
    }
}

/// Probe → wire.
pub fn probe_to_pb(p: &Probe) -> pb::ReadinessProbe {
    match p {
        Probe::Tcp(port) => pb::ReadinessProbe {
            port: *port as u32,
            http_path: String::new(),
            http_status: 0,
        },
        Probe::Http { port, path, status } => pb::ReadinessProbe {
            port: *port as u32,
            http_path: path.clone(),
            http_status: status.unwrap_or(0) as u32,
        },
    }
}

/// Wire → probe.
pub fn probe_from_pb(p: &pb::ReadinessProbe) -> Result<Probe> {
    let port = u16::try_from(p.port)
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| Error::InvalidArgument(format!("probe port {} out of range", p.port)))?;
    Ok(if p.http_path.is_empty() {
        Probe::Tcp(port)
    } else {
        Probe::Http {
            port,
            path: p.http_path.clone(),
            status: (p.http_status != 0).then_some(p.http_status as u16),
        }
    })
}

pub(crate) fn ts(t: SystemTime) -> pbjson_types::Timestamp {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    pbjson_types::Timestamp {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

pub(crate) fn from_ts(t: &pbjson_types::Timestamp) -> SystemTime {
    UNIX_EPOCH + Duration::new(t.seconds.max(0) as u64, t.nanos.max(0) as u32)
}

pub(crate) fn dur(d: Duration) -> pbjson_types::Duration {
    pbjson_types::Duration {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

#[allow(dead_code)] // used by the server feature
pub(crate) fn from_dur(d: &pbjson_types::Duration) -> Duration {
    Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32)
}

/// Record → wire.
pub fn info_to_pb(r: &SandboxRecord) -> pb::Sandbox {
    pb::Sandbox {
        name: r.name.clone(),
        provider: provider_to_pb(r.provider) as i32,
        state: state_to_pb(&r.status) as i32,
        image: r.image.clone().unwrap_or_default(),
        endpoints: r.endpoints.clone().into_iter().collect(),
        labels: r.labels.clone().into_iter().collect(),
        created_at: r.created_at.map(ts),
        error: match &r.status {
            Status::Unknown(u) => u.clone(),
            _ => String::new(),
        },
        runtime_type: r.runtime_type.clone(),
        ephemeral: r.ephemeral,
        services: r
            .services
            .iter()
            .map(|(k, v)| (k.clone(), *v as u32))
            .collect(),
        location: r.location.clone(),
        expires_at: r.expires_at.map(ts),
        provider_details: r.provider_details.clone().into_iter().collect(),
        id: r.id.clone(),
        kind: r.kind.clone(),
        runtime: r.runtime.clone(),
    }
}

/// Wire → record.
pub fn info_from_pb(s: &pb::Sandbox) -> SandboxRecord {
    let location = if s.location.is_empty() {
        crate::runtime::location_of(provider_from_pb(s.provider).unwrap_or(ProviderKind::Direct))
            .to_string()
    } else {
        s.location.clone()
    };
    SandboxRecord {
        // A daemon from before qualified ids sends none.
        id: if s.id.is_empty() {
            cua_sandbox_core::refs::canonical(&format!("{location}:{}", s.name))
                .unwrap_or_else(|_| s.name.clone())
        } else {
            s.id.clone()
        },
        name: s.name.clone(),
        provider: provider_from_pb(s.provider).unwrap_or(ProviderKind::Direct),
        runtime_type: s.runtime_type.clone(),
        status: state_from_pb(s.state, &s.error),
        ephemeral: s.ephemeral,
        services: s
            .services
            .iter()
            .map(|(k, v)| (k.clone(), *v as u16))
            .collect(),
        image: (!s.image.is_empty()).then(|| s.image.clone()),
        labels: s.labels.clone().into_iter().collect(),
        endpoints: s.endpoints.clone().into_iter().collect(),
        created_at: s.created_at.as_ref().map(from_ts),
        kind: s.kind.clone(),
        runtime: s.runtime.clone(),
        location,
        expires_at: s.expires_at.as_ref().map(from_ts),
        provider_details: s.provider_details.clone().into_iter().collect(),
        // Not on the daemon wire.
        image_info: None,
    }
}

/// Public URL → wire.
#[allow(dead_code)] // used by the server feature
pub fn public_url_to_pb(u: &crate::shares::PublicUrl) -> pb::PublicUrl {
    pb::PublicUrl {
        id: u.id.clone(),
        url: u.url.clone(),
        expires_at: Some(ts(u.expires_at)),
        sandbox: u.sandbox.clone(),
        service: u.service.clone(),
        provider_details: u.provider_details.clone().into_iter().collect(),
    }
}

/// Wire → public URL.
pub fn public_url_from_pb(u: &pb::PublicUrl) -> crate::shares::PublicUrl {
    crate::shares::PublicUrl {
        id: u.id.clone(),
        url: u.url.clone(),
        expires_at: u.expires_at.as_ref().map(from_ts).unwrap_or(UNIX_EPOCH),
        sandbox: u.sandbox.clone(),
        service: u.service.clone(),
        provider_details: u.provider_details.clone().into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn record_roundtrips() {
        let r = SandboxRecord {
            id: "cloud:dev".into(),
            name: "dev".into(),
            provider: ProviderKind::Fleet,
            runtime_type: "fleet".into(),
            status: Status::Running,
            ephemeral: true,
            services: [("env".to_string(), 3211u16)].into(),
            image: Some("img".into()),
            labels: BTreeMap::new(),
            endpoints: [("env".to_string(), "https://x".to_string())].into(),
            created_at: Some(UNIX_EPOCH + Duration::from_secs(5)),
            location: "cloud".into(),
            kind: "vm".into(),
            runtime: "kubevirt".into(),
            expires_at: Some(UNIX_EPOCH + Duration::from_secs(905)),
            provider_details: [("claim".to_string(), "c-1".to_string())].into(),
            image_info: None,
        };
        assert_eq!(info_from_pb(&info_to_pb(&r)), r);
        // An older daemon sends no id: it is derived from the location.
        let mut old = info_to_pb(&r);
        old.id.clear();
        assert_eq!(info_from_pb(&old).id, "cloud:dev");
    }

    #[test]
    fn probes_roundtrip() {
        for p in [
            Probe::Tcp(22),
            Probe::Http {
                port: 8000,
                path: "/status".into(),
                status: Some(200),
            },
        ] {
            assert_eq!(probe_from_pb(&probe_to_pb(&p)).unwrap(), p);
        }
        assert!(probe_from_pb(&pb::ReadinessProbe::default()).is_err());
    }
}

/// A sidecar from the wire (an empty name derives from the image).
pub fn sidecar_from_pb(s: pb::Sidecar) -> Result<cua_sandbox_core::Sidecar> {
    let ports = s
        .ports
        .iter()
        .map(|p| {
            u16::try_from(*p)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| Error::InvalidArgument(format!("sidecar port {p}")))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut c = cua_sandbox_core::Sidecar::new(s.image);
    if !s.name.is_empty() {
        c.name = s.name;
    }
    c.command = (!s.command.is_empty()).then_some(s.command);
    c.env = s.env.into_iter().collect();
    c.ports = ports;
    Ok(c)
}

/// A sidecar to the wire.
pub fn sidecar_to_pb(s: &cua_sandbox_core::Sidecar) -> pb::Sidecar {
    pb::Sidecar {
        name: s.name.clone(),
        image: s.image.clone(),
        command: s.command.clone().unwrap_or_default(),
        env: s.env.clone().into_iter().collect(),
        ports: s.ports.iter().map(|p| u32::from(*p)).collect(),
    }
}

/// Registry credentials from the wire.
pub fn registry_secret_from_pb(
    s: pb::RegistrySecret,
) -> Result<cua_sandbox_core::RegistryCredentials> {
    if s.username.is_empty() || s.password.is_empty() {
        return Err(Error::InvalidArgument(
            "registry secret needs a username and a password".into(),
        ));
    }
    let c = cua_sandbox_core::RegistryCredentials::new(s.username, s.password);
    Ok(if s.registry.is_empty() {
        c
    } else {
        c.for_registry(s.registry)
    })
}

/// Registry credentials to the wire (loopback daemon only).
pub fn registry_secret_to_pb(c: &cua_sandbox_core::RegistryCredentials) -> pb::RegistrySecret {
    pb::RegistrySecret {
        registry: c.registry.clone().unwrap_or_default(),
        username: c.username.clone(),
        password: c.password.clone(),
    }
}

fn layer_from_pb(l: pb::ImageLayer) -> Result<cua_image::spec::ImageLayer> {
    use cua_image::spec::ImageLayer as L;
    Ok(match l.r#type.as_str() {
        "apt_install" => L::AptInstall {
            packages: l.packages,
        },
        "pip_install" => L::PipInstall {
            packages: l.packages,
        },
        "uv_install" => L::UvInstall {
            packages: l.packages,
        },
        "run" => L::Run { command: l.command },
        "app_install" => L::AppInstall { app_id: l.app_id },
        other => {
            return Err(Error::InvalidArgument(format!(
                "unknown image layer type {other:?}"
            )));
        }
    })
}

fn layer_to_pb(l: &cua_image::spec::ImageLayer) -> pb::ImageLayer {
    use cua_image::spec::ImageLayer as L;
    let mut out = pb::ImageLayer::default();
    match l {
        L::AptInstall { packages } => {
            out.r#type = "apt_install".into();
            out.packages = packages.clone();
        }
        L::PipInstall { packages } => {
            out.r#type = "pip_install".into();
            out.packages = packages.clone();
        }
        L::UvInstall { packages } => {
            out.r#type = "uv_install".into();
            out.packages = packages.clone();
        }
        L::Run { command } => {
            out.r#type = "run".into();
            out.command = command.clone();
        }
        L::AppInstall { app_id } => {
            out.r#type = "app_install".into();
            out.app_id = app_id.clone();
        }
    }
    out
}

/// Image layers from the wire (`from` is the sandbox's image).
pub fn build_from_pb(b: pb::ImageBuild) -> Result<cua_sandbox_core::BuildSpec> {
    Ok(cua_sandbox_core::BuildSpec {
        from: String::new(),
        layers: b
            .layers
            .into_iter()
            .map(layer_from_pb)
            .collect::<Result<_>>()?,
        env: b.env.into_iter().collect(),
        ports: b
            .ports
            .iter()
            .map(|p| {
                u16::try_from(*p).map_err(|_| Error::InvalidArgument(format!("image port {p}")))
            })
            .collect::<Result<_>>()?,
        files: b
            .files
            .into_iter()
            .map(|f| cua_sandbox_core::BuildFile {
                source: f.source.into(),
                destination: f.destination,
            })
            .collect(),
        timeout: b.timeout.as_ref().map(from_dur),
    })
}

/// Image layers to the wire.
pub fn build_to_pb(b: &cua_sandbox_core::BuildSpec) -> pb::ImageBuild {
    pb::ImageBuild {
        layers: b.layers.iter().map(layer_to_pb).collect(),
        env: b.env.clone().into_iter().collect(),
        ports: b.ports.iter().map(|p| u32::from(*p)).collect(),
        files: b
            .files
            .iter()
            .map(|f| pb::BuildFile {
                source: f.source.display().to_string(),
                destination: f.destination.clone(),
            })
            .collect(),
        timeout: b.timeout.map(dur),
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn sidecars_secrets_and_builds_round_trip() {
        let mut c = cua_sandbox_core::Sidecar::new("redis:7-alpine");
        c.ports = vec![6379];
        c.env.insert("A".into(), "1".into());
        c.command = Some(vec!["redis-server".into()]);
        assert_eq!(sidecar_from_pb(sidecar_to_pb(&c)).unwrap(), c);
        let unnamed = pb::Sidecar {
            image: "ghcr.io/me/db:1".into(),
            ..Default::default()
        };
        assert_eq!(sidecar_from_pb(unnamed).unwrap().name, "db");
        assert!(
            sidecar_from_pb(pb::Sidecar {
                image: "x".into(),
                ports: vec![70000],
                ..Default::default()
            })
            .is_err()
        );

        let creds = cua_sandbox_core::RegistryCredentials::new("u", "p").for_registry("ghcr.io");
        assert_eq!(
            registry_secret_from_pb(registry_secret_to_pb(&creds)).unwrap(),
            creds
        );
        assert!(registry_secret_from_pb(pb::RegistrySecret::default()).is_err());

        let b = cua_sandbox_core::BuildSpec {
            from: String::new(),
            layers: vec![
                cua_sandbox_core::ImageLayer::PipInstall {
                    packages: vec!["mcp".into()],
                },
                cua_sandbox_core::ImageLayer::Run {
                    command: "true".into(),
                },
            ],
            env: [("K".to_string(), "v".to_string())].into(),
            ports: vec![8765],
            files: vec![cua_sandbox_core::BuildFile {
                source: "/tmp/x".into(),
                destination: "/x".into(),
            }],
            timeout: Some(Duration::from_secs(60)),
        };
        assert_eq!(build_from_pb(build_to_pb(&b)).unwrap(), b);
        let bad = pb::ImageBuild {
            layers: vec![pb::ImageLayer {
                r#type: "brew_install".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(build_from_pb(bad).is_err());
    }
}
