// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who may use the daemon's socket as the user in the Cua Spaces build.
//!
//! A release build signed by Cua asks the OS: the peer's audit token (so a
//! recycled pid or a later `exec` does not match) is turned into a code
//! object and checked against the Cua requirement (Apple-anchored, the Cua
//! team, hardened runtime, a Cua signing identifier: the app, its `cua`, the
//! standalone CLI). Anything else is held to the approval policy. A build
//! that is not itself signed by Cua (a developer's) cannot tell, so it
//! trusts the programs of its own bundle, like the open daemon does.

use cua_daemon::caller::{Caller, PeerVerifier, SameProgram};

/// The peer check of the Cua Spaces daemon.
#[derive(Debug)]
pub struct CuaPeers {
    fallback: SameProgram,
    #[cfg(target_os = "macos")]
    signed_by_cua: bool,
}

impl CuaPeers {
    /// The check for this process.
    pub fn new() -> Self {
        Self {
            fallback: SameProgram::new(),
            #[cfg(target_os = "macos")]
            signed_by_cua: matches!(
                cua_keyvault::macos::self_signing(),
                cua_keyvault::caller::Signing::Signed { ref team_id, .. }
                    if team_id == cua_keyvault::caller::CUA_TEAM_ID
            ),
        }
    }
}

impl Default for CuaPeers {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerVerifier for CuaPeers {
    #[cfg(target_os = "macos")]
    fn verify(&self, fd: i32) -> Caller {
        if !self.signed_by_cua {
            return self.fallback.verify(fd);
        }
        let policy = cua_keyvault::caller::TrustPolicy::production();
        match cua_keyvault::caller::identify_peer(fd, &policy) {
            Ok(id) if id.first_party => Caller::User,
            Ok(id) => Caller::Agent(
                id.path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".into()),
            ),
            Err(_) => Caller::Agent("unknown".into()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn verify(&self, fd: i32) -> Caller {
        self.fallback.verify(fd)
    }
}
