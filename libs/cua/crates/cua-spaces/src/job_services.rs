//! Host services a job member reaches on its own loopback
//! (`JobSpec::services`, for example `compat:e2b`).
//!
//! The binary that hosts the daemon registers one [`JobServices`] provider
//! ([`register`]); core code never depends on what a service is. For each
//! service a member names, the host's runner:
//!
//! 1. asks the provider to start it for the member's lifetime: it answers
//!    with the host loopback ports it listens on, environment variables and
//!    files for the member (paths relative to its work directory;
//!    `{work}` in a value is the work directory as the member sees it);
//! 2. starts the member Space's hotspot with a dialer that reaches only
//!    `127.0.0.1:<those ports>` on this host and nothing else;
//! 3. runs the member's command under the jobs runtime's port forwarder
//!    (`_cua_ports.py`), which listens on the same ports on the guest's
//!    loopback and connects through the hotspot;
//! 4. drops the service and the hotspot when the member ends.
//!
//! This uses only what every image with cua-spacesd has (the hotspot), so
//! it works on every kind and OS a member runs on.

use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

/// What a started service gives the member.
pub struct ServiceGrant {
    /// Host loopback ports the member may reach (the same numbers on its
    /// own loopback).
    pub ports: Vec<u16>,
    /// Environment for the member's command (`{work}`: its work directory).
    pub env: BTreeMap<String, String>,
    /// Files written into its work directory before it starts.
    pub files: Vec<(String, Vec<u8>)>,
    /// Kept until the member ends; dropping it stops the service.
    pub guard: Box<dyn std::any::Any + Send + Sync>,
}

/// Starts named services for job members.
#[async_trait]
pub trait JobServices: Send + Sync + 'static {
    /// Called as soon as a member is accepted, before its Space is made:
    /// a service that needs minutes of one-time work (an image pull, a
    /// first boot) does it now, in step with the Space's own, instead of
    /// after it. Best effort; [`JobServices::start`] still reports errors
    /// and waits for whatever this began. Default: nothing.
    async fn prepare(&self, _service: &str, _job: &cua_cluster::JobSpec) {}

    /// Starts `service` for the member `job`; an `Err` fails the member
    /// before its command starts, with that message.
    async fn start(
        &self,
        service: &str,
        job: &cua_cluster::JobSpec,
    ) -> Result<ServiceGrant, String>;
}

static PROVIDER: OnceLock<Arc<dyn JobServices>> = OnceLock::new();

/// Registers this process's provider (once; later calls are ignored).
pub fn register(provider: Arc<dyn JobServices>) {
    let _ = PROVIDER.set(provider);
}

/// The registered provider.
pub fn provider() -> Option<Arc<dyn JobServices>> {
    PROVIDER.get().cloned()
}

/// The port forwarder the member's command runs under (stdlib Python).
pub const FORWARDER: &str = include_str!("job_services/_cua_ports.py");
/// Where it goes in the work directory.
pub const FORWARDER_PATH: &str = ".cua-services/_cua_ports.py";

/// Replaces `{work}` with the member's work directory.
pub fn expand(value: &str, work: &str) -> String {
    value.replace("{work}", work)
}

/// The command a member runs with services: the forwarder (under
/// `python3`, else uv's Python) listening on `ports` and relaying through
/// the hotspot's SOCKS address, then the member's own command.
pub fn wrap(argv: &[String], work: &str, socks: &str, ports: &BTreeSet<u16>) -> Vec<String> {
    let ports = ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let fwd = format!("{work}/{FORWARDER_PATH}");
    let mut v = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        "f=\"$1\"; shift; if command -v python3 >/dev/null 2>&1; then exec python3 \"$f\" \"$@\"; \
         else exec uv run --no-project python \"$f\" \"$@\"; fi"
            .to_string(),
        "sh".to_string(),
        fwd,
        "--socks".to_string(),
        socks.to_string(),
        "--ports".to_string(),
        ports,
        "--".to_string(),
    ];
    v.extend(argv.iter().cloned());
    v
}

/// A hotspot dialer that reaches only `127.0.0.1:<port>` for `ports`.
pub fn loopback_dialer(ports: BTreeSet<u16>) -> crate::hotspot::Dialer {
    let ports = Arc::new(ports);
    Arc::new(move |host: String, port: u16| {
        let ports = ports.clone();
        Box::pin(async move {
            let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
            if !loopback || !ports.contains(&port) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{host}:{port} is not a service of this job"),
                ));
            }
            tokio::net::TcpStream::connect(("127.0.0.1", port)).await
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_the_command_after_the_forwarder() {
        let ports: BTreeSet<u16> = [49443, 49982].into();
        let v = wrap(
            &["python".into(), "smoke.py".into()],
            "/cua/work",
            "127.0.0.1:1080",
            &ports,
        );
        assert_eq!(v[0], "/bin/sh");
        assert_eq!(v[4], "/cua/work/.cua-services/_cua_ports.py");
        let i = v.iter().position(|a| a == "--ports").unwrap();
        assert_eq!(v[i + 1], "49443,49982");
        assert_eq!(&v[v.len() - 3..], &["--", "python", "smoke.py"]);
    }

    #[tokio::test]
    async fn the_dialer_reaches_only_the_services_ports() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let d = loopback_dialer([port].into());
        assert!(d("127.0.0.1".into(), port).await.is_ok());
        let e = d("127.0.0.1".into(), port.wrapping_add(1))
            .await
            .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        let e = d("example.com".into(), port).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn expands_the_work_directory() {
        assert_eq!(expand("{work}/x.pem", "/cua/work"), "/cua/work/x.pem");
    }
}
