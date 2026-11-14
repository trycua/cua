//! A computer's cua-spacesd restarts behind a cached connection (This Mac's
//! Set up shares its desktop, which starts its cua-spacesd again, CUA-3974).
//! Over the mesh nothing resets the old connection: its calls go unanswered
//! until the HTTP/2 keepalive gives up and they fail with "http2 error
//! (operation timed out)". The first agent started on This Mac after a fresh
//! setup failed that way. Here a proxy in front of the relay plays that
//! mesh: on `cut`, the Space's connections it carries go silent, without a
//! reset, while new ones pass.

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MACHINE: &str = "0123abcd4567ef89";

/// Forwards to `target`; [`SilentProxy::cut`] silences the Space's
/// connections open at that moment (the relay directory's requests and
/// every later connection still pass).
struct SilentProxy {
    url: String,
    cut: Arc<AtomicU64>,
}

impl SilentProxy {
    async fn start(target: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let cut = Arc::new(AtomicU64::new(0));
        let epochs = cut.clone();
        let target = target.trim_start_matches("http://").to_string();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                let epoch = epochs.load(Ordering::SeqCst);
                let epochs = epochs.clone();
                let target = target.clone();
                tokio::spawn(async move {
                    let Ok(server) = TcpStream::connect(&target).await else {
                        return;
                    };
                    let (cr, cw) = client.into_split();
                    let (sr, sw) = server.into_split();
                    let space_conn = Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let up = pump(cr, sw, epoch, epochs.clone(), space_conn.clone(), true);
                    let down = pump(sr, cw, epoch, epochs, space_conn, false);
                    let _ = tokio::join!(up, down);
                });
            }
        });
        Self { url, cut }
    }

    fn cut(&self) {
        self.cut.fetch_add(1, Ordering::SeqCst);
    }
}

/// Copies `from` to `to` until the connection is cut (one of the Space's,
/// opened before the latest cut): then it reads and drops, never closing.
async fn pump(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    epoch: u64,
    epochs: Arc<AtomicU64>,
    space_conn: Arc<std::sync::atomic::AtomicBool>,
    client_side: bool,
) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut first = true;
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        // The Space's own connections (gRPC over HTTP/2, its HTTP calls
        // under /m/<machine>); not the relay directory's.
        if client_side
            && first
            && (buf[..n].starts_with(b"PRI * HTTP/2.0")
                || String::from_utf8_lossy(&buf[..n]).contains(" /m/"))
        {
            space_conn.store(true, Ordering::SeqCst);
        }
        first = false;
        if space_conn.load(Ordering::SeqCst) && epochs.load(Ordering::SeqCst) > epoch {
            // Silent: the bytes go nowhere and the socket stays open.
            continue;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

struct World {
    relay: FakeRelay,
    proxy: SilentProxy,
    _srv: MockServer,
    _home: tempfile::TempDir,
    spaces: Spaces,
}

async fn world() -> World {
    let relay = FakeRelay::start().await;
    relay.add_account("tok", "ada", None);
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "tok",
            &cua_host::relay::RegisterRequest {
                id: MACHINE.into(),
                name: "This Mac".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_session(MACHINE, true, Some(1_700_000_000));
    let srv = MockServer::start(MockAuth {
        token: None,
        gateway: None,
        prefix: None,
    })
    .await;
    relay.tunnel(MACHINE, &srv.url());
    let proxy = SilentProxy::start(relay.url.clone()).await;
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(home.path())
        .probe_timeout(Duration::from_secs(10))
        .relay(RelayAccount::new(
            proxy.url.clone(),
            Arc::new(StaticToken("tok".into())),
        ))
        .build();
    spaces.list_all().await.unwrap();
    World {
        relay,
        proxy,
        _srv: srv,
        _home: home,
        spaces,
    }
}

/// Well inside the keepalive (20 s + 10 s) a dead connection would wait.
const PROMPT: Duration = Duration::from_secs(12);

async fn echo(space: &cua_spaces::Space) -> String {
    tokio::time::timeout(PROMPT, space.bash("echo alive", Duration::from_secs(10)))
        .await
        .expect("the call hung on a dead connection")
        .unwrap()
        .render()
}

/// The relay lists the computer with a new session (its cua-spacesd
/// joined again): the next use of the Space dials again.
#[tokio::test]
async fn a_computer_that_connected_again_is_dialed_again() {
    let w = world().await;
    let id = format!("relay:{MACHINE}");
    let space = w.spaces.space(&id).await.unwrap();
    assert!(echo(&space).await.contains("alive"));

    // Set up shares This Mac's desktop: its cua-spacesd starts again.
    w.proxy.cut();
    w.relay.set_session(MACHINE, true, Some(1_700_000_120));
    w.spaces.list_all().await.unwrap();

    let space = w.spaces.space(&id).await.unwrap();
    assert!(echo(&space).await.contains("alive"));
}

/// Before the relay's listing says so, starting an agent checks the cached
/// connection first: a silent one is replaced, so the start goes out once,
/// on a connection that answers.
#[tokio::test]
async fn a_checked_space_replaces_a_silent_connection() {
    let w = world().await;
    let id = format!("relay:{MACHINE}");
    let space = w.spaces.space(&id).await.unwrap();
    assert!(echo(&space).await.contains("alive"));

    w.proxy.cut();

    let started = std::time::Instant::now();
    let checked = tokio::time::timeout(PROMPT, w.spaces.checked(space))
        .await
        .expect("the check hung")
        .unwrap();
    assert!(echo(&checked).await.contains("alive"));
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    // And the cache holds the new connection.
    let again = w.spaces.space(&id).await.unwrap();
    assert!(echo(&again).await.contains("alive"));
}

/// A live connection passes the check as it is.
#[tokio::test]
async fn a_live_connection_passes_the_check() {
    let w = world().await;
    let space = w.spaces.space(&format!("relay:{MACHINE}")).await.unwrap();
    let checked = w.spaces.checked(space).await.unwrap();
    assert!(echo(&checked).await.contains("alive"));
}
