// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Spaces build of the `cua` command (source-available,
//! FSL-1.1-MIT): the MIT `cua` ([`cua_cli`]) with the Cua Spaces
//! extensions registered.
//!
//! - `cua daemon` hosts the Keyvault broker, the Cua Volume, teleport and the
//!   persistent-agent supervisor ([`cua_spaces_ext::daemon`]);
//! - `cua teleport`, `cua keyvault` and `cua volume config` run here
//!   ([`CuaSpacesCli`]); the MIT `cua` hands them to this one;
//! - the MCP `teleport_browser_session` tool files requests with the
//!   Keyvault ([`teleport_session::KeyvaultBroker`]);
//! - `cua viewer` serves the standalone HTML5 viewer ([`viewer`]) and `cua
//!   sb stream-probe` decodes the stream (`cua_spaces_ffi::media_decode`).
//!
//! The Spaces apps ship this binary as their `cua`.

use std::io::Write;
use std::sync::Arc;

use cua_cli::drive_cmd::ConfigCmd;
use cua_cli::extension::{CliExtension, DecodedFrames};
use cua_cli::keyvault_cmd::KeyvaultCmd;
use cua_cli::teleport::TeleportCmd;
use cua_cli::teleport_session::Broker;
use cua_sdk::{Cua, CuaError, MediaEvent, MediaOpenOptions, MediaSession, SpacesdClient};

pub mod drive_config;
pub mod keyvault_cmd;
pub mod teleport;
pub mod teleport_session;
pub mod viewer;

/// The Cua Spaces commands.
pub struct CuaSpacesCli;

#[async_trait::async_trait(?Send)]
impl CliExtension for CuaSpacesCli {
    async fn teleport(
        &self,
        cua: Option<&Cua>,
        cmd: TeleportCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError> {
        teleport::run(cua, cmd, json, out).await
    }

    async fn keyvault(
        &self,
        cmd: KeyvaultCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError> {
        keyvault_cmd::run(cmd, json, out).await
    }

    fn drive_config(
        &self,
        cmd: ConfigCmd,
        json: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError> {
        drive_config::config(cmd, json, out)
    }

    fn session_broker(&self) -> Option<Arc<dyn Broker>> {
        #[cfg(unix)]
        {
            Some(Arc::new(teleport_session::KeyvaultBroker))
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    async fn viewer(
        &self,
        cua: Arc<Cua>,
        listen: &str,
        no_open: bool,
        out: &mut dyn Write,
    ) -> Result<i32, CuaError> {
        viewer::serve(cua, listen, no_open, out).await
    }

    async fn open_media_decoded(
        &self,
        env: Arc<SpacesdClient>,
        options: MediaOpenOptions,
        frames: Arc<dyn DecodedFrames>,
    ) -> Result<Arc<MediaSession>, CuaError> {
        cua_spaces_ffi::media_decode::spacesd_open_media_decoded(
            env,
            options,
            Arc::new(Decoded(frames)),
        )
        .await
    }
}

/// Packs the decoder's frames for [`DecodedFrames`].
struct Decoded(Arc<dyn DecodedFrames>);

impl cua_spaces_ffi::media_decode::DecodedFrameSink for Decoded {
    fn on_decoded_frame(&self, frame: cua_spaces_ffi::media_decode::DecodedVideoFrame) {
        let row = frame.width as usize * 4;
        let data = if frame.stride as usize == row {
            frame.data
        } else {
            frame
                .data
                .chunks(frame.stride as usize)
                .flat_map(|r| r[..row.min(r.len())].to_vec())
                .collect()
        };
        self.0.frame(frame.width, frame.height, data);
    }

    fn on_event(&self, event: MediaEvent) {
        self.0.event(event);
    }
}

/// Registers the Cua Spaces extensions (the daemon's and the commands).
pub fn register() {
    cua_spaces_ext::daemon::register();
    cua_cli::extension::register(Arc::new(CuaSpacesCli));
}

/// `cua` with the Cua Spaces extensions: registers them, then runs
/// [`cua_cli::main`].
pub fn main() {
    register();
    cua_cli::main()
}
