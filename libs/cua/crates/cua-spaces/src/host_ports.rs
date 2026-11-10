//! The host's ports at `localhost` inside a Space.
//!
//! The `localhost` network preset opens ports of the computer that runs the
//! Space (`localhost:3000`). They are reachable at the name `host` on every
//! runtime ([`crate::topology::HOST_NAME`]). Where the Space's daemon can
//! ([`FEATURE`]), they also answer at `localhost:3000` inside the Space, so
//! an app written for a local dev server works unchanged:
//!
//! - the guest listens on `127.0.0.1:<port>` for each listed port, and
//!   relays each connection over a ticketed WebSocket to this process;
//! - this process connects it to the same port on this computer's own
//!   loopback, which also works for a host app bound to loopback only;
//! - nothing else is ever dialed: only the listed ports, never the Docker
//!   API or another local service, and `localhost:all` publishes nothing
//!   (it stays reachable at `host:<port>` with its warning).
//!
//! A port something in the Space already listens on is skipped; that port
//! stays reachable at `host:<port>`, and `cua network show` says so.
//!
//! A container whose tree has members it can't reach by a route (VMs, and
//! members on other computers) gets one more port in the same session: the
//! tree proxy ([`crate::tree_proxy`]), a SOCKS5 server this process runs
//! for it, never a connection to anything on this computer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};

use crate::error::{Error, Result};
use crate::space::Space;
use cua_spacesd_client::pb;

/// The capability a guest that publishes host ports advertises.
pub const FEATURE: &str = crate::topology::HOST_PORTS_FEATURE;

/// What one Space's published host ports look like now.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct HostPortsStatus {
    /// Answering at `localhost:<port>` inside the Space.
    pub at_localhost: Vec<u16>,
    /// Reachable at `host:<port>` only, with why.
    pub at_host_only: Vec<(u16, String)>,
    /// The tree proxy answers at `127.0.0.1:41920` inside the Space
    /// ([`crate::tree_proxy`]).
    pub tree_proxy: bool,
}

/// One Space's session.
pub(crate) struct HostPortsRun {
    ports: Vec<u16>,
    status: HostPortsStatus,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for HostPortsRun {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The sessions this process keeps.
pub(crate) type HostPortsTable = tokio::sync::Mutex<BTreeMap<String, HostPortsRun>>;

/// The ports a Space's settings publish at `localhost` (the `localhost`
/// preset's listed ports; `all` publishes none).
pub fn published_ports(settings: &crate::network::NetworkSettings) -> Vec<u16> {
    if !settings.preset.opens_localhost() || settings.all_localhost_ports {
        return Vec::new();
    }
    let mut v = settings.localhost_ports.clone();
    v.sort_unstable();
    v.dedup();
    v
}

impl Space {
    /// Publishes `ports` of this computer's loopback at the same ports on
    /// this Space's loopback, until the returned task ends (or is aborted).
    pub async fn publish_host_ports(
        &self,
        ports: &[u16],
        proxy: Option<crate::Spaces>,
    ) -> Result<(HostPortsStatus, tokio::task::JoinHandle<()>)> {
        self.require(FEATURE)?;
        let resp = self
            .spacesd()?
            .tunnel()
            .start_host_ports(pb::StartHostPortsRequest {
                ports: ports.iter().map(|p| u32::from(*p)).collect(),
                ticket_ttl: None,
            })
            .await
            .map_err(cua_spacesd_client::Error::from)?
            .into_inner();
        let mut request = self
            .websocket_url(&resp.ws_path)?
            .into_client_request()
            .map_err(|e| Error::Stream(e.to_string()))?;
        for (k, v) in self.websocket_headers().await? {
            if let (Ok(k), Ok(v)) = (
                http::HeaderName::from_bytes(k.as_bytes()),
                http::HeaderValue::from_str(&v),
            ) {
                request.headers_mut().insert(k, v);
            }
        }
        cua_spacesd_client::transport::ensure_crypto_provider();
        let (ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| Error::Stream(format!("host ports socket: {e}")))?;
        let (sink, stream) = ws.split();
        let sink = sink.with(|bytes: Vec<u8>| async move {
            Ok::<_, tungstenite::Error>(tungstenite::Message::Binary(bytes.into()))
        });
        let source = stream.filter_map(|m| async move {
            match m {
                Ok(tungstenite::Message::Binary(b)) => Some(b.to_vec()),
                _ => None,
            }
        });
        let allowed: Arc<BTreeSet<u16>> = Arc::new(
            resp.listening
                .iter()
                .filter_map(|p| u16::try_from(*p).ok())
                .collect(),
        );
        let id = self.id().to_string();
        let proxy_for = id.clone();
        let task = tokio::spawn(async move {
            cua_hotspot::peer::serve_streams(
                Box::pin(source),
                Box::pin(sink),
                move |host: String, port: u16| {
                    let allowed = allowed.clone();
                    let proxy = proxy.clone();
                    let proxy_for = proxy_for.clone();
                    async move {
                        // The guest names its own loopback; this side only
                        // ever dials the same port on this computer's.
                        if !allowed.contains(&port) || host != "127.0.0.1" {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::PermissionDenied,
                                format!("{host}:{port} is not a host port this Space may reach"),
                            ));
                        }
                        // The tree proxy's port is served here, in memory.
                        if port == crate::tree_proxy::PORT
                            && let Some(spaces) = proxy
                        {
                            return Ok(crate::tree_proxy::proxy_stream(spaces, proxy_for));
                        }
                        tokio::net::TcpStream::connect(("127.0.0.1", port))
                            .await
                            .map(crate::hotspot::boxed)
                    }
                },
            )
            .await;
            tracing::info!(space = %id, "host ports socket closed");
        });
        let status = HostPortsStatus {
            tree_proxy: resp.listening.contains(&u32::from(crate::tree_proxy::PORT)),
            at_localhost: resp
                .listening
                .iter()
                .filter_map(|p| u16::try_from(*p).ok())
                .filter(|p| *p != crate::tree_proxy::PORT)
                .collect(),
            at_host_only: resp
                .skipped
                .iter()
                .filter_map(|s| Some((u16::try_from(s.port).ok()?, s.reason.clone())))
                .filter(|(p, _)| *p != crate::tree_proxy::PORT)
                .collect(),
        };
        Ok((status, task))
    }
}

impl crate::Spaces {
    /// Brings Space `key`'s published host ports in line with its settings
    /// (started, changed, re-attached after its daemon restarted, or
    /// stopped).
    pub async fn reconcile_host_ports(&self, key: &str) -> Result<()> {
        let settings = crate::network::settings_of(self.inner.registry.meta(key).as_ref());
        let mut ports = published_ports(&settings);
        let running = self.is_running(key);
        // A container that can't reach part of its tree by a route gets the
        // tree proxy on its loopback.
        let proxy = running && self.wants_tree_proxy(key, &settings).await;
        tracing::debug!(space = %key, running, proxy, ports = ?ports, "host ports reconcile");
        if proxy {
            ports.push(crate::tree_proxy::PORT);
        }
        let mut table = self.inner.host_ports.lock().await;
        if ports.is_empty() || !running {
            table.remove(key);
            return Ok(());
        }
        if table
            .get(key)
            .is_some_and(|r| r.ports == ports && !r.task.is_finished())
        {
            return Ok(());
        }
        table.remove(key);
        let space = self.space(key).await?;
        if !space.supports(FEATURE) {
            // Reachable at `host:<port>` only (the gateway or the runtime's
            // own name for the host); `network show` says so.
            return Ok(());
        }
        let (status, task) = space
            .publish_host_ports(&ports, proxy.then(|| self.clone()))
            .await?;
        table.insert(
            key.to_string(),
            HostPortsRun {
                ports,
                status,
                task,
            },
        );
        Ok(())
    }

    /// Whether Space `key` is a container that gets the tree proxy: its tree
    /// has members it can't reach by a route ([`crate::tree_proxy::wanted`]).
    pub(crate) async fn wants_tree_proxy(
        &self,
        key: &str,
        settings: &crate::network::NetworkSettings,
    ) -> bool {
        let Ok(crate::SpaceId::Local { name }) = crate::SpaceId::parse(key) else {
            return false;
        };
        let tree = self.book_tree_of(key);
        let members = cua_vmm::tree::running_members(&tree);
        let linked = self.linked_trees().contains(&tree);
        if !crate::tree_proxy::wanted(settings, &members, linked) {
            return false;
        }
        match self.inner.sandboxes.connect(&name).await {
            Ok(sb) => {
                crate::network::backend_kind(sb.runtime_type())
                    == Some(cua_vmm::BackendKind::Container)
            }
            Err(_) => false,
        }
    }

    /// What Space `key`'s host ports look like now.
    pub async fn host_ports_status(&self, key: &str) -> Option<HostPortsStatus> {
        self.inner
            .host_ports
            .lock()
            .await
            .get(key)
            .map(|r| r.status.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{NetworkPreset, NetworkSettings};

    #[test]
    fn only_listed_localhost_ports_are_published() {
        let mut s = NetworkSettings::preset(NetworkPreset::Localhost);
        s.localhost_ports = vec![5432, 3000, 3000];
        assert_eq!(published_ports(&s), vec![3000, 5432]);
        s.all_localhost_ports = true;
        s.localhost_ports.clear();
        assert!(
            published_ports(&s).is_empty(),
            "all ports: none at localhost"
        );
        assert!(published_ports(&NetworkSettings::internet()).is_empty());
    }
}
