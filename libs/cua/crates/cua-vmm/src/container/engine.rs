//! Container-engine discovery: find the Docker-API socket the user's CLI would
//! use (DOCKER_HOST → docker context → well-known sockets) and classify what
//! kind of engine sits behind it (Colima, Docker Desktop, OrbStack, Podman,
//! native Linux). The kind decides how gVisor can be provisioned.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::host;

/// What runs the engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineKind {
    /// Colima VM (profile name).
    Colima {
        profile: String,
    },
    DockerDesktop,
    OrbStack,
    Podman,
    /// dockerd directly on a Linux host.
    NativeLinux,
    Unknown,
}

/// A discovered engine endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineEndpoint {
    /// `unix:///path`, `npipe:////./pipe/...`, `tcp://...`.
    pub uri: String,
    /// Where the endpoint came from (`DOCKER_HOST`, `context:<name>`, `socket`).
    pub source: String,
    pub kind: EngineKind,
}

/// Classify an engine from its socket path/URI.
pub fn classify(uri: &str) -> EngineKind {
    let path = uri.trim_start_matches("unix://");
    if let Some(idx) = path.find("/.colima/") {
        let rest = &path[idx + "/.colima/".len()..];
        let profile = rest.split('/').next().unwrap_or("default");
        let profile = if profile == "docker.sock" {
            "default"
        } else {
            profile
        };
        return EngineKind::Colima {
            profile: profile.to_string(),
        };
    }
    if path.contains("/.orbstack/") {
        return EngineKind::OrbStack;
    }
    if path.contains("podman") {
        return EngineKind::Podman;
    }
    if path.contains("/.docker/run/docker.sock")
        || path.contains("docker.raw.sock")
        || uri.contains("dockerDesktop")
    {
        return EngineKind::DockerDesktop;
    }
    if cfg!(target_os = "linux") && (path == "/var/run/docker.sock" || path == "/run/docker.sock") {
        return EngineKind::NativeLinux;
    }
    EngineKind::Unknown
}

/// Read the docker CLI's current context endpoint from `docker_config_dir`
/// (`~/.docker`). Returns `(context_name, host_uri)`.
pub fn context_endpoint(
    docker_config_dir: &Path,
    env_context: Option<&str>,
) -> Option<(String, String)> {
    let name = match env_context {
        Some(n) => n.to_string(),
        None => {
            let cfg: serde_json::Value =
                serde_json::from_slice(&std::fs::read(docker_config_dir.join("config.json")).ok()?)
                    .ok()?;
            cfg.get("currentContext")?.as_str()?.to_string()
        }
    };
    if name == "default" {
        return None;
    }
    // Context metadata lives in contexts/meta/<sha256(name)>/meta.json; scan
    // rather than hash so we need no digest dependency here.
    let meta_root = docker_config_dir.join("contexts").join("meta");
    for entry in std::fs::read_dir(meta_root).ok()?.flatten() {
        let Ok(raw) = std::fs::read(entry.path().join("meta.json")) else {
            continue;
        };
        let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            continue;
        };
        if meta.get("Name").and_then(|v| v.as_str()) == Some(name.as_str()) {
            let host = meta
                .pointer("/Endpoints/docker/Host")?
                .as_str()?
                .to_string();
            return Some((name, host));
        }
    }
    None
}

/// Well-known engine sockets, most specific first.
fn well_known_sockets() -> Vec<PathBuf> {
    let home = host::home_dir();
    let mut v = vec![
        home.join(".colima/default/docker.sock"),
        home.join(".colima/docker.sock"),
        home.join(".orbstack/run/docker.sock"),
        home.join(".docker/run/docker.sock"),
        PathBuf::from("/var/run/docker.sock"),
        PathBuf::from("/run/docker.sock"),
    ];
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        v.push(PathBuf::from(&rt).join("docker.sock"));
        v.push(PathBuf::from(rt).join("podman/podman.sock"));
    }
    v
}

/// Find the engine endpoint the way the docker CLI would.
pub fn discover() -> Option<EngineEndpoint> {
    if let Ok(h) = std::env::var("DOCKER_HOST")
        && !h.is_empty()
    {
        return Some(EngineEndpoint {
            kind: classify(&h),
            uri: h,
            source: "DOCKER_HOST".into(),
        });
    }
    let docker_dir = std::env::var_os("DOCKER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| host::home_dir().join(".docker"));
    let env_ctx = std::env::var("DOCKER_CONTEXT").ok();
    if let Some((name, h)) = context_endpoint(&docker_dir, env_ctx.as_deref()) {
        let path = h.trim_start_matches("unix://");
        if !h.starts_with("unix://") || Path::new(path).exists() {
            return Some(EngineEndpoint {
                kind: classify(&h),
                uri: h,
                source: format!("context:{name}"),
            });
        }
    }
    well_known_sockets()
        .into_iter()
        .find(|p| p.exists())
        .map(|p| {
            let uri = format!("unix://{}", p.display());
            EngineEndpoint {
                kind: classify(&uri),
                uri,
                source: "socket".into(),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_engines() {
        assert_eq!(
            classify("unix:///Users/me/.colima/default/docker.sock"),
            EngineKind::Colima {
                profile: "default".into()
            }
        );
        assert_eq!(
            classify("unix:///Users/me/.colima/work/docker.sock"),
            EngineKind::Colima {
                profile: "work".into()
            }
        );
        assert_eq!(
            classify("unix:///Users/me/.orbstack/run/docker.sock"),
            EngineKind::OrbStack
        );
        assert_eq!(
            classify("unix:///Users/me/.docker/run/docker.sock"),
            EngineKind::DockerDesktop
        );
        assert_eq!(
            classify("unix:///run/user/1000/podman/podman.sock"),
            EngineKind::Podman
        );
    }

    #[test]
    fn reads_current_context_from_docker_config() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("config.json"),
            r#"{"auths":{},"currentContext":"colima"}"#,
        )
        .unwrap();
        let meta = d.path().join("contexts/meta/abc123");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(
            meta.join("meta.json"),
            r#"{"Name":"colima","Metadata":{},"Endpoints":{"docker":{"Host":"unix:///x/.colima/default/docker.sock","SkipTLSVerify":false}}}"#,
        )
        .unwrap();
        let (name, host) = context_endpoint(d.path(), None).unwrap();
        assert_eq!(name, "colima");
        assert_eq!(host, "unix:///x/.colima/default/docker.sock");
        assert!(context_endpoint(d.path(), Some("default")).is_none());
        assert!(context_endpoint(d.path(), Some("missing")).is_none());
    }
}
