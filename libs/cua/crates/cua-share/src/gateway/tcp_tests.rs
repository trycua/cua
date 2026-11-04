//! Public TCP shares through the gateway: the relay's WebSocket spliced to
//! the one Space port the share names, over a real Unix socket and real TCP
//! upstreams. (The per-share connection and bandwidth caps are the relay's:
//! see cua-relay `tcp_shares` and its `share_links` tests.)

use super::*;
use crate::model::{Access, Owner, TcpEndpoint, random_hex};
use futures_util::{SinkExt as _, StreamExt as _};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, UnixStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const SPACE: &str = "local:mc";
const OTHER_SPACE: &str = "local:other";
const PORT: u16 = 25565;

#[derive(Default)]
struct Log(Mutex<Vec<String>>);
impl AuditSink for Log {
    fn record(&self, principal: &str, action: &str, handle: &str, detail: &str) {
        self.0
            .lock()
            .unwrap()
            .push(format!("{principal}|{action}|{handle}|{detail}"));
    }
}

/// Space ports as the daemon's forwards would answer them, and every lookup
/// the gateway made.
#[derive(Default)]
struct Spaces {
    bases: Mutex<HashMap<(String, u16), String>>,
    calls: Mutex<Vec<(String, u16)>>,
}

impl Spaces {
    fn set(&self, space: &str, port: u16, base: &str) {
        self.bases
            .lock()
            .unwrap()
            .insert((space.to_owned(), port), base.to_owned());
    }
    fn calls(&self) -> Vec<(String, u16)> {
        self.calls.lock().unwrap().clone()
    }
}

impl UpstreamResolver for Spaces {
    fn resolve_space<'a>(
        &'a self,
        space: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + 'a>> {
        self.calls.lock().unwrap().push((space.to_owned(), port));
        let found = self
            .bases
            .lock()
            .unwrap()
            .get(&(space.to_owned(), port))
            .cloned();
        Box::pin(async move { found.ok_or_else(|| "no such Space".to_string()) })
    }
}

struct Rig {
    sock: PathBuf,
    store: Arc<ShareStore>,
    spaces: Arc<Spaces>,
    _log: Arc<Log>,
    _h: GatewayHandle,
    _d: tempfile::TempDir,
}

async fn rig() -> Rig {
    let d = tempfile::tempdir().unwrap();
    let store = Arc::new(ShareStore::open(d.path().join("share/shares.json")).unwrap());
    let log = Arc::new(Log::default());
    let spaces = Arc::new(Spaces::default());
    let gw = Arc::new(Gateway::new(
        store.clone(),
        spaces.clone(),
        log.clone(),
        GatewayConfig::default(),
    ));
    let sock = d.path().join("share/gateway.sock");
    let h = gw.serve_unix(&sock).await.unwrap();
    Rig {
        sock,
        store,
        spaces,
        _log: log,
        _h: h,
        _d: d,
    }
}

/// A public TCP share of `target`.
fn tcp_share(target: Target) -> Share {
    Share {
        handle: format!("share-{}", random_hex(6)),
        key: random_hex(20),
        name: None,
        target,
        access: Access::Public,
        owner: Owner {
            account: "alice".into(),
            email: None,
        },
        teams: vec![],
        created_ms: now_ms(),
        expires_ms: Some(now_ms() + 3_600_000),
        url: "tcp://mc-1a2b3c4d.m.share.test".into(),
        tcp: Some(TcpEndpoint {
            address: "tcp.m.share.test:26001".into(),
            host: "tcp.m.share.test".into(),
            port: 26001,
            srv_name: Some("mc-1a2b3c4d.m.share.test".into()),
            service: Some("minecraft".into()),
            route: "relay".into(),
            cloud: None,
            route_line: "Through Cua".into(),
            allow: vec![],
            check_host: false,
            max_bytes: 0,
            slot: 0,
        }),
        origin: None,
        waiting: None,
    }
}

fn space_port() -> Target {
    Target::SpacePort {
        space: SPACE.into(),
        port: PORT,
    }
}

type Ws = WebSocketStream<UnixStream>;

/// Opens the share's WebSocket as the relay does; `Err` carries the HTTP
/// status of a refusal.
async fn open(rig: &Rig, share: &Share) -> Result<Ws, u16> {
    let s = UnixStream::connect(&rig.sock).await.unwrap();
    let url = format!("ws://s1.share.test/share/pub/{}/", share.key);
    match tokio_tungstenite::client_async(url, s).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => Err(r.status().as_u16()),
        Err(e) => panic!("handshake: {e}"),
    }
}

/// A Space port that echoes everything until its peer closes; returns its
/// loopback base and a counter of connections it accepted.
async fn echo_port() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let n = accepted.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    (base, accepted)
}

/// Reads binary messages until `want` bytes arrived (or the socket ends).
async fn read_bytes(ws: &mut Ws, want: usize) -> Vec<u8> {
    let mut got = Vec::new();
    while got.len() < want {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => got.extend_from_slice(&b),
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("read: {e}"),
            Err(_) => panic!("no data in time ({} of {want} bytes)", got.len()),
        }
    }
    got
}

/// Waits for the socket to end (a Close or the stream's end).
async fn ends(ws: &mut Ws) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) => return true,
            Ok(Some(Ok(_))) => continue,
            Err(_) => return false,
        }
    }
}

#[tokio::test]
async fn bytes_flow_both_ways_through_the_one_space_port() {
    let rig = rig().await;
    let (base, accepted) = echo_port().await;
    rig.spaces.set(SPACE, PORT, &base);
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();

    let mut ws = open(&rig, &share).await.expect("upgraded");
    for chunk in [&b"\x10\x00handshake"[..], b"status", &[0u8, 1, 2, 255]] {
        ws.send(Message::Binary(chunk.to_vec().into()))
            .await
            .unwrap();
        assert_eq!(read_bytes(&mut ws, chunk.len()).await, chunk);
    }
    // Only the share's own Space and port were asked for, once.
    assert_eq!(rig.spaces.calls(), vec![(SPACE.to_owned(), PORT)]);
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_client_that_closes_ends_the_space_side_with_eof() {
    let rig = rig().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    rig.spaces
        .set(SPACE, PORT, &format!("http://{}", l.local_addr().unwrap()));
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        let mut all = Vec::new();
        // read_to_end returns only at EOF: the client's close must reach here.
        s.read_to_end(&mut all).await.unwrap();
        let _ = tx.send(all);
    });
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();
    let mut ws = open(&rig, &share).await.expect("upgraded");
    ws.send(Message::Binary(b"last words".to_vec().into()))
        .await
        .unwrap();
    ws.close(None).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(10), rx)
        .await
        .expect("the Space port saw EOF in time")
        .unwrap();
    assert_eq!(got, b"last words");
}

#[tokio::test]
async fn a_space_side_half_close_delivers_everything_then_ends() {
    let rig = rig().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    rig.spaces
        .set(SPACE, PORT, &format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        s.write_all(b"motd and goodbye").await.unwrap();
        // Half-close: no more bytes from the server, but it keeps reading.
        s.shutdown().await.unwrap();
        let mut sink = Vec::new();
        let _ = s.read_to_end(&mut sink).await;
    });
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();
    let mut ws = open(&rig, &share).await.expect("upgraded");
    assert_eq!(read_bytes(&mut ws, 16).await, b"motd and goodbye");
    assert!(
        ends(&mut ws).await,
        "the WebSocket ends after the server's EOF"
    );
}

#[tokio::test]
async fn a_closed_space_port_is_a_502_and_leaves_nothing_behind() {
    let rig = rig().await;
    // A port nothing listens on.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    rig.spaces
        .set(SPACE, PORT, &format!("http://127.0.0.1:{dead}"));
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();
    for _ in 0..20 {
        assert_eq!(open(&rig, &share).await.err(), Some(502));
    }
    // The Space unreachable altogether: also a 502, not a hang.
    let gone = tcp_share(Target::SpacePort {
        space: "local:gone".into(),
        port: PORT,
    });
    rig.store.add(gone.clone()).unwrap();
    assert_eq!(open(&rig, &gone).await.err(), Some(502));
    // The gateway still serves: a live port right after works.
    let (base, _) = echo_port().await;
    rig.spaces.set(SPACE, PORT, &base);
    let mut ws = open(&rig, &share).await.expect("upgraded");
    ws.send(Message::Binary(b"ok".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(read_bytes(&mut ws, 2).await, b"ok");
}

#[tokio::test]
async fn nothing_but_the_spaces_own_port_is_reachable() {
    let rig = rig().await;
    let (host_port, host_hits) = echo_port().await;
    let host_port: u16 = host_port.rsplit(':').next().unwrap().parse().unwrap();

    // A TCP share of a port on this computer (host loopback): refused, and
    // nothing connects to it.
    let host = tcp_share(Target::Port { port: host_port });
    rig.store.add(host.clone()).unwrap();
    assert_eq!(open(&rig, &host).await.err(), Some(403));
    // Files and folders are no TCP target either.
    let file = tcp_share(Target::File {
        path: "/etc/hosts".into(),
    });
    rig.store.add(file.clone()).unwrap();
    assert_eq!(open(&rig, &file).await.err(), Some(403));
    assert_eq!(host_hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    // Whatever the resolver says, only loopback (the daemon's forward into
    // the Space) is dialed: LAN, cloud metadata and other hosts are refused.
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();
    for base in [
        "http://10.0.0.5:25565",
        "http://192.168.1.20:25565",
        "http://169.254.169.254:80",
        "http://[fd00::1]:25565",
        "http://example.com:25565",
        "http://127.0.0.1", // no port
    ] {
        rig.spaces.set(SPACE, PORT, base);
        assert_eq!(open(&rig, &share).await.err(), Some(403), "{base}");
    }

    // Another Space's port is never asked for: a share names one Space.
    let (other, other_hits) = echo_port().await;
    rig.spaces.set(OTHER_SPACE, PORT, &other);
    let (mine, _) = echo_port().await;
    rig.spaces.set(SPACE, PORT, &mine);
    let mut ws = open(&rig, &share).await.expect("upgraded");
    ws.send(Message::Binary(b"x".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(read_bytes(&mut ws, 1).await, b"x");
    assert_eq!(other_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(
        rig.spaces
            .calls()
            .iter()
            .all(|(s, p)| s == SPACE && *p == PORT),
        "{:?}",
        rig.spaces.calls()
    );

    // A plain HTTP request to a TCP share is refused (it is not a link).
    let mut s = UnixStream::connect(&rig.sock).await.unwrap();
    s.write_all(
        format!(
            "GET /share/pub/{}/ HTTP/1.1\r\nhost: s1.share.test\r\nconnection: close\r\n\r\n",
            share.key
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 400"));
}

#[tokio::test]
async fn large_transfers_survive_backpressure_in_both_directions() {
    const SIZE: usize = 8 << 20;
    let pattern = |i: usize| (i * 31 % 251) as u8;
    let rig = rig().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    rig.spaces
        .set(SPACE, PORT, &format!("http://{}", l.local_addr().unwrap()));
    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let (mut r, mut w) = s.into_split();
        // Down: 8 MiB to the client as fast as the socket takes it.
        let down = tokio::spawn(async move {
            let data: Vec<u8> = (0..SIZE).map(pattern).collect();
            w.write_all(&data).await.unwrap();
            w.shutdown().await.unwrap();
        });
        // Up: read the client's 8 MiB slowly, so the splice must hold back.
        let mut got = Vec::with_capacity(SIZE);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = r.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
            if got.len() % (1 << 20) < n {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        down.await.unwrap();
        let ok = got.len() == SIZE && got.iter().enumerate().all(|(i, b)| *b == pattern(i));
        let _ = tx.send(ok);
    });
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();
    let ws = open(&rig, &share).await.expect("upgraded");
    let (mut sink, mut stream) = ws.split();
    let up = tokio::spawn(async move {
        let data: Vec<u8> = (0..SIZE).map(pattern).collect();
        for chunk in data.chunks(32 * 1024) {
            sink.send(Message::Binary(chunk.to_vec().into()))
                .await
                .unwrap();
        }
        // A WebSocket close ends both directions (no half-close): keep the
        // sink open until the download is read too.
        sink
    });
    // Read the 8 MiB slowly as well.
    let mut got = Vec::with_capacity(SIZE);
    while got.len() < SIZE {
        match tokio::time::timeout(Duration::from_secs(30), stream.next()).await {
            Ok(Some(Ok(Message::Binary(b)))) => {
                got.extend_from_slice(&b);
                if got.len() % (1 << 20) < b.len() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            Ok(Some(Ok(_))) => {}
            other => panic!("stream ended early at {} bytes: {other:?}", got.len()),
        }
    }
    assert!(got.iter().enumerate().all(|(i, b)| *b == pattern(i)));
    let mut sink = up.await.unwrap();
    sink.close().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .expect("upload finished")
            .unwrap(),
        "the Space port got every byte in order"
    );
}

#[tokio::test]
async fn a_space_that_goes_away_mid_splice_ends_the_connection() {
    let rig = rig().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    rig.spaces
        .set(SPACE, PORT, &format!("http://{}", l.local_addr().unwrap()));
    let (gone_tx, gone_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        s.write_all(b"hello").await.unwrap();
        // The Space is deleted: its forward closes and takes the connection
        // with it (reset, not a polite shutdown).
        let _ = gone_rx.await;
        #[allow(deprecated)] // the reset is the point of this test
        let _ = s.set_linger(Some(Duration::ZERO));
        drop(s);
    });
    let share = tcp_share(space_port());
    rig.store.add(share.clone()).unwrap();
    let mut ws = open(&rig, &share).await.expect("upgraded");
    assert_eq!(read_bytes(&mut ws, 5).await, b"hello");
    gone_tx.send(()).unwrap();
    assert!(
        ends(&mut ws).await,
        "the client's connection ends with the Space"
    );
    // Writing after the end never hangs (it fails, or is dropped).
    let late = tokio::time::timeout(
        Duration::from_secs(5),
        ws.send(Message::Binary(b"anyone?".to_vec().into())),
    )
    .await;
    assert!(late.is_ok(), "a send after the end returned in time");
    // The share's record was removed with the Space: the next open is refused.
    rig.store
        .remove_where(|s| s.handle == share.handle)
        .unwrap();
    assert!(matches!(open(&rig, &share).await, Err(404) | Err(429)));
}

#[tokio::test]
async fn a_waiting_share_and_a_cloud_host_share_serve_nothing_here() {
    let rig = rig().await;
    let (base, accepted) = echo_port().await;
    rig.spaces.set(SPACE, PORT, &base);
    // Waiting for the owner's approval: not even found.
    let mut waiting = tcp_share(space_port());
    waiting.waiting = Some(crate::model::Waiting {
        reason: "approval".into(),
        text: "an agent in local:mc asked".into(),
        route_line: "Through Cua".into(),
    });
    rig.store.add(waiting.clone()).unwrap();
    assert_eq!(open(&rig, &waiting).await.err(), Some(404));
    // Served by the Space's own cloud host: never through the gateway.
    let mut host = tcp_share(space_port());
    if let Some(t) = host.tcp.as_mut() {
        t.route = "host".into();
    }
    rig.store.add(host.clone()).unwrap();
    assert_eq!(open(&rig, &host).await.err(), Some(404));
    assert!(rig.spaces.calls().is_empty(), "no Space port was asked for");
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 0);
}
