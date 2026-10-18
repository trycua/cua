// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `SystemService.Diagnose` / `DiagnoseOnce`: the image self-test over RPC.
//!
//! The checks themselves live in `cua-spacesd-doctor` (linked by the
//! `cua-spacesd` binary through [`crate::ServerBuilder::diagnoser`]). They
//! act as a client of this very server over loopback, so a Diagnose call
//! exercises the real service stack, not an in-process shortcut. The server
//! hands the diagnoser the loopback address it is bound to and its current
//! root token; neither leaves the process.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use cua_proto::env::v1::{DiagnoseOptions, DiagnoseReport, DiagnoseResponse, ErrorReason};
use tokio::sync::mpsc;
use tonic::Code;

use crate::context::ServerContext;
use crate::error::status;

/// Where the diagnoser reaches this server.
#[derive(Clone)]
pub struct DiagnoseTarget {
    /// `http://127.0.0.1:<port>` (or `[::1]`).
    pub url: String,
    /// Current root token; `None` on an open loopback bind.
    pub token: Option<String>,
}

impl std::fmt::Debug for DiagnoseTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnoseTarget")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Runs the doctor checks. Implemented by `cua-spacesd-doctor`.
#[tonic::async_trait]
pub trait Diagnoser: Send + Sync + 'static {
    /// Runs every selected check against `target`, sending `started` and
    /// `check` events to `events` as they happen (a closed receiver must not
    /// stop the run), and returns the final report.
    async fn diagnose(
        &self,
        target: DiagnoseTarget,
        options: DiagnoseOptions,
        events: mpsc::Sender<DiagnoseResponse>,
    ) -> DiagnoseReport;
}

/// Error when the binary was built without the doctor.
pub const NO_DIAGNOSER: &str =
    "this cua-spacesd build has no doctor linked; run `cua-spacesd doctor` from a full build";

/// Shared state of the Diagnose RPCs: the hook and a one-run-at-a-time gate
/// (effectful checks drive fixture windows; two runs would fight over them).
#[derive(Clone)]
pub struct DiagnoseState {
    ctx: ServerContext,
    diagnoser: Option<Arc<dyn Diagnoser>>,
    running: Arc<tokio::sync::Mutex<()>>,
}

impl DiagnoseState {
    /// New state.
    pub fn new(ctx: ServerContext, diagnoser: Option<Arc<dyn Diagnoser>>) -> Self {
        Self {
            ctx,
            diagnoser,
            running: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Where this server listens, as a loopback URL.
    pub fn target(&self) -> Result<DiagnoseTarget, tonic::Status> {
        let bound = self.ctx.local_addr().ok_or_else(|| {
            status(
                Code::Unavailable,
                ErrorReason::TargetUnavailable,
                "the server is not bound yet",
            )
        })?;
        Ok(DiagnoseTarget {
            url: format!("http://{}", loopback_for(bound)),
            token: self.ctx.auth().token().map(|t| t.to_string()),
        })
    }

    /// Starts a run: returns the event receiver. The final report is sent
    /// as the last event.
    pub fn start(
        &self,
        options: DiagnoseOptions,
    ) -> Result<mpsc::Receiver<DiagnoseResponse>, tonic::Status> {
        let diagnoser = self.diagnoser.clone().ok_or_else(|| {
            status(
                Code::FailedPrecondition,
                ErrorReason::FeatureUnsupported,
                NO_DIAGNOSER,
            )
        })?;
        let guard = self.running.clone().try_lock_owned().map_err(|_| {
            status(
                Code::ResourceExhausted,
                ErrorReason::LimitExceeded,
                "a Diagnose run is already in progress; retry when it finishes",
            )
        })?;
        let target = self.target()?;
        // Bounded: a slow reader applies backpressure; events past a closed
        // receiver are dropped by the diagnoser, which keeps running so the
        // fixtures it started are always torn down.
        let (sender, receiver) = mpsc::channel(64);
        tokio::spawn(async move {
            let report = diagnoser.diagnose(target, options, sender.clone()).await;
            let _ = sender
                .send(DiagnoseResponse {
                    event: Some(cua_proto::env::v1::diagnose_response::Event::Report(report)),
                })
                .await;
            drop(guard);
        });
        Ok(receiver)
    }

    /// Runs to completion and returns only the report.
    pub async fn run_once(
        &self,
        options: DiagnoseOptions,
    ) -> Result<DiagnoseReport, tonic::Status> {
        let mut receiver = self.start(options)?;
        while let Some(event) = receiver.recv().await {
            if let Some(cua_proto::env::v1::diagnose_response::Event::Report(report)) = event.event
            {
                return Ok(report);
            }
        }
        Err(status(
            Code::Internal,
            ErrorReason::Internal,
            "the doctor finished without a report",
        ))
    }
}

/// The address a local client uses to reach a listener bound to `bound`
/// (an unspecified bind is reachable on loopback).
pub fn loopback_for(bound: SocketAddr) -> SocketAddr {
    let ip = match bound.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, bound.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unspecified_binds_map_to_loopback() {
        assert_eq!(
            loopback_for("0.0.0.0:3211".parse().unwrap()).to_string(),
            "127.0.0.1:3211"
        );
        assert_eq!(
            loopback_for("[::]:3211".parse().unwrap()).to_string(),
            "[::1]:3211"
        );
        assert_eq!(
            loopback_for("10.0.0.5:3211".parse().unwrap()).to_string(),
            "10.0.0.5:3211"
        );
    }

    #[test]
    fn target_debug_redacts_the_token() {
        let target = DiagnoseTarget {
            url: "http://127.0.0.1:1".into(),
            token: Some("secret-value".into()),
        };
        let text = format!("{target:?}");
        assert!(!text.contains("secret-value"), "{text}");
        assert!(text.contains("<redacted>"));
    }
}
