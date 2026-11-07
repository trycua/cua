// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! How a Space's traffic reaches the other computers of its tree.
//!
//! A tree can span the account's machines. Traffic between them (members
//! talking to each other, an exit Space on another computer, Volume sync)
//! rides this computer's mesh node, which probes every path to each other
//! machine (the local network, direct across the internet, the relay),
//! keeps the fastest that answers and moves within seconds when it stops
//! answering. This module asks the node, on its control socket
//! (`cua_mesh::gang`), what it chose and why, and passes a pin
//! (`cua mesh status --prefer lan|direct|relay|auto`).
//!
//! No mesh node here (signed out, or the mesh is off): no paths, and
//! `cua network show` says the tree stays on this computer.

use serde::Serialize;

/// A path pin.
pub use cua_mesh::Prefer;
/// How traffic reaches one other machine.
pub use cua_mesh::gang::MachinePath;

/// What [`machine_paths`] found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Paths {
    /// Whether this computer's mesh node answered.
    pub mesh: bool,
    /// The pin.
    pub prefer: String,
    /// One per other machine of the account.
    pub machines: Vec<MachinePath>,
    /// Why there are none, in plain words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The control socket of the computer whose Cua home is `home`.
pub fn control_socket(home: &std::path::Path) -> std::path::PathBuf {
    cua_mesh::gang::control_path(home)
}

/// How traffic from this computer reaches each other computer now.
pub async fn machine_paths(home: &std::path::Path) -> Paths {
    #[cfg(unix)]
    {
        let sock = control_socket(home);
        let mut c = match cua_mesh::gang::ControlClient::connect(&sock).await {
            Ok(c) => c,
            Err(_) => {
                return Paths {
                    note: Some(
                        "No mesh on this computer: its Spaces' trees stay on this computer".into(),
                    ),
                    prefer: Prefer::Auto.as_str().into(),
                    ..Default::default()
                };
            }
        };
        match c.status().await {
            Ok((prefer, machines)) => Paths {
                mesh: true,
                prefer: prefer.as_str().into(),
                note: machines
                    .is_empty()
                    .then(|| "No other computers on your account right now".to_string()),
                machines,
            },
            Err(e) => Paths {
                prefer: Prefer::Auto.as_str().into(),
                note: Some(format!("The mesh didn't answer: {e}")),
                ..Default::default()
            },
        }
    }
    #[cfg(not(unix))]
    {
        let _ = home;
        Paths {
            prefer: Prefer::Auto.as_str().into(),
            note: Some("Path details aren't available on this system yet".into()),
            ..Default::default()
        }
    }
}

/// Pins the path to every other machine (`auto` lifts the pin).
pub async fn set_prefer(home: &std::path::Path, prefer: Prefer) -> crate::Result<()> {
    #[cfg(unix)]
    {
        let sock = control_socket(home);
        let mut c = cua_mesh::gang::ControlClient::connect(&sock)
            .await
            .map_err(|_| {
                crate::Error::host(
                    "the mesh",
                    "this computer has no mesh running (sign in, and check `cua mesh status`)",
                )
            })?;
        c.set_prefer(prefer)
            .await
            .map_err(|e| crate::Error::host("the mesh", e))
    }
    #[cfg(not(unix))]
    {
        let _ = (home, prefer);
        Err(crate::Error::Unsupported(
            "a path pin on this system".into(),
        ))
    }
}

/// The paths a Space's tree uses: this computer's paths to the others
/// ([`machine_paths`]); every tree member on another computer is reached
/// over one of them.
pub async fn tree_paths(spaces: &crate::Spaces, _space: &str) -> Paths {
    machine_paths(spaces.home_dir()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_mesh_says_the_tree_stays_here() {
        let dir = tempfile::tempdir().unwrap();
        let p = machine_paths(dir.path()).await;
        assert!(!p.mesh);
        assert!(p.machines.is_empty());
        assert_eq!(p.prefer, "auto");
        assert!(p.note.unwrap().contains("No mesh"), "plain words");
    }
}
