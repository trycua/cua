//! Test fixtures (feature `test-fixtures`, never shipped).
//!
//! - [`start_env`]: a `cua_spacesd_client::testing::MockServer` behind a small TCP
//!   front that also serves a scripted rcdp wire v2 `/media` WebSocket on
//!   the same port, so env RPCs *and* media sessions work against one URL.
//! - [`start_fleet_http`]: the in-memory `FakeFleet` API served over real
//!   loopback HTTP, for bindings that configure Fleet by base URL.
//!
//! Every loop here is bounded (message counts and deadlines).

use cua_fleet::testing::FakeFleet;
/// The media wire's WebSocket subprotocols (RCDP wire v2, as cua-spacesd
/// speaks it; `media_wire_matches_the_driver` in the cua-spacesd e2e suite
/// pins them to the driver's).
pub mod v2 {
    /// The wire subprotocol.
    pub const WS_SUBPROTOCOL: &str = "rcdp.v2";
    /// A ticket offered as a subprotocol: `cua.ticket.<ticket>`.
    pub const WS_TICKET_SUBPROTOCOL_PREFIX: &str = "cua.ticket.";
    /// The legacy ticket subprotocol, still accepted.
    pub const LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX: &str = "rcdp.v2.ticket.";
}
use cua_spacesd_client::testing::{MockAuth, MockGateway, MockServer};
use futures_util::{SinkExt, StreamExt};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::{
    self,
    handshake::server::{ErrorResponse, Request, Response},
};

/// The ticket `MockServer::open_media` hands out.
pub const MOCK_TICKET: &str = "ticket-abc";

/// What the media WebSocket saw.
#[derive(Debug, Default, Clone)]
pub struct MediaLog {
    /// Successful attaches.
    pub attaches: u32,
    /// Rejected attaches (bad ticket).
    pub rejected: u32,
    /// Request path + query of the last attach.
    pub last_path: String,
    /// `authorization` header of the last attach.
    pub last_authorization: Option<String>,
    /// `x-cua-fleet-claim` header of the last attach.
    pub last_claim: Option<String>,
    /// Subprotocol the last attach was answered with.
    pub last_subprotocol: Option<String>,
    /// Text messages received from clients.
    pub received: Vec<String>,
}

/// A running env fixture.
pub struct SpacesdFixture {
    /// `http://127.0.0.1:<port>` (the front; env RPCs and `/media`).
    pub url: String,
    /// The mock's state.
    pub mock: MockServer,
    /// Media observations.
    pub media: Arc<Mutex<MediaLog>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SpacesdFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the env fixture. `token` is the spacesd token; `gateway`
/// emulates the Fleet gateway prefix and credentials.
pub async fn start_env(token: Option<&str>, gateway: Option<MockGateway>) -> SpacesdFixture {
    let mock = MockServer::start(MockAuth {
        token: token.map(str::to_string),
        gateway,
        prefix: None,
    })
    .await;
    let upstream = mock.addr;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind front");
    let addr = listener.local_addr().expect("front addr");
    let media = Arc::new(Mutex::new(MediaLog::default()));
    let log = media.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let _ = serve_conn(stream, upstream, log).await;
            });
        }
    });
    SpacesdFixture {
        url: format!("http://{addr}"),
        mock,
        media,
        task,
    }
}

async fn is_media_upgrade(stream: &TcpStream) -> bool {
    let mut buf = [0u8; 512];
    // Wait (bounded) for the request line.
    for _ in 0..50 {
        match stream.peek(&mut buf).await {
            Ok(n) if n >= 4 => {
                let head = &buf[..n];
                if !head.starts_with(b"GET ") {
                    return false;
                }
                if let Some(end) = head.windows(2).position(|w| w == b"\r\n") {
                    let line = String::from_utf8_lossy(&head[..end]);
                    let path = line.split(' ').nth(1).unwrap_or_default();
                    let path = path.split('?').next().unwrap_or_default();
                    return path.ends_with("/media");
                }
                if n == buf.len() {
                    return false;
                }
            }
            Ok(0) => return false,
            Ok(_) => {}
            Err(_) => return false,
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

async fn serve_conn(
    mut stream: TcpStream,
    upstream: SocketAddr,
    log: Arc<Mutex<MediaLog>>,
) -> std::io::Result<()> {
    if !is_media_upgrade(&stream).await {
        let mut up = TcpStream::connect(upstream).await?;
        let _ = tokio::io::copy_bidirectional(&mut stream, &mut up).await;
        return Ok(());
    }
    let seen = log.clone();
    #[allow(clippy::result_large_err)] // tungstenite's callback signature
    let callback = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let path = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        let ticket_ok = path.contains(&format!("ticket={MOCK_TICKET}"))
            || req
                .headers()
                .get("sec-websocket-protocol")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(',').map(str::trim).any(|p| {
                        [
                            v2::WS_TICKET_SUBPROTOCOL_PREFIX,
                            v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX,
                        ]
                        .iter()
                        .any(|prefix| p.strip_prefix(prefix) == Some(MOCK_TICKET))
                    })
                });
        let mut l = seen.lock().unwrap();
        if !ticket_ok {
            l.rejected += 1;
            let mut r = ErrorResponse::new(Some("bad ticket".into()));
            *r.status_mut() = http::StatusCode::UNAUTHORIZED;
            return Err(r);
        }
        l.attaches += 1;
        l.last_path = path;
        let h = |k: &str| {
            req.headers()
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        l.last_authorization = h("authorization");
        l.last_claim = h(cua_spacesd_client::transport::FLEET_CLAIM_HEADER);
        // Like the driver (cua-spacesd-desktop media_ws): echo `rcdp.v2` when
        // offered, else the ticket subprotocol the client authenticated
        // with (browsers require the server to pick an offered value).
        let offered: Vec<String> = req
            .headers()
            .get_all("sec-websocket-protocol")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|p| p.trim().to_string())
            .collect();
        let pick = if offered.iter().any(|p| p == v2::WS_SUBPROTOCOL) {
            Some(v2::WS_SUBPROTOCOL.to_string())
        } else {
            offered.into_iter().find(|p| {
                p.starts_with(v2::WS_TICKET_SUBPROTOCOL_PREFIX)
                    || p.starts_with(v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX)
            })
        };
        l.last_subprotocol = pick.clone();
        let mut resp = resp;
        if let Some(p) = pick {
            resp.headers_mut().insert(
                "sec-websocket-protocol",
                p.parse().expect("subprotocol is a header value"),
            );
        }
        Ok(resp)
    };
    let ws = match tokio_tungstenite::accept_hdr_async(stream, callback).await {
        Ok(ws) => ws,
        Err(_) => return Ok(()),
    };
    let (mut tx, mut rx) = ws.split();
    let script = [
        tungstenite::Message::Text(
            serde_json::json!({"type":"hello","payload":{"protocol":"rcdp","versions":[2],
                "selected_version":2,"capabilities":["desktop.v1","keyframe_on_attach.v1","audio.v1"]}})
            .to_string()
            .into(),
        ),
        tungstenite::Message::Text(
            serde_json::json!({"type":"session_opened","payload":{"session_id":"media-1",
                "target":{"kind":"display","display_id":"primary"}}})
            .to_string()
            .into(),
        ),
        tungstenite::Message::Binary(video_packet(7, true).into()),
        tungstenite::Message::Binary(audio_packet(3, 11).into()),
        tungstenite::Message::Binary(video_packet(8, false).into()),
    ];
    for m in script {
        if tx.send(m).await.is_err() {
            return Ok(());
        }
    }
    // Echo up to 100 client text messages, then close.
    for _ in 0..100 {
        match tokio::time::timeout(Duration::from_secs(30), rx.next()).await {
            Ok(Some(Ok(tungstenite::Message::Text(t)))) => {
                log.lock().unwrap().received.push(t.as_str().to_string());
                let echo = serde_json::json!({"type":"echo","payload": t.as_str()}).to_string();
                if tx
                    .send(tungstenite::Message::Text(echo.into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(Some(Ok(tungstenite::Message::Close(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) => break,
        }
    }
    let _ = tx.send(tungstenite::Message::Close(None)).await;
    Ok(())
}

/// A video packet framed exactly like the driver's (`VideoPacket::encode`
/// in cua-spacesd-session): a big-endian header length and payload length,
/// the `WireHeader::Video` descriptor as JSON, then an H.264-looking
/// payload. The cua-spacesd e2e suite pins it to the driver's encoder.
pub fn video_packet(sequence: u64, keyframe: bool) -> Vec<u8> {
    let payload: &[u8] = if keyframe {
        b"\x00\x00\x00\x01\x67\x42\x00\x00\x00\x01\x68\x00\x00\x00\x01\x65"
    } else {
        b"\x00\x00\x00\x01\x41"
    };
    let header = format!(
        r#"{{"direction":"video","message":{{"session_id":"media-1","sequence":{sequence},"geometry_epoch":1,"codec_epoch":1,"width_px":64,"height_px":48,"capture_timestamp_us":{},"codec":"h264","keyframe":{keyframe}}}}}"#,
        1_000 * sequence
    );
    let mut packet = Vec::with_capacity(8 + header.len() + payload.len());
    packet.extend_from_slice(&(header.len() as u32).to_be_bytes());
    packet.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    packet.extend_from_slice(header.as_bytes());
    packet.extend_from_slice(payload);
    packet
}

/// A `RAU2` audio packet (MEDIA.md §12.2) with a 3-byte payload.
pub fn audio_packet(track_id: u16, sequence: u32) -> Vec<u8> {
    let mut b = cua_proto::AUDIO_PACKET_MAGIC.to_vec();
    b.push(2);
    b.push(0);
    b.extend(track_id.to_be_bytes());
    b.extend(sequence.to_be_bytes());
    b.extend(20_000u64.to_be_bytes());
    b.extend(960u16.to_be_bytes());
    b.push(1);
    b.push(0);
    b.extend([0xf8, 0xff, 0xfe]);
    b
}

/// A running fake Fleet API on loopback HTTP.
pub struct FleetHttpFixture {
    /// `http://127.0.0.1:<port>`.
    pub base_url: String,
    /// The fake.
    pub fake: FakeFleet,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FleetHttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Token the fake accepts (any token is accepted; this is the conventional
/// one).
pub const FAKE_FLEET_TOKEN: &str = "fake-fleet-token";

/// Serves `fake` over loopback HTTP/1.1.
pub async fn start_fleet_http(fake: FakeFleet) -> FleetHttpFixture {
    start_fleet_http_with_gateway(fake, None).await
}

/// Same, and forwards the gateway's `/api/svc/...` requests to `gateway`
/// (for example a `MockServer` with a [`fake_gateway`]) instead of the
/// fake's canned reply, so a separate process (the CLI) can reach an
/// spacesd through "Fleet".
pub async fn start_fleet_http_with_gateway(
    fake: FakeFleet,
    gateway: Option<String>,
) -> FleetHttpFixture {
    serve_fleet_http(fake, gateway.map(Upstream::Gateway)).await
}

/// Same, and routes every claim's `env` service
/// (`/api/svc/<namespace>/<sandbox>-env/...`) to the cua-spacesd at `spacesd`
/// with the gateway prefix stripped, as the real gateway does. Any claim of
/// any pool then has a working spacesd, so a separate process (docs examples,
/// the CLI) can run `shell`, `files` or `screenshot` on a fake cloud sandbox.
/// Other services keep the fake's canned reply.
pub async fn start_fleet_http_with_spacesd(fake: FakeFleet, spacesd: String) -> FleetHttpFixture {
    serve_fleet_http(fake, Some(Upstream::Spacesd(spacesd))).await
}

/// Where the fixture forwards `/api/svc/...` requests.
#[derive(Clone)]
enum Upstream {
    /// Everything under `/api/svc/`, path unchanged (a gateway emulation).
    Gateway(String),
    /// Only `<sandbox>-env` services, prefix stripped (a plain spacesd).
    Spacesd(String),
}

impl Upstream {
    /// The upstream and the path (with query) to send, when `path_and_query`
    /// is forwarded.
    fn route(&self, path_and_query: &str) -> Option<(String, String)> {
        match self {
            Upstream::Gateway(gw) => path_and_query
                .starts_with("/api/svc/")
                .then(|| (gw.clone(), path_and_query.to_string())),
            Upstream::Spacesd(url) => {
                let rest = path_and_query.strip_prefix("/api/svc/")?;
                let (_namespace, rest) = rest.split_once('/')?;
                let (service, rest) = match rest.find(['/', '?']) {
                    Some(i) => rest.split_at(i),
                    None => (rest, ""),
                };
                if !service.ends_with("-env") {
                    return None;
                }
                let rest = if rest.starts_with('/') {
                    rest.to_string()
                } else {
                    format!("/{rest}")
                };
                Some((url.clone(), rest))
            }
        }
    }
}

async fn serve_fleet_http(fake: FakeFleet, upstream: Option<Upstream>) -> FleetHttpFixture {
    use cua_fleet::HttpClient;
    use http_body_util::BodyExt;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fleet");
    let addr = listener.local_addr().expect("fleet addr");
    let served = fake.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let fake = served.clone();
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let svc =
                    hyper::service::service_fn(move |req: http::Request<hyper::body::Incoming>| {
                        let fake = fake.clone();
                        let upstream = upstream.clone();
                        async move {
                            let route = upstream.as_ref().and_then(|u| {
                                u.route(
                                    req.uri()
                                        .path_and_query()
                                        .map(|p| p.as_str())
                                        .unwrap_or("/"),
                                )
                            });
                            if let Some((to, path)) = &route
                                && is_websocket(req.headers())
                            {
                                return Ok::<_, std::convert::Infallible>(
                                    proxy_upgrade(to, path, req).await,
                                );
                            }
                            let (parts, body) = req.into_parts();
                            let body = body
                                .collect()
                                .await
                                .map(|b| b.to_bytes())
                                .unwrap_or_default();
                            if let Some((to, path)) = route {
                                return Ok::<_, std::convert::Infallible>(
                                    proxy(&to, &path, parts, body).await,
                                );
                            }
                            let r = fake
                                .execute(cua_fleet::sdk::HttpRequest {
                                    method: parts.method.to_string(),
                                    url: format!("http://fleet.test{}", parts.uri),
                                    headers: parts
                                        .headers
                                        .iter()
                                        .map(|(k, v)| cua_fleet::sdk::HttpHeader {
                                            name: k.to_string(),
                                            value: v.to_str().unwrap_or_default().to_string(),
                                        })
                                        .collect(),
                                    body: (!body.is_empty()).then(|| body.to_vec()),
                                    timeout_secs: None,
                                    max_response_bytes: None,
                                })
                                .await
                                .expect("fake fleet never fails");
                            let mut resp = http::Response::builder().status(r.status);
                            for h in r.headers {
                                resp = resp.header(h.name, h.value);
                            }
                            Ok::<_, std::convert::Infallible>(
                                resp.body(http_body_util::Either::Left(http_body_util::Full::new(
                                    bytes::Bytes::from(r.body),
                                )))
                                .unwrap(),
                            )
                        }
                    });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
                    .with_upgrades()
                    .await;
            });
        }
    });
    FleetHttpFixture {
        base_url: format!("http://{addr}"),
        fake,
        task,
    }
}

type ProxyBody = http_body_util::Either<http_body_util::Full<bytes::Bytes>, hyper::body::Incoming>;

/// Forwards one buffered request to `upstream` and streams the reply back
/// unchanged (gRPC-Web frames keep their upstream chunking).
async fn proxy(
    upstream: &str,
    path_and_query: &str,
    parts: http::request::Parts,
    body: bytes::Bytes,
) -> http::Response<ProxyBody> {
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http::<http_body_util::Full<bytes::Bytes>>();
    let uri = format!("{}{}", upstream.trim_end_matches('/'), path_and_query);
    let mut req = http::Request::builder().method(parts.method).uri(uri);
    for (k, v) in parts.headers.iter() {
        if k != http::header::HOST
            && k != http::header::CONNECTION
            && k != http::header::TRANSFER_ENCODING
        {
            req = req.header(k, v);
        }
    }
    let bad = |m: String| {
        http::Response::builder()
            .status(502)
            .body(http_body_util::Either::Left(http_body_util::Full::new(
                bytes::Bytes::from(m),
            )))
            .unwrap()
    };
    let req = match req.body(http_body_util::Full::new(body)) {
        Ok(r) => r,
        Err(e) => return bad(e.to_string()),
    };
    match client.request(req).await {
        Ok(r) => r.map(http_body_util::Either::Right),
        Err(e) => bad(e.to_string()),
    }
}

fn is_websocket(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Relays a WebSocket upgrade (the spacesd's `/tunnel`, `/media`, ...) to
/// `upstream` like the Fleet gateway does: the handshake with every header,
/// then the raw upgraded bytes both ways.
async fn proxy_upgrade(
    upstream: &str,
    path_and_query: &str,
    mut req: http::Request<hyper::body::Incoming>,
) -> http::Response<ProxyBody> {
    let bad = |m: String| {
        http::Response::builder()
            .status(502)
            .body(http_body_util::Either::Left(http_body_util::Full::new(
                bytes::Bytes::from(m),
            )))
            .unwrap()
    };
    let authority = upstream
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let stream = match TcpStream::connect(&authority).await {
        Ok(s) => s,
        Err(e) => return bad(e.to_string()),
    };
    let (mut sender, conn) =
        match hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await {
            Ok(v) => v,
            Err(e) => return bad(e.to_string()),
        };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let downstream = hyper::upgrade::on(&mut req);
    let mut up = http::Request::builder()
        .method(req.method())
        .uri(path_and_query);
    for (k, v) in req.headers() {
        if k != http::header::HOST {
            up = up.header(k, v);
        }
    }
    up = up.header(http::header::HOST, authority.as_str());
    let up = match up.body(http_body_util::Empty::<bytes::Bytes>::new()) {
        Ok(r) => r,
        Err(e) => return bad(e.to_string()),
    };
    let mut resp = match sender.send_request(up).await {
        Ok(r) => r,
        Err(e) => return bad(e.to_string()),
    };
    if resp.status() == http::StatusCode::SWITCHING_PROTOCOLS {
        let upstream_io = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            if let (Ok(a), Ok(b)) = tokio::join!(downstream, upstream_io) {
                let mut a = hyper_util::rt::TokioIo::new(a);
                let mut b = hyper_util::rt::TokioIo::new(b);
                let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
            }
        });
    }
    resp.map(http_body_util::Either::Right)
}

/// A running read-only OCI registry on loopback HTTP serving a
/// [`cua_image::testing::FakeRegistry`]. Point `CUA_REGISTRY_MIRRORS` at
/// [`RegistryFixture::mirror`] and a separate process resolves images
/// (manifests, configs) from it instead of the network.
pub struct RegistryFixture {
    /// `127.0.0.1:<port>`.
    pub host: String,
    task: tokio::task::JoinHandle<()>,
}

impl RegistryFixture {
    /// A `CUA_REGISTRY_MIRRORS` value that sends every registry here.
    pub fn mirror(&self) -> String {
        format!("*=http://{}", self.host)
    }
}

impl Drop for RegistryFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The images the docs examples and e2e fixtures resolve, in the canonical
/// layouts: `python:3.12-slim` (a multi-arch rootfs) and
/// `ghcr.io/trycua/linux:24.04` (rootfs with cua-spacesd, plus its `-disk`
/// containerDisk sibling named by the variants annotation).
pub fn sample_registry() -> cua_image::testing::FakeRegistry {
    use cua_image::resolve::{SPACESD_LABEL, VARIANTS_ANNOTATION};
    let mut r = cua_image::testing::FakeRegistry::default();
    let arches = ["amd64", "arm64"];
    r.index("docker.io/library/python:3.12-slim", &arches, false, None);
    let disk = r.index("ghcr.io/trycua/linux:24.04-disk", &arches, true, None);
    let variants = serde_json::json!({"containerdisk": format!("ghcr.io/trycua/linux@{disk}")});
    r.index(
        "ghcr.io/trycua/linux:24.04",
        &arches,
        false,
        Some(serde_json::json!({
            VARIANTS_ANNOTATION: variants.to_string(),
            SPACESD_LABEL: "true",
        })),
    );
    r
}

/// Serves `registry` (the `/v2/` API: manifests by tag or digest, blobs)
/// over loopback HTTP/1.1. A mirrored pull names its registry in `?ns=`
/// (default `docker.io`).
pub async fn start_registry_http(registry: cua_image::testing::FakeRegistry) -> RegistryFixture {
    let registry = Arc::new(registry);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind registry");
    let host = listener.local_addr().expect("registry addr").to_string();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let registry = registry.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(
                    move |req: http::Request<hyper::body::Incoming>| {
                        let registry = registry.clone();
                        async move { Ok::<_, std::convert::Infallible>(registry_reply(&registry, &req)) }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    RegistryFixture { host, task }
}

fn registry_reply<B>(
    registry: &cua_image::testing::FakeRegistry,
    req: &http::Request<B>,
) -> http::Response<http_body_util::Full<bytes::Bytes>> {
    let reply = |status: u16, headers: Vec<(&str, String)>, body: Vec<u8>| {
        let mut r = http::Response::builder().status(status);
        for (k, v) in headers {
            r = r.header(k, v);
        }
        let body = if req.method() == http::Method::HEAD {
            Vec::new()
        } else {
            body
        };
        r.body(http_body_util::Full::new(bytes::Bytes::from(body)))
            .unwrap()
    };
    let not_found = || {
        reply(
            404,
            vec![("content-type", "application/json".into())],
            br#"{"errors":[{"code":"MANIFEST_UNKNOWN","message":"not found"}]}"#.to_vec(),
        )
    };
    let path = req.uri().path();
    let ns = req
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("ns=")))
        .unwrap_or("docker.io");
    if path == "/v2/" || path == "/v2" {
        return reply(200, vec![], b"{}".to_vec());
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return not_found();
    };
    if let Some((repo, reference)) = rest.rsplit_once("/manifests/") {
        let full = if reference.starts_with("sha256:") {
            format!("{ns}/{repo}@{reference}")
        } else {
            format!("{ns}/{repo}:{reference}")
        };
        let Some(raw) = registry.manifests.get(&full) else {
            return not_found();
        };
        let media_type = serde_json::from_slice::<serde_json::Value>(raw)
            .ok()
            .and_then(|v| v["mediaType"].as_str().map(str::to_string))
            .unwrap_or_else(|| "application/vnd.oci.image.manifest.v1+json".into());
        return reply(
            200,
            vec![
                ("content-type", media_type),
                (
                    "docker-content-digest",
                    cua_image::digest::sha256_bytes(raw),
                ),
                ("content-length", raw.len().to_string()),
            ],
            raw.clone(),
        );
    }
    if let Some((_, digest)) = rest.rsplit_once("/blobs/")
        && let Some(raw) = registry.blobs.get(digest)
    {
        return reply(
            200,
            vec![
                ("content-type", "application/octet-stream".into()),
                ("docker-content-digest", digest.to_string()),
                ("content-length", raw.len().to_string()),
            ],
            raw.clone(),
        );
    }
    not_found()
}

/// The gateway emulation matching `FakeFleet`'s binding of claim `claim`
/// in pool `pool` (sandbox `sbx-<claim>`).
pub fn fake_gateway(pool: &str, claim: &str) -> MockGateway {
    MockGateway {
        prefix: format!("/api/svc/{pool}/sbx-{claim}-env"),
        bearer: FAKE_FLEET_TOKEN.into(),
        claim: claim.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    async fn attach(url: &str, protocols: &str) -> (Option<String>, Vec<u8>) {
        let ws_url = format!("{}/media", url.replacen("http://", "ws://", 1));
        let mut req = ws_url.into_client_request().unwrap();
        req.headers_mut()
            .insert("sec-websocket-protocol", protocols.parse().unwrap());
        let (mut ws, resp) = tokio_tungstenite::connect_async(req).await.unwrap();
        let picked = resp
            .headers()
            .get("sec-websocket-protocol")
            .map(|v| v.to_str().unwrap().to_string());
        for _ in 0..10 {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(tungstenite::Message::Binary(b)))) => return (picked, b.to_vec()),
                Ok(Some(Ok(_))) => continue,
                other => panic!("no video packet: {other:?}"),
            }
        }
        panic!("no video packet in 10 messages");
    }

    /// Parity with the driver's media socket: the offered wire subprotocol
    /// (else the ticket one) is echoed, and video goes out as
    /// [`video_packet`] frames them.
    #[tokio::test]
    async fn media_socket_matches_the_driver() {
        let fx = start_env(Some("tok"), None).await;
        let ticket = format!("{}{MOCK_TICKET}", v2::WS_TICKET_SUBPROTOCOL_PREFIX);
        let (picked, packet) = attach(&fx.url, &format!("{}, {ticket}", v2::WS_SUBPROTOCOL)).await;
        assert_eq!(picked.as_deref(), Some(v2::WS_SUBPROTOCOL));
        assert_eq!(packet, video_packet(7, true));
        let (picked, _) = attach(&fx.url, &ticket).await;
        assert_eq!(picked.as_deref(), Some(ticket.as_str()));
        assert_eq!(
            fx.media.lock().unwrap().last_subprotocol.as_deref(),
            Some(ticket.as_str())
        );
        // The legacy `rcdp.v2.ticket.` form still attaches.
        let legacy = format!("{}{MOCK_TICKET}", v2::LEGACY_WS_TICKET_SUBPROTOCOL_PREFIX);
        let (picked, _) = attach(&fx.url, &format!("{}, {legacy}", v2::WS_SUBPROTOCOL)).await;
        assert_eq!(picked.as_deref(), Some(v2::WS_SUBPROTOCOL));
        let (picked, _) = attach(&fx.url, &legacy).await;
        assert_eq!(picked.as_deref(), Some(legacy.as_str()));
    }
}
