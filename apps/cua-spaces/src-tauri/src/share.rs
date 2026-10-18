// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sharing a Space with other cua.ai accounts (the Share sheet). The sheet
//! is the app core's (`share.*`, shared with the SwiftUI app); these
//! commands run its requests through the app's Spaces runtime and the
//! account's relay.
//!
//! Sharing hands the Space to someone else, so [`AppShareConsent`] asks for
//! presence on macOS (Touch ID or the login password). Elsewhere the Share
//! button the person just pressed in this app is the confirmation: this
//! runtime is not reachable by agents (they use the daemon, which asks for
//! presence itself).

use cua_spaces::share::{ShareConsent, ShareRole, SpaceShares};

use crate::core::{AppCore, CmdResult};

/// Confirms a share for the app's own Spaces runtime.
pub struct AppShareConsent;

impl ShareConsent for AppShareConsent {
    fn confirm(&self, reason: &str) -> Result<(), String> {
        if cfg!(target_os = "macos") {
            crate::biometric::authorize(reason)
        } else {
            Ok(())
        }
    }
}

fn msg(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl AppCore {
    /// Who `space` is shared with.
    pub async fn space_shares(&self, space: &str) -> CmdResult<SpaceShares> {
        self.spaces().space_shares(space).await.map_err(msg)
    }

    /// Shares `space` with `who` as `role` (`viewer` or `editor`).
    pub async fn share_space(&self, space: &str, who: &str, role: &str) -> CmdResult<SpaceShares> {
        let role = ShareRole::parse(role).map_err(msg)?;
        self.spaces()
            .share_space(space, who, role)
            .await
            .map_err(msg)
    }

    /// Stops sharing `space` with `who`.
    pub async fn unshare_space(&self, space: &str, who: &str) -> CmdResult<SpaceShares> {
        self.spaces()
            .unshare_space(space, Some(who))
            .await
            .map_err(msg)
    }
}
