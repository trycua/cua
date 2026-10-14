// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`HostBackend`] over the `cua-host` library, signed in with the app's own
//! cua.ai session (the device-grant session in [`crate::auth`]).

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;

use crate::auth::SessionHandle;
use crate::host::{HostBackend, HostSettingChange, HostSetupRequest, HostStatusView};

/// The app's sign-in session as `cua-host` account tokens.
pub struct SessionTokens(pub Arc<SessionHandle>);

#[async_trait]
impl cua_host::AccountTokens for SessionTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        self.0.get_valid_token(false).await.map_err(|e| {
            cua_host::Error::Unauthenticated(format!(
                "sign in to Cua to set this machine up through the relay ({e})"
            ))
        })
    }
}

/// `cua-host` behind the Tauri commands.
pub struct CuaHostBackend {
    host: cua_host::Host,
    tokens: Arc<dyn cua_host::AccountTokens>,
    driver_bin: Option<std::path::PathBuf>,
}

impl CuaHostBackend {
    pub fn new(host: cua_host::Host, tokens: Arc<dyn cua_host::AccountTokens>) -> Self {
        Self {
            host,
            tokens,
            driver_bin: None,
        }
    }

    /// Uses this cua-spacesd binary (else `CUA_SPACESD_BIN`, one bundled
    /// next to the app, or the release download; see `cua-host`).
    pub fn with_driver_bin(mut self, bin: impl Into<std::path::PathBuf>) -> Self {
        self.driver_bin = Some(bin.into());
        self
    }

    /// Setup options for a validated request (see [`crate::host::validate_setup`]).
    pub fn options(request: &HostSetupRequest) -> Result<cua_host::SetupOptions, String> {
        let mut opts = if request.mode == "direct" {
            let listen: SocketAddr = request
                .direct
                .as_deref()
                .unwrap_or_default()
                .parse()
                .map_err(|_| "direct mode needs ip:port".to_string())?;
            cua_host::SetupOptions::direct(listen)
        } else {
            cua_host::SetupOptions::relay(
                request
                    .relay_url
                    .clone()
                    .unwrap_or_else(cua_host::relay_url_from_env),
            )
        };
        opts.name = request.name.clone();
        opts.allow = request.allow.clone().unwrap_or_default();
        (opts.share_desktop, opts.provide_spaces) = request.settings();
        Ok(opts)
    }
}

fn view(status: cua_host::HostStatus) -> Result<HostStatusView, String> {
    HostStatusView::from_serializable(&status)
}

#[async_trait]
impl HostBackend for CuaHostBackend {
    async fn status(&self) -> Result<HostStatusView, String> {
        view(self.host.status().await.map_err(|e| e.to_string())?)
    }
    async fn setup(&self, request: HostSetupRequest) -> Result<HostStatusView, String> {
        let mut opts = Self::options(&request)?;
        opts.driver_bin = self.driver_bin.clone();
        view(
            self.host
                .setup(opts, self.tokens.as_ref())
                .await
                .map_err(|e| e.to_string())?,
        )
    }
    async fn stop_sharing(&self) -> Result<HostStatusView, String> {
        view(self.host.stop_sharing().await.map_err(|e| e.to_string())?)
    }
    async fn start_sharing(&self) -> Result<HostStatusView, String> {
        view(self.host.start_sharing().await.map_err(|e| e.to_string())?)
    }
    async fn remove(&self) -> Result<(), String> {
        self.host.remove().await.map_err(|e| e.to_string())
    }
    async fn configure(&self, change: HostSettingChange) -> Result<HostStatusView, String> {
        view(
            self.host
                .configure(cua_host::HostSettingsChange {
                    share_desktop: change.share_desktop,
                    provide_spaces: change.provide_spaces,
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?,
        )
    }
}
