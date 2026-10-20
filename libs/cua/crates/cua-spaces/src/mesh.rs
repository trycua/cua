// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Relay Spaces over the WireGuard®-based mesh (`cua-mesh`).
//!
//! When a relay coordinates a mesh and the machine is on it (same protocol
//! version), the Space is reached through a loopback forward into the mesh:
//! WireGuard end to end, direct when the two NATs allow it, through the
//! relay as ciphertext otherwise.
//!
//! - `relay:<machine-id>` (an account relay): the machine's mesh endpoint
//!   identifies this device from the relay's network map, so the account
//!   token never goes to the machine; a per-forward session key (sent as a
//!   request header) keeps another local user from borrowing the forward.
//! - A Space added by `<relay>/m/<machine-id>` with its env token (a
//!   self-hosted static relay): this device pairs with the machine by
//!   proving it holds the env token (the machine checks it; the relay only
//!   passes the proof along), then sends the env token inside WireGuard.
//!
//! The mesh is off unless asked for ([`cua_mesh::policy`]): `CUA_MESH`
//! picks the policy, and without it the Cua Spaces app's daemon follows the
//! app's update channel (off on every channel in this release):
//! - `off` (default): the relay's HTTPS path only, as before the mesh;
//! - `prefer`: the mesh when both sides support it, the relay's HTTPS path
//!   otherwise (an older relay or machine, or a mesh that does not answer),
//!   with a log line saying so;
//! - `require`: the mesh or nothing (a relay that can read sessions is
//!   never used).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_mesh::dialer::{Dialer, SESSION_HEADER};
use cua_spacesd_client::{ConnectOptions, SpacesdClient};

use crate::relay::RelayAccount;

/// How relay Spaces use the mesh (`CUA_MESH`, see [`cua_mesh::policy`]).
pub use cua_mesh::policy::Policy as MeshPolicy;

/// Set on the cua daemon the Cua Spaces app starts.
const STARTED_BY_ENV: &str = "CUA_DAEMON_STARTED_BY";

/// The policy for this process now: `CUA_MESH`, else (in the daemon the
/// Cua Spaces app started) the app's update channel, else off. Read on
/// every connect, so a change applies to the next connection.
pub fn policy() -> MeshPolicy {
    let app = std::env::var(STARTED_BY_ENV).is_ok_and(|v| v == "app");
    let home = app.then(cua_home::cua_home);
    MeshPolicy::current(home.as_deref())
}

/// Which node: the account's (one per relay) or an anonymous one for a
/// static relay.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Account(String),
    Static(String),
}

enum Slot {
    Ready {
        dialer: Arc<Dialer>,
        /// Held while the node lives: one persistent identity per home.
        _lock: Option<std::fs::File>,
    },
    /// The mesh cannot be used (the relay has none, or refused this
    /// device), and why; checked again after a while.
    Unsupported(Instant, String),
}

const RECHECK: Duration = Duration::from_secs(600);

/// Per-runtime mesh nodes, started on first use.
#[derive(Default)]
pub(crate) struct MeshClients {
    nodes: tokio::sync::Mutex<HashMap<Key, Slot>>,
    /// Spaces connected over the mesh, and through which node.
    routed: std::sync::Mutex<HashMap<String, Key>>,
}

/// How a relay Space connects, chosen by [`connect_options`].
pub(crate) enum Route {
    /// Through the mesh: these options reach the machine's spacesd.
    Mesh(Box<ConnectOptions>),
    /// The relay's HTTPS path.
    Relay,
}

/// Who a relay Space connects as.
pub(crate) enum Via<'a> {
    /// `relay:<id>`: the signed-in account.
    Account(&'a RelayAccount),
    /// `<relay>/m/<id>` with the Space's env token.
    Static {
        /// Relay base URL.
        relay: &'a str,
        /// The env token.
        token: &'a str,
    },
}

fn key_of(via: &Via<'_>) -> Key {
    match via {
        Via::Account(a) => Key::Account(a.url.clone()),
        Via::Static { relay, .. } => Key::Static((*relay).to_owned()),
    }
}

impl MeshClients {
    /// Records that Space `space` (its id) connected over `via`'s mesh.
    pub(crate) fn note_routed(&self, space: &str, via: &Via<'_>) {
        self.routed
            .lock()
            .expect("routed")
            .insert(space.to_owned(), key_of(via));
    }

    /// Why a Space connected over the mesh must reconnect: its node can no
    /// longer use the mesh (the relay turned it off, or refused this
    /// device). Reconnecting applies `CUA_MESH` again, so `prefer` moves
    /// to the relay's HTTPS path and `require` says why it cannot.
    pub(crate) async fn stale(
        &self,
        space: &str,
        policy: impl FnOnce() -> MeshPolicy,
    ) -> Option<String> {
        let key = self.routed.lock().expect("routed").get(space).cloned()?;
        let reason = if !policy().uses_mesh() {
            Some("the mesh was turned off".into())
        } else {
            None
        };
        let reason = match reason {
            Some(r) => Some(r),
            None => self.unusable(&key).await,
        }?;
        self.routed.lock().expect("routed").remove(space);
        Some(reason)
    }

    /// Why Space `space` (its id) lost the mesh, when it was connected over
    /// it and its node can no longer be used (a revoked device, the mesh
    /// turned off). Waits up to `wait` for the node to learn it: a call cut
    /// by the relay fails a moment before the relay's refusal arrives.
    /// Leaves the Space marked, so its next use reconnects.
    pub(crate) async fn cut_reason(&self, space: &str, wait: Duration) -> Option<String> {
        let key = self.routed.lock().expect("routed").get(space).cloned()?;
        let deadline = Instant::now() + wait;
        loop {
            if let Some(reason) = self.unusable(&key).await {
                return Some(reason);
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Why `key`'s node can no longer be used, if it cannot.
    async fn unusable(&self, key: &Key) -> Option<String> {
        match self.nodes.lock().await.get(key) {
            Some(Slot::Ready { dialer, .. }) => dialer.mesh().unusable(),
            _ => Some("the mesh node is gone".into()),
        }
    }

    async fn dialer(&self, via: &Via<'_>) -> Result<Arc<Dialer>, String> {
        let key = key_of(via);
        let mut nodes = self.nodes.lock().await;
        match nodes.get(&key) {
            // A node the relay no longer coordinates (mesh turned off, or
            // this device refused): start over, which checks the relay.
            Some(Slot::Ready { dialer, .. }) if dialer.mesh().unusable().is_some() => {
                // Stop it now (forwards still holding it end): the next node
                // takes over this home's identity.
                dialer.mesh().shutdown();
                nodes.remove(&key);
            }
            Some(Slot::Ready { dialer, .. }) => return Ok(dialer.clone()),
            Some(Slot::Unsupported(at, why)) if at.elapsed() < RECHECK => {
                return Err(why.clone());
            }
            _ => {}
        }
        match start(via).await {
            Ok((d, lock)) => {
                let d = Arc::new(d);
                nodes.insert(
                    key,
                    Slot::Ready {
                        dialer: d.clone(),
                        _lock: lock,
                    },
                );
                Ok(d)
            }
            Err(e) => {
                nodes.insert(key, Slot::Unsupported(Instant::now(), e.clone()));
                Err(e)
            }
        }
    }

    /// The status of each node, for displays and measurements.
    pub(crate) async fn status(&self) -> Vec<(String, cua_mesh::Status)> {
        self.nodes
            .lock()
            .await
            .iter()
            .filter_map(|(key, slot)| match (key, slot) {
                (Key::Account(url) | Key::Static(url), Slot::Ready { dialer, .. }) => {
                    Some((url.clone(), dialer.mesh().status()))
                }
                _ => None,
            })
            .collect()
    }
}

async fn relay_has_mesh(url: &str) -> Result<(), String> {
    let info = cua_host::RelayClient::new(url)
        .map_err(|e| e.to_string())?
        .info()
        .await
        .map_err(|e| e.to_string())?;
    match info.mesh {
        Some(m) if m.version == cua_mesh::VERSION => Ok(()),
        Some(m) => Err(format!(
            "the relay speaks mesh v{}, this cua v{}",
            m.version,
            cua_mesh::VERSION
        )),
        None => Err("the relay has no mesh coordinator".into()),
    }
}

async fn start(via: &Via<'_>) -> Result<(Dialer, Option<std::fs::File>), String> {
    let (url, auth): (String, cua_mesh::AuthProvider) = match via {
        Via::Account(account) => {
            let account = (*account).clone();
            let url = account.url.clone();
            let auth: cua_mesh::AuthProvider = Arc::new(move || {
                let account = account.clone();
                Box::pin(async move {
                    let token = account
                        .tokens
                        .access_token()
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut headers = vec![(
                        "x-cua-relay-authorization".to_owned(),
                        format!("Bearer {token}"),
                    )];
                    // Asked once per (re)connection to the coordinator: a
                    // fresh session, since a relay that restarted (the usual
                    // reason to reconnect) forgot the cached one.
                    if let Some(device) = &account.device {
                        device.reset_session().await;
                    }
                    if let Some(session) = account.device_session().await {
                        headers.push((cua_host::relay::DEVICE_SESSION_HEADER.to_owned(), session));
                    }
                    Ok(headers)
                })
            });
            (url, auth)
        }
        Via::Static { relay, .. } => ((*relay).to_owned(), cua_mesh::static_auth(Vec::new())),
    };
    relay_has_mesh(&url).await?;
    // One persistent node per Cua home and kind; a second process (a CLI
    // run next to the daemon) gets a short-lived one the relay forgets.
    let name = match via {
        Via::Account(_) => "client",
        Via::Static { .. } => "static-client",
    };
    let dir = cua_home::cua_home().join("mesh");
    let (identity, lock) = match lock_identity(&dir, name) {
        Some(lock) => (Some(dir.join(format!("{name}.json"))), Some(lock)),
        None => (None, None),
    };
    let dialer = Dialer::start(&url, auth, identity, Duration::from_secs(10))
        .await
        .map_err(|e| e.to_string())?;
    Ok((dialer, lock))
}

fn lock_identity(dir: &std::path::Path, name: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("{name}.lock")))
        .ok()?;
    f.try_lock().ok()?;
    Some(f)
}

/// Picks the route to `machine_id`. With [`MeshPolicy::Require`] an
/// unusable mesh is an error, never a silent fallback.
pub(crate) async fn connect_options(
    clients: &MeshClients,
    via: Via<'_>,
    machine_id: &str,
    policy: MeshPolicy,
) -> crate::Result<Route> {
    if policy == MeshPolicy::Off {
        return Ok(Route::Relay);
    }
    match try_mesh(clients, &via, machine_id).await {
        Ok(options) => Ok(Route::Mesh(Box::new(options))),
        Err(reason) if policy == MeshPolicy::Require => Err(crate::Error::host(
            "the mesh",
            format!(
                "computer {machine_id} is not reachable over the mesh (CUA_MESH=require): {reason}"
            ),
        )),
        Err(reason) => {
            tracing::info!(machine = %machine_id, %reason, "using the relay's HTTPS path");
            Ok(Route::Relay)
        }
    }
}

async fn try_mesh(
    clients: &MeshClients,
    via: &Via<'_>,
    machine_id: &str,
) -> Result<ConnectOptions, String> {
    let dialer = clients.dialer(via).await?;
    let token = match via {
        Via::Static { token, .. } => Some(*token),
        Via::Account(_) => None,
    };
    let route = dialer
        .route(machine_id, token, Duration::from_secs(3))
        .await
        .map_err(|e| e.to_string())?;
    let mut options =
        ConnectOptions::parse(&format!("http://{}", route.local)).map_err(|e| e.to_string())?;
    if let Some(session) = &route.session {
        options = options.header(SESSION_HEADER, session.clone());
    }
    if let Some(token) = token {
        options = options.token(token);
    }
    // Check the path answers before committing to it.
    let probe = options.clone().probe_timeout(Duration::from_secs(8));
    match SpacesdClient::connect(probe).await {
        Ok(_) => Ok(options),
        Err(e) => {
            dialer.forget(machine_id).await;
            Err(format!("the mesh path did not answer: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(url: &str) -> RelayAccount {
        RelayAccount::new(url, Arc::new(cua_host::relay::StaticToken("t".into())))
    }

    #[tokio::test]
    async fn policy_off_never_touches_the_mesh() {
        let clients = MeshClients::default();
        let a = account("http://127.0.0.1:9");
        let route = connect_options(&clients, Via::Account(&a), "m1", MeshPolicy::Off)
            .await
            .unwrap();
        assert!(matches!(route, Route::Relay));
    }

    #[tokio::test]
    async fn a_relay_without_a_mesh_falls_back_unless_required() {
        // The fake relay's /v1/info has no `mesh`: an older relay.
        let relay = cua_host::testing::FakeRelay::start().await;
        let clients = MeshClients::default();
        let a = account(&relay.url);
        let route = connect_options(&clients, Via::Account(&a), "m1", MeshPolicy::Prefer)
            .await
            .unwrap();
        assert!(matches!(route, Route::Relay));
        let err = connect_options(&clients, Via::Account(&a), "m1", MeshPolicy::Require)
            .await
            .err()
            .expect("require refuses the plaintext path");
        assert!(err.to_string().contains("CUA_MESH=require"), "{err}");
        let route = connect_options(
            &clients,
            Via::Static {
                relay: &relay.url,
                token: "env",
            },
            "m1",
            MeshPolicy::Prefer,
        )
        .await
        .unwrap();
        assert!(matches!(route, Route::Relay));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_space_on_a_mesh_the_relay_turned_off_reconnects() {
        let coord = cua_mesh::testing::FakeCoordinator::start(cua_mesh::Mode::Account).await;
        let dialer = Dialer::start(
            &coord.url,
            cua_mesh::static_auth(Vec::new()),
            None,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        let clients = MeshClients::default();
        let key = Key::Account(coord.url.clone());
        clients.nodes.lock().await.insert(
            key.clone(),
            Slot::Ready {
                dialer: Arc::new(dialer),
                _lock: None,
            },
        );
        clients
            .routed
            .lock()
            .unwrap()
            .insert("relay:m1".into(), key);
        assert!(
            clients
                .stale("relay:m1", || MeshPolicy::Prefer)
                .await
                .is_none(),
            "mesh still on"
        );

        // The relay turns its mesh off.
        coord.set_mesh(false);
        // A call cut at that moment is explained (the node is unusable).
        let cut = clients
            .cut_reason("relay:m1", Duration::from_secs(5))
            .await
            .expect("the cut is explained");
        assert!(cut.contains("no mesh coordinator"), "{cut}");
        // Spaces that never used the mesh are never explained this way.
        assert!(
            clients
                .cut_reason("relay:other", Duration::from_secs(5))
                .await
                .is_none()
        );
        let mut reason = None;
        for _ in 0..200 {
            reason = clients.stale("relay:m1", || MeshPolicy::Prefer).await;
            if reason.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let reason = reason.expect("the connection is reported stale");
        assert!(reason.contains("no mesh coordinator"), "{reason}");
        // Reported once: the reconnect picks the route again.
        assert!(
            clients
                .stale("relay:m1", || MeshPolicy::Prefer)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_space_on_the_mesh_reconnects_when_the_mesh_is_turned_off() {
        let clients = MeshClients::default();
        clients
            .routed
            .lock()
            .unwrap()
            .insert("relay:m1".into(), Key::Account("http://relay".into()));
        // Off now (CUA_MESH=off, or the app left the channel that uses it):
        // the cached connection is dropped and reconnects over HTTPS.
        let reason = clients.stale("relay:m1", || MeshPolicy::Off).await;
        assert_eq!(reason.as_deref(), Some("the mesh was turned off"));
        assert!(
            clients
                .stale("relay:m1", || MeshPolicy::Off)
                .await
                .is_none()
        );
        // A Space that never used the mesh is never stale.
        assert!(
            clients
                .stale("relay:m2", || MeshPolicy::Off)
                .await
                .is_none()
        );
    }

    #[test]
    fn the_policy_reads_cua_mesh() {
        assert_eq!(MeshPolicy::parse("off"), Some(MeshPolicy::Off));
        assert_eq!(MeshPolicy::parse(" REQUIRE "), Some(MeshPolicy::Require));
        assert_eq!(MeshPolicy::parse(""), None);
        assert_eq!(MeshPolicy::parse("prefer"), Some(MeshPolicy::Prefer));
    }
}
