// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Sign in to Cua" on the SDK's `cua-auth`: browser sign-in (authorization
//! code + PKCE with a loopback redirect) with a device-code fallback, and
//! the ONE credential store the cua CLI, the SDK and `cua daemon` share (OS
//! vault on macOS and Windows, a 0600 `~/.cua/credentials.json` on Linux).
//! The app's former `~/.cua/spaces-session.json` is migrated on startup.

use std::sync::Arc;

pub use cua_auth::{Credentials, Flow, Method, PendingLogin, Store};

/// Why a token could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Nobody is signed in.
    NoSession,
    /// The session existed but could not be refreshed (revoked, expired or
    /// the identity provider is unreachable).
    Refresh(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::NoSession => write!(f, "not signed in"),
            TokenError::Refresh(reason) => write!(f, "session refresh failed: {reason}"),
        }
    }
}

/// The shared, refreshable user session. [`crate::core::AppTokens`] pulls
/// the Fleet bearer from this one handle; the store is shared with the CLI
/// and the daemon, so a sign-in or sign-out anywhere is seen everywhere.
pub struct SessionHandle {
    session: cua_auth::Session,
}

impl SessionHandle {
    /// A handle on `store` (issuer and client from `CUA_OIDC_ISSUER` /
    /// `CUA_OIDC_CLIENT_ID`). Returns the handle and whether a session is
    /// stored.
    pub fn new(store: Store) -> (Arc<Self>, bool) {
        Self::with_oidc(cua_auth::Oidc::from_env(), store)
    }

    /// With an explicit issuer (tests: a fake one).
    pub fn with_oidc(oidc: cua_auth::Oidc, store: Store) -> (Arc<Self>, bool) {
        let signed_in = matches!(store.load(), Ok(Some(_)));
        (
            Arc::new(Self {
                session: cua_auth::Session::new(oidc, store),
            }),
            signed_in,
        )
    }

    /// The signed-in identity (email, else username), if any.
    pub async fn identity(&self) -> Option<String> {
        self.session.identity().and_then(|i| i.display())
    }

    /// The signed-in account's claims (name, email, username, subject).
    pub fn profile(&self) -> Option<cua_auth::Identity> {
        self.session.identity()
    }

    /// Starts a sign-in (browser, else device code).
    pub async fn begin_login(&self) -> Result<PendingLogin, String> {
        self.session
            .begin_login(Flow::Auto)
            .await
            .map_err(|e| e.to_string())
    }

    /// Stores freshly issued credentials; returns the display identity.
    pub async fn install(&self, c: Credentials) -> Result<Option<String>, String> {
        self.session
            .install(c)
            .await
            .map(|i| i.display())
            .map_err(|e| e.to_string())
    }

    /// Signs out (revokes best effort) everywhere.
    pub async fn clear(&self) {
        if let Err(e) = self.session.logout().await {
            tracing::warn!("sign-out: {e}");
        }
    }

    /// The current access token, refreshed when it expires or `force`d.
    pub async fn get_valid_token(&self, force: bool) -> Result<String, TokenError> {
        self.session.access_token(force).await.map_err(|e| match e {
            cua_auth::Error::NotLoggedIn => TokenError::NoSession,
            other => TokenError::Refresh(other.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_file_store_session_round_trips_and_signs_out() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::File(d.path().join("credentials.json"));
        // Nothing listens on port 9: revocation fails quietly.
        let (h, signed_in) = SessionHandle::with_oidc(
            cua_auth::Oidc::new("http://127.0.0.1:9", "cua-cli"),
            store.clone(),
        );
        assert!(!signed_in);
        assert_eq!(h.get_valid_token(false).await, Err(TokenError::NoSession));
        let c = Credentials::from_token_response(&serde_json::json!({
            "access_token": "h.eyJlbWFpbCI6ImFAYi5jIn0.s", "expires_in": 3600
        }))
        .unwrap();
        assert_eq!(h.install(c).await.unwrap().as_deref(), Some("a@b.c"));
        assert_eq!(h.identity().await.as_deref(), Some("a@b.c"));
        assert!(h.get_valid_token(false).await.is_ok());
        let (_, again) = SessionHandle::with_oidc(
            cua_auth::Oidc::new("http://127.0.0.1:9", "cua-cli"),
            store.clone(),
        );
        assert!(again, "another handle (the CLI) sees the same session");
        h.clear().await;
        assert!(store.load().unwrap().is_none());
    }
}
