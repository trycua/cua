// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Share edges this computer joins (CUA-2602): small servers in the owner's
//! own cloud that carry public TCP shares of Spaces on this computer, so the
//! traffic never crosses Cua's relay.
//!
//! The cua daemon writes them into the host policy (`edges`: URL, token
//! file, machine id); this watcher joins each one exactly as a computer joins
//! a relay (the same tunnel, the same `/share/pub/<token>/` route to the
//! daemon's share gateway) and leaves the ones the policy no longer lists.
//! An edge only ever asks this computer for public share paths; it holds no
//! relay keys, so nothing else it sends is authorized.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;

/// One edge to join.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
pub struct EdgeJoin {
    /// `wss://<edge host>`.
    pub url: String,
    /// The edge's token, in a file only the owner can read.
    pub token_file: PathBuf,
    /// This computer's id on the edge.
    pub machine_id: String,
}

#[derive(Deserialize, Default)]
struct PolicyEdges {
    #[serde(default)]
    edges: Vec<EdgeJoin>,
    #[serde(default = "yes")]
    sharing: bool,
}

fn yes() -> bool {
    true
}

/// The edges `policy` lists (none while sharing is stopped).
pub fn edges_of(policy: &str) -> Vec<EdgeJoin> {
    let p: PolicyEdges = serde_json::from_str(policy).unwrap_or_default();
    if !p.sharing {
        return Vec::new();
    }
    p.edges
        .into_iter()
        .filter(|e| {
            (e.url.starts_with("wss://") || e.url.starts_with("ws://127.0.0.1"))
                && cua_relay::valid_machine_id(&e.machine_id)
        })
        .collect()
}

/// Joins the edges the policy at `policy` lists, every 15 s, until `stop`.
pub async fn run(policy: Option<PathBuf>, local: SocketAddr, stop: CancellationToken) {
    let Some(policy) = policy else { return };
    let mut joined: HashMap<EdgeJoin, CancellationToken> = HashMap::new();
    loop {
        let want = std::fs::read_to_string(&policy)
            .map(|t| edges_of(&t))
            .unwrap_or_default();
        joined.retain(|e, s| {
            let keep = want.contains(e);
            if !keep {
                tracing::info!(edge = %e.url, "leaving share edge");
                s.cancel();
            }
            keep
        });
        for e in want {
            if joined.contains_key(&e) {
                continue;
            }
            let s = stop.child_token();
            let mut join = cua_relay::client::JoinConfig::new(
                e.url.clone(),
                String::new(),
                e.machine_id.clone(),
                local,
            );
            join.version = env!("CARGO_PKG_VERSION").into();
            join.relay_token_file = Some(e.token_file.clone());
            tracing::info!(edge = %e.url, "joining share edge");
            tokio::spawn(cua_relay::client::run(join, s.clone()));
            joined.insert(e, s);
        }
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(15)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tls_edges_with_valid_ids_and_only_while_sharing() {
        let p = r#"{"sharing": true, "edges": [
            {"url": "wss://ec2-1.example", "token_file": "/x", "machine_id": "machine-0001"},
            {"url": "http://plain.example", "token_file": "/x", "machine_id": "machine-0001"},
            {"url": "wss://ec2-2.example", "token_file": "/x", "machine_id": "BAD ID"}
        ]}"#;
        let e = edges_of(p);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].url, "wss://ec2-1.example");
        assert!(edges_of(&p.replace("\"sharing\": true", "\"sharing\": false")).is_empty());
        assert!(edges_of("{}").is_empty());
        assert!(edges_of("not json").is_empty());
    }
}
