//! Ergonomic async client for cua-spacesd (`cua.env.v1`).
//!
//! ```no_run
//! # async fn demo() -> cua_spacesd_client::Result<()> {
//! let env = cua_spacesd_client::SpacesdClient::connect_url("127.0.0.1:3211", Some("token".into())).await?;
//! let out = env.run("uname -a").await?;
//! println!("{}", out.stdout_str());
//! env.upload("/tmp/a.bin", vec![0u8; 1024], Default::default()).await?;
//! let png = env.screenshot(Default::default()).await?;
//! # let _ = png; Ok(()) }
//! ```
//!
//! - Endpoints: `http(s)://host:port`, bare `host:port` (default 3211), or a
//!   relay URL (`https://relay/m/<id>`).
//! - Transports: native gRPC (HTTP/2, h2c or TLS) or gRPC-Web over
//!   HTTP/1.1 (relays, anything HTTP/1-only). `Auto` probes
//!   `GetCapabilities` over HTTP/2 and falls back to gRPC-Web.
//! - Proxies: RPC and raw HTTP connections honor `HTTP_PROXY` / `HTTPS_PROXY`,
//!   then `ALL_PROXY`, with `NO_PROXY` exclusions (including hosts and CIDRs).
//!   Uppercase values take precedence over lowercase, even when empty; an empty
//!   scheme-specific value permits `ALL_PROXY`. Unauthenticated HTTP proxies
//!   use CONNECT for both HTTP and HTTPS origins. Origin TLS verification stays
//!   enabled. Proxy credentials, HTTPS/SOCKS proxies and CGI proxy use are refused;
//!   a selected proxy failure never falls back to a direct connection. This does
//!   not change the separate WebSocket media and tunnel transports.
//! - Auth: `authorization: Bearer <token>`; `x-cua-principal-bin`; through
//!   a gateway (the relay) the gateway bearer, with the env token moved to
//!   `x-cua-env-authorization`.
//! - Errors: `google.rpc.Status` + `cua.env.v1.ErrorInfo` → [`Error`].

#[cfg(unix)]
mod bulk;
mod client;
mod computer;
mod connector;
pub mod diagnose;
mod egress;
mod endpoint;
pub mod error;
pub mod expect;
mod fs;
mod http;
pub mod manifest;
pub mod old_host;
mod process;
mod record;
mod tls;
pub mod transport;
pub mod tunnel;

pub use client::{
    ConnectOptions, DEFAULT_CHUNK_BYTES, MAX_MESSAGE_BYTES, RetryPolicy, SpacesdClient,
};

pub use computer::{
    DESKTOP_COMPONENT, DesktopReadiness, KeySpec, MediaOptions, MediaSession, Screenshot,
    ScreenshotOptions,
};
pub use egress::EgressPolicy;
pub use endpoint::{Endpoint, EndpointKind};
pub use error::{Error, ErrorDetails, Result};
pub use fs::{
    DownloadOptions, DownloadResult, DownloadSink, FileAccess, RUN_AS_FEATURE, UploadOptions,
    UploadResult, UploadSource, XATTR_FEATURE,
};
pub use http::{
    DEFAULT_HTTP_RESPONSE_LIMIT, DEFAULT_HTTP_TIMEOUT, HttpCall, HttpReply, MAX_HTTP_REQUEST_BYTES,
};
pub use process::{
    Command, DEFAULT_PROCESS_KEEPALIVE, ExitStatus, Output, PROCESS_RUN_AS_FEATURE, ProcessEvent,
    ProcessHandle, ProcessRef, Replay, RunAsMode,
};
pub use record::{RecordOptions, Recorded, ScreenRecording, find_ffmpeg};
pub use transport::{
    BearerProvider, ChannelError, ExtraHeaders, GatewayAuth, GrpcChannel, Keepalive, StaticBearer,
    Transport, TransportPreference, set_process_principal,
};
pub use tunnel::{ForwardOptions, ForwardStats, TUNNEL_FORWARD_FEATURE, TcpForward};

/// Generated `cua.env.v1` types.
pub use cua_proto::env::v1 as pb;

#[cfg(feature = "testing")]
pub mod testing;
