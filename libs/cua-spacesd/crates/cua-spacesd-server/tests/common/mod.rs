// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared harness for the conformance suite.
//!
//! By default every test starts an in-process server on an ephemeral
//! loopback port with a temp data dir, a temp Downloads dir, a fake teleport
//! host and a fake tool registry, so nothing touches the real machine beyond
//! child processes (`sh`, `sleep`, `stty`, `python3 -m http.server` bound to
//! loopback) in temp directories.
//!
//! Set `CUA_ENV_TEST_TARGET=http://host:port[/prefix]` and
//! `CUA_ENV_TEST_TOKEN=…` to run the same suite against any running driver
//! (a container, or through `cua-relay` with `/m/<machine-id>`). Tests that
//! need in-process fakes skip themselves in that mode.
//!
//! Knobs: `CUA_ENV_TEST_BIG_BYTES` (transfer size, default 64 MiB; 1 GiB in
//! the docker lane), `CUA_ENV_TEST_LONG_SECS` (long-running stream, default
//! 60), `CUA_ENV_TEST_GUEST_TMP` (guest scratch root in remote mode, default
//! `/tmp`).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use serde_json::{json, Value};

pub const IN_PROCESS_TOKEN: &str = "conformance-token";

/// The POSIX shell the suite's scripts run in: `/bin/sh` on Unix. On Windows
/// it is the `sh` on PATH (Git for Windows, which the CI runners ship), so
/// the same scripts exercise the Windows process API (pipes, ConPTY, exit
/// codes, kill).
pub const SH: &str = if cfg!(windows) { "sh" } else { "/bin/sh" };

/// The Python the tunnel tests serve HTTP with.
pub const PYTHON: &str = if cfg!(windows) { "python" } else { "python3" };

/// Both wire protocols.
pub const TRANSPORTS: [TransportPreference; 2] =
    [TransportPreference::Native, TransportPreference::GrpcWeb];

/// A driver under test.
pub struct Target {
    /// Base URL.
    pub url: String,
    /// Access token.
    pub token: String,
    /// Scratch directory inside the guest (created per test).
    pub scratch: String,
    /// In-process server state, if any.
    pub local: Option<Local>,
}

/// In-process extras.
pub struct Local {
    pub ctx: ServerContext,
    pub manifest: cua_spacesd_server::RouteManifest,
    pub downloads: PathBuf,
    pub host: Arc<cua_spacesd_teleport::FakeHost>,
    pub teleport_home: PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// A tool provider with a fake `get_screen_size` (a tool cua-driver's
/// authorization has reviewed as read-only) and an unreviewed `echo` that
/// cua-driver's authorization must refuse. No platform effects.
pub struct FakeTools;

#[async_trait]
impl cua_driver_core::server::ToolProvider for FakeTools {
    fn tools_list(&self) -> Value {
        json!({
            "tools": [{
                "name": "get_screen_size",
                "description": "Fake screen size.",
                "inputSchema": {"type": "object", "properties": {}},
                "annotations": {"readOnlyHint": true, "destructiveHint": false},
            }, {
                "name": "echo",
                "description": "Echoes its `text` argument.",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}},
                "annotations": {"readOnlyHint": true, "destructiveHint": false},
            }],
            "capability_version": "1",
        })
    }

    async fn invoke_tool(&self, name: &str, arguments: Value) -> Result<Value, String> {
        match name {
            "get_screen_size" => Ok(json!({
                "content": [{"type": "text", "text": "1280x800"}],
                "structuredContent": {"width": 1280, "height": 800, "scale_factor": 1.0},
            })),
            "echo" => Ok(json!({
                "content": [{"type": "text", "text": arguments["text"].as_str().unwrap_or("")}],
                "structuredContent": {"echoed": arguments["text"]},
            })),
            other => Err(format!("unknown tool {other}")),
        }
    }
}

pub fn remote() -> Option<(String, String)> {
    let url = std::env::var("CUA_ENV_TEST_TARGET")
        .ok()
        .filter(|s| !s.is_empty())?;
    let token = std::env::var("CUA_ENV_TEST_TOKEN").unwrap_or_default();
    Some((url, token))
}

/// `tokio_tungstenite::connect_async` (with TLS for `wss://`).
// Mirrors `connect_async`'s own signature.
#[allow(clippy::result_large_err)]
pub async fn ws_connect(
    url: impl AsRef<str>,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::handshake::client::Response,
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let request = url.as_ref().into_client_request()?;
    tokio_tungstenite::connect_async(request).await
}

pub fn big_bytes() -> u64 {
    std::env::var("CUA_ENV_TEST_BIG_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64 * 1024 * 1024)
}

pub fn long_secs() -> u64 {
    std::env::var("CUA_ENV_TEST_LONG_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
}

/// Starts (or points at) a driver.
pub async fn target() -> Target {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_test_writer()
        .try_init();
    if let Some((url, token)) = remote() {
        let root = std::env::var("CUA_ENV_TEST_GUEST_TMP").unwrap_or_else(|_| "/tmp".into());
        let scratch = format!("{root}/cua-conformance-{}", uuid::Uuid::new_v4().simple());
        let t = Target {
            url,
            token,
            scratch,
            local: None,
        };
        // The target may still be (re)joining a relay: retry for up to 60 s.
        let mut last = None;
        for _ in 0..120 {
            let c = t.client(TransportPreference::Native).await;
            match c
                .filesystem()
                .make_dir(cua_proto::env::v1::MakeDirRequest {
                    path: t.scratch.clone(),
                    parents: true,
                    mode: 0,
                    ..Default::default()
                })
                .await
            {
                Ok(_) => {
                    last = None;
                    break;
                }
                Err(e) => last = Some(e),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        if let Some(error) = last {
            panic!("create remote scratch dir: {error}");
        }
        return t;
    }
    let data = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        data_dir: data.path().to_path_buf(),
        downloads_dir: Some(downloads.path().to_path_buf()),
        teleport_home: Some(home.path().to_path_buf()),
        teleport_ledger_dir: Some(data.path().join("teleport-ledger")),
        shutdown_grace: Duration::from_secs(1),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, Some(IN_PROCESS_TOKEN.into()));
    // Everything fails by default (nothing running, no Keychain), except
    // launching: on macOS the app is opened through `open`, which succeeds.
    let host = Arc::new(cua_spacesd_teleport::FakeHost::new().with_responder(|c| {
        Ok(if c.kind == cua_spacesd_teleport::EffectKind::AppLaunch {
            cua_spacesd_teleport::host::HostOutput::ok("")
        } else {
            cua_spacesd_teleport::host::HostOutput::failed()
        })
    }));
    let receiver = Arc::new(cua_spacesd_teleport::Receiver::with_host(
        home.path().to_path_buf(),
        host.clone(),
    ));
    let server = ServerBuilder::new(ctx.clone())
        .tools(Arc::new(FakeTools))
        .teleport_receiver(receiver)
        .build();
    let manifest = server.manifest().clone();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    Target {
        url: format!("http://{addr}"),
        token: IN_PROCESS_TOKEN.into(),
        scratch: scratch_path(scratch.path()),
        local: Some(Local {
            ctx,
            manifest,
            downloads: downloads.path().to_path_buf(),
            host,
            teleport_home: home.path().to_path_buf(),
            _dirs: vec![data, downloads, home, scratch],
        }),
    }
}

/// The scratch dir as the tests write it into paths and shell scripts. On
/// Windows `canonicalize` returns a `\\?\` verbatim path, in which the `/`
/// the tests append is not a separator and which Git's `sh` cannot open;
/// use the plain drive path with `/` separators, which both accept.
fn scratch_path(dir: &std::path::Path) -> String {
    let canonical = dir.canonicalize().unwrap().display().to_string();
    if cfg!(windows) {
        canonical
            .strip_prefix(r"\\?\")
            .unwrap_or(&canonical)
            .replace('\\', "/")
    } else {
        canonical
    }
}

impl Target {
    /// Connects with a transport.
    pub async fn client(&self, transport: TransportPreference) -> SpacesdClient {
        self.client_with_token(transport, Some(self.token.clone()))
            .await
    }

    /// Connects with an explicit token (or none), without the capability
    /// probe so auth failures surface on the first call.
    pub async fn client_with_token(
        &self,
        transport: TransportPreference,
        token: Option<String>,
    ) -> SpacesdClient {
        let mut options = ConnectOptions::parse(&self.url)
            .unwrap()
            .transport(transport)
            .probe(false);
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            options = options.token(token);
        }
        SpacesdClient::connect(options).await.expect("connect")
    }

    pub fn endpoint(&self) -> cua_spacesd_client::Endpoint {
        cua_spacesd_client::Endpoint::parse(&self.url).unwrap()
    }

    /// Absolute path of `name` in the scratch dir.
    pub fn path(&self, name: &str) -> String {
        format!("{}/{name}", self.scratch)
    }

    /// Deletes the scratch dir.
    pub async fn cleanup(&self) {
        if self.local.is_none() {
            let c = self.client(TransportPreference::Native).await;
            let _ = c
                .filesystem()
                .remove(cua_proto::env::v1::RemoveRequest {
                    path: self.scratch.clone(),
                    recursive: true,
                    missing_ok: true,
                    ..Default::default()
                })
                .await;
        }
    }
}

/// Deterministic pseudo-random bytes (xorshift64*), streamable.
pub struct Pattern {
    state: u64,
}

impl Pattern {
    pub fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            self.state ^= self.state >> 12;
            self.state ^= self.state << 25;
            self.state ^= self.state >> 27;
            let v = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

/// SHA-256 of `len` bytes of `Pattern::new(seed)` generated in `chunk`-byte
/// pieces (piece boundaries do not matter as long as they are multiples of
/// 8).
pub fn pattern_sha(seed: u64, len: u64) -> String {
    use sha2::Digest;
    let mut p = Pattern::new(seed);
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut left = len;
    while left > 0 {
        let n = left.min(buf.len() as u64) as usize;
        p.fill(&mut buf[..n]);
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    hex::encode(hasher.finalize())
}

type HttpBody = http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error>;

/// Minimal HTTP/1.1 client for `/files`, `/health` and `/mcp` (http or
/// https).
pub struct TestHttp(
    hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        HttpBody,
    >,
);

impl TestHttp {
    pub async fn request(
        &self,
        request: http::Request<HttpBody>,
    ) -> Result<http::Response<hyper::body::Incoming>, hyper_util::client::legacy::Error> {
        self.0.request(request).await
    }

    pub async fn get(
        &self,
        uri: http::Uri,
    ) -> Result<http::Response<hyper::body::Incoming>, hyper_util::client::legacy::Error> {
        let mut request = http::Request::new(full(bytes::Bytes::new()));
        *request.uri_mut() = uri;
        self.request(request).await
    }
}

pub fn http_client() -> TestHttp {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    TestHttp(
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .build(https),
    )
}

pub fn full(
    bytes: impl Into<bytes::Bytes>,
) -> http_body_util::combinators::BoxBody<bytes::Bytes, std::io::Error> {
    use http_body_util::BodyExt;
    http_body_util::Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed()
}

/// Reads a response body fully (bounded to `limit` bytes).
pub async fn body_bytes(response: http::Response<hyper::body::Incoming>, limit: usize) -> Vec<u8> {
    use http_body_util::BodyExt;
    let mut body = response.into_body();
    let mut out = Vec::new();
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.expect("body frame").into_data() {
            out.extend_from_slice(&data);
            assert!(out.len() <= limit, "response body exceeds {limit} bytes");
        }
    }
    out
}

/// Runs `sh -c script` to completion and returns (exit code, stdout).
pub async fn run_sh(client: &SpacesdClient, script: &str) -> (Option<i32>, Vec<u8>) {
    use cua_proto::env::v1::process_data::Output;
    use cua_proto::env::v1::process_event::Event;
    let mut stream = client
        .process()
        .start_process(cua_proto::env::v1::StartProcessRequest {
            config: Some(cua_proto::env::v1::ProcessConfig {
                command: SH.into(),
                args: vec!["-c".into(), script.into()],
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("start")
        .into_inner();
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        let Some(message) = tokio::time::timeout(Duration::from_secs(120), stream.message())
            .await
            .expect("process stream stalled")
            .expect("stream error")
        else {
            break;
        };
        match message.event.and_then(|e| e.event) {
            Some(Event::Data(d)) => {
                if let Some(Output::Stdout(b)) = d.output {
                    out.extend_from_slice(&b);
                    assert!(out.len() < 256 * 1024 * 1024, "unbounded output");
                }
            }
            Some(Event::End(end)) => {
                assert!(end.error.is_empty(), "process error: {}", end.error);
                return (end.exit_code, out);
            }
            _ => {}
        }
    }
    panic!("process stream ended without ProcessEnd");
}

/// Seeds the pinned `uv` a host's job members get (`<tools>/job-tools/<this
/// machine's target>/uv`), so a test needs no download: a stand-in that
/// prints its version.
pub fn seed_uv(dirs: &cua_cluster::jobdir::JobDirs) {
    let target = cua_host::uv::uv_target().expect("a pinned uv target");
    // The executor's own tools directory: under the cua home, or the
    // shared one when it runs each run as its own user (root on Linux).
    let dir = dirs.tools_dir(target);
    std::fs::create_dir_all(&dir).unwrap();
    let uv = dir.join("uv");
    std::fs::write(&uv, "#!/bin/sh\necho uv stand-in\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(
        dir.join(".uv-version"),
        format!("{} {target}\n", cua_host::uv::UV_VERSION),
    )
    .unwrap();
}
