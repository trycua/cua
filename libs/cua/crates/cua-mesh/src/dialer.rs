// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`Dialer`]: "reach machine X on relay R over the mesh", for any client
//! (Spaces, the cluster coordinator, the SDK).
//!
//! It runs one client node on the relay's mesh and keeps one loopback
//! forward per machine. A machine of a static-token relay is paired first
//! with the machine's env token (only a proof leaves this process). The
//! caller then talks to `http://<Route::local>` exactly as it would to the
//! machine's spacesd, sending [`Route::session`] (when set) as the
//! `x-cua-mesh-session` header so the machine vouches for this device.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::{AuthProvider, Error, Forward, Mesh, MeshConfig, NodeKind, Result};

/// The spacesd service on a machine.
pub const SPACESD_SERVICE: &str = "spacesd";
/// The request header carrying a forward's session key.
pub const SESSION_HEADER: &str = "x-cua-mesh-session";

/// Where to reach one machine.
#[derive(Debug, Clone)]
pub struct Route {
    /// Loopback address (TCP for spacesd; UDP on the same port reaches the
    /// machine's QUIC media port when it was free).
    pub local: SocketAddr,
    /// Send as [`SESSION_HEADER`] on every request (account relays).
    pub session: Option<String>,
    /// The machine's node key.
    pub peer: crate::PublicKey,
}

struct Entry {
    forward: Arc<Forward>,
    route: Route,
}

/// A client node on one relay's mesh plus its forwards.
pub struct Dialer {
    mesh: Mesh,
    forwards: tokio::sync::Mutex<HashMap<String, Entry>>,
}

impl std::fmt::Debug for Dialer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialer").field("mesh", &self.mesh).finish()
    }
}

impl Drop for Dialer {
    fn drop(&mut self) {
        self.mesh.shutdown();
    }
}

impl Dialer {
    /// Joins `relay`'s mesh as a client with `auth` (account relays: the
    /// account token and device session headers; static relays: none).
    /// `identity`: a persistent node identity file, or `None` for a
    /// short-lived node the relay forgets on disconnect.
    pub async fn start(
        relay: &str,
        auth: AuthProvider,
        identity: Option<PathBuf>,
        timeout: Duration,
    ) -> Result<Self> {
        let cfg = MeshConfig::new(relay.to_owned(), NodeKind::Client, identity, auth);
        let mesh = Mesh::start(cfg).await?;
        if let Err(e) = mesh.wait_ready(timeout).await {
            mesh.shutdown();
            return Err(e);
        }
        Ok(Self {
            mesh,
            forwards: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    /// The node.
    pub fn mesh(&self) -> &Mesh {
        &self.mesh
    }

    /// The route to `machine_id`, opening a forward on first use. With
    /// `env_token` (a static relay), pairs first when the machine is not in
    /// the map yet.
    pub async fn route(
        &self,
        machine_id: &str,
        env_token: Option<&str>,
        wait: Duration,
    ) -> Result<Route> {
        let mut forwards = self.forwards.lock().await;
        if let Some(e) = forwards.get(machine_id)
            && self.mesh.machine(machine_id).map(|p| p.node_key) == Some(e.route.peer)
        {
            return Ok(e.route.clone());
        }
        // A static relay's machine is in the map only once paired: pair
        // right away instead of waiting for a map that will not have it.
        let mut peer = match env_token {
            Some(_) => self.mesh.machine(machine_id),
            None => self.mesh.wait_machine_in_map(machine_id, wait).await,
        };
        if peer.is_none()
            && let Some(token) = env_token
        {
            self.mesh.pair(machine_id, token).await?;
            peer = self.mesh.wait_machine(machine_id, wait).await;
        }
        let peer = peer.ok_or_else(|| {
            Error::NotFound(format!(
                "computer {machine_id} is not on the mesh (an older cua-spacesd, offline, or not shared)"
            ))
        })?;
        if peer.version != crate::VERSION {
            return Err(Error::Unsupported(format!(
                "computer {machine_id} speaks mesh v{}, this node v{}",
                peer.version,
                crate::VERSION
            )));
        }
        // Account relays: the machine vouches for this device only on
        // requests carrying the forward's key. Static relays: the env token
        // in each request is the credential.
        let session = env_token
            .is_none()
            .then(|| hex::encode(crate::key::os_random::<24>()));
        let forward = Arc::new(
            self.mesh
                .forward(peer.node_key, SPACESD_SERVICE, session.clone())
                .await?,
        );
        let route = Route {
            local: forward.local,
            session,
            peer: peer.node_key,
        };
        forwards.insert(
            machine_id.to_owned(),
            Entry {
                forward,
                route: route.clone(),
            },
        );
        Ok(route)
    }

    /// Drops the forward to `machine_id` (its path stopped answering).
    pub async fn forget(&self, machine_id: &str) {
        if let Some(e) = self.forwards.lock().await.remove(machine_id) {
            drop(e.forward);
        }
    }
}
