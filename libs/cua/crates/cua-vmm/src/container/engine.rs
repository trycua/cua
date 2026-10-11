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

/// Docker Desktop's engine pipe on Windows (the docker CLI's `default`
/// context).
#[cfg(windows)]
const WINDOWS_ENGINE_PIPE: &str = r"\\.\pipe\docker_engine";

/// How to get a container engine on this host, for "no engine found".
pub fn install_hint() -> &'static str {
    match std::env::consts::OS {
        "linux" => {
            "install Docker Engine (`curl -fsSL https://get.docker.com | sh`, then \
             `sudo systemctl enable --now docker`) or Podman (`systemctl --user enable --now \
             podman.socket`)"
        }
        "macos" => {
            "install one (`brew install colima docker && colima start`, Docker Desktop or \
             OrbStack), or set `runtime.linux` to `auto` to use the built-in Linux runtime"
        }
        "windows" => {
            "install Docker Desktop (https://docs.docker.com/desktop/setup/install/windows-install/) \
             and start it"
        }
        _ => "install Docker or Podman and start it",
    }
}

/// How to start the engine `kind`, for "the engine does not answer".
pub fn start_hint(kind: &EngineKind) -> &'static str {
    match kind {
        EngineKind::Colima { .. } => "start it with `colima start`",
        EngineKind::DockerDesktop => "open Docker Desktop and wait until it says it is running",
        EngineKind::OrbStack => "start it with `orb start`",
        EngineKind::Podman if cfg!(target_os = "linux") => {
            "start it with `systemctl --user start podman.socket`"
        }
        EngineKind::Podman => "start it with `podman machine start`",
        EngineKind::NativeLinux => "start it with `sudo systemctl start docker`",
        EngineKind::Unknown if cfg!(target_os = "linux") => {
            "start it (`sudo systemctl start docker`, or `systemctl --user start podman.socket`)"
        }
        EngineKind::Unknown => "start your container engine",
    }
}

/// How to get access to `socket`, for "permission denied".
pub fn permission_hint(socket: &str) -> String {
    if cfg!(target_os = "linux") {
        format!(
            "permission denied on {socket}: add yourself to the `docker` group \
             (`sudo usermod -aG docker $USER`, then log out and back in), or use rootless \
             Docker or Podman"
        )
    } else {
        format!("permission denied on {socket}: check that your user may use the engine's socket")
    }
}

/// What a connect to a Unix engine socket found before speaking HTTP.
#[derive(Debug, PartialEq, Eq)]
pub enum SocketCheck {
    /// Something accepted the connection.
    Accepting,
    /// The socket file is gone (or dangling): no engine is installed or running.
    Missing,
    /// Nothing listens on it: the engine is installed but not running.
    NotListening,
    /// This user may not connect.
    PermissionDenied,
    /// Any other error (kept as text).
    Other(String),
}

/// Connects to the Unix socket behind `uri` (`unix:///path` or a path)
/// without speaking HTTP, so "not running" and "permission denied" are
/// told apart before the ping. `None` for other URIs (tcp, npipe).
pub fn check_socket(uri: &str) -> Option<SocketCheck> {
    #[cfg(unix)]
    {
        let path = uri
            .strip_prefix("unix://")
            .or_else(|| uri.starts_with('/').then_some(uri))?;
        Some(match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => SocketCheck::Accepting,
            Err(e) => match e.kind() {
                std::io::ErrorKind::PermissionDenied => SocketCheck::PermissionDenied,
                std::io::ErrorKind::NotFound => SocketCheck::Missing,
                std::io::ErrorKind::ConnectionRefused => SocketCheck::NotListening,
                _ => SocketCheck::Other(e.to_string()),
            },
        })
    }
    #[cfg(not(unix))]
    {
        let _ = uri;
        None
    }
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
    // A context whose socket is gone is an engine that is installed but
    // stopped (Docker Desktop quit): used only when no other socket is up,
    // so the create can say to start it rather than to install one.
    let mut stopped_context = None;
    if let Some((name, h)) = context_endpoint(&docker_dir, env_ctx.as_deref()) {
        let path = h.trim_start_matches("unix://");
        let endpoint = EngineEndpoint {
            kind: classify(&h),
            uri: h.clone(),
            source: format!("context:{name}"),
        };
        if !h.starts_with("unix://") || Path::new(path).exists() {
            return Some(endpoint);
        }
        stopped_context = Some(endpoint);
    }
    #[cfg(windows)]
    if Path::new(WINDOWS_ENGINE_PIPE).exists() {
        return Some(EngineEndpoint {
            uri: "npipe:////./pipe/docker_engine".into(),
            source: "socket".into(),
            kind: EngineKind::DockerDesktop,
        });
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
        .or(stopped_context)
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

    #[cfg(unix)]
    #[test]
    fn check_socket_tells_not_running_from_missing_and_up() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("docker.sock");
        let uri = format!("unix://{}", path.display());
        assert_eq!(check_socket(&uri), Some(SocketCheck::Missing));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert_eq!(check_socket(&uri), Some(SocketCheck::Accepting));
        // The engine stopped and left its socket file behind.
        drop(listener);
        assert_eq!(check_socket(&uri), Some(SocketCheck::NotListening));
        assert_eq!(check_socket("tcp://127.0.0.1:2375"), None);
    }

    #[test]
    fn hints_name_this_hosts_commands() {
        if cfg!(target_os = "linux") {
            assert!(install_hint().contains("get.docker.com"));
            assert!(start_hint(&EngineKind::NativeLinux).contains("systemctl start docker"));
            assert!(permission_hint("unix:///var/run/docker.sock").contains("usermod -aG docker"));
        }
        if cfg!(target_os = "macos") {
            assert!(install_hint().contains("colima"));
        }
        assert!(
            start_hint(&EngineKind::Colima {
                profile: "default".into()
            })
            .contains("colima start")
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
