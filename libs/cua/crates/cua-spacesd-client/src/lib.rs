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
//! - Endpoints: `http(s)://host:port`, bare `host:port` (default 3211), a
//!   Fleet gateway service URL, or a relay URL (`https://relay/m/<id>`).
//! - Transports: native gRPC (HTTP/2, h2c or TLS) or gRPC-Web over
//!   HTTP/1.1 (Fleet gateway, relays, anything HTTP/1-only). `Auto` probes
//!   `GetCapabilities` over HTTP/2 and falls back to gRPC-Web.
//! - Auth: `authorization: Bearer <token>`; `x-cua-principal-bin`; through
//!   the Fleet gateway the Fleet bearer plus `X-Cua-Fleet-Claim`, with the
//!   env token moved to `x-cua-env-authorization`.
//! - Errors: `google.rpc.Status` + `cua.env.v1.ErrorInfo` → [`Error`].

mod client;
mod computer;
pub mod diagnose;
mod endpoint;
pub mod error;
pub mod expect;
mod fs;
mod http;
pub mod manifest;
mod process;
pub mod transport;
pub mod tunnel;

pub use client::{
    ConnectOptions, DEFAULT_CHUNK_BYTES, MAX_MESSAGE_BYTES, RetryPolicy, SpacesdClient,
};

pub use computer::{
    DESKTOP_COMPONENT, DesktopReadiness, KeySpec, MediaOptions, MediaSession, Screenshot,
    ScreenshotOptions,
};
pub use endpoint::{Endpoint, EndpointKind};
pub use error::{Error, ErrorDetails, Result};
pub use fs::{
    DownloadOptions, DownloadResult, DownloadSink, UploadOptions, UploadResult, UploadSource,
};
pub use http::{
    DEFAULT_HTTP_RESPONSE_LIMIT, DEFAULT_HTTP_TIMEOUT, HttpCall, HttpReply, MAX_HTTP_REQUEST_BYTES,
};
pub use process::{
    Command, DEFAULT_PROCESS_KEEPALIVE, ExitStatus, Output, ProcessEvent, ProcessHandle,
    ProcessRef, Replay,
};
pub use transport::{
    BearerProvider, ChannelError, ExtraHeaders, GatewayAuth, GrpcChannel, Keepalive, StaticBearer,
    Transport, TransportPreference,
};
pub use tunnel::{ForwardOptions, ForwardStats, TUNNEL_FORWARD_FEATURE, TcpForward};

/// Generated `cua.env.v1` types.
pub use cua_proto::env::v1 as pb;

#[cfg(feature = "testing")]
pub mod testing;
