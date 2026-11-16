//! A computer's presence follows the relay, not a cached connection
//! (CUA-3981).
//!
//! The device test of the new Home quit Cua Spaces on the second computer.
//! The relay listed it offline at once (`cua` said so), but the first
//! computer's app kept showing it Online, and Run something kept picking it,
//! for over five minutes: the app's reachability probe is
//! [`Spaces::space`], and it answered with the connection cached from
//! before. That connection ran over the mesh, which sends no reset when the
//! other side disappears, and nothing was sent on it to find out.
//!
//! Here a proxy in front of the relay plays that mesh: on `cut`, the Space's
//! connections it carries at that moment go silent, without a reset, while
//! new ones pass (an app quit without a clean disconnect).

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use cua_spaces_contract::presence::{LISTED_OFFLINE, says_listed_offline};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MACHINE: &str = "0123abcd4567ef89";
const SPACE: &str = "relay:0123abcd4567ef89";

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
                    let space_conn = Arc::new(AtomicBool::new(false));
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
    space_conn: Arc<AtomicBool>,
    client_side: bool,
) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut first = true;
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        // The Space's own connections (gRPC over HTTP/2, its calls under
        // /m/<machine>); not the relay directory's.
        if client_side
            && first
            && (buf[..n].starts_with(b"PRI * HTTP/2.0")
                || String::from_utf8_lossy(&buf[..n]).contains(" /m/"))
        {
            space_conn.store(true, Ordering::SeqCst);
        }
        first = false;
        if space_conn.load(Ordering::SeqCst) && epochs.load(Ordering::SeqCst) > epoch {
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
                name: "j17-2".into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    relay.set_session(MACHINE, true, Some(1_700_000_000));
    // As the relay does: no tunnel to a computer that is not connected.
    relay.set_refuse_offline(true);
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

/// What the list says of the computer (`offline` when the relay says so).
fn status(spaces: &Spaces) -> String {
    spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == SPACE)
        .expect("the computer is listed")
        .status
}

/// Within seconds, not minutes: the live check plus one refused dial.
const PROMPT: Duration = Duration::from_secs(8);

/// The other computer's app quits without a clean disconnect: its
/// connection goes silent and the relay lists it offline. The next probe
/// does not answer with the cached connection: it says the computer is
/// offline, in the words the apps read as offline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_quit_without_a_clean_disconnect_reads_offline_within_seconds() {
    let w = world().await;
    w.spaces.space(SPACE).await.expect("online, it connects");

    w.proxy.cut();
    w.relay.set_online(MACHINE, false, "");
    w.spaces.list_all().await.unwrap();
    assert_eq!(status(&w.spaces), "offline", "the list follows the relay");

    let t = Instant::now();
    let probe = tokio::time::timeout(PROMPT, w.spaces.space(SPACE))
        .await
        .expect("the probe hung on a dead connection");
    let e = probe
        .expect_err("a cached dead connection is not reachable")
        .to_string();
    assert!(says_listed_offline(&e), "{e}");
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());

    // The next probes (the apps poll) fail at once, still as offline.
    let t = Instant::now();
    let e = w.spaces.space(SPACE).await.unwrap_err().to_string();
    assert!(e.contains(LISTED_OFFLINE), "{e}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

    // It comes back: reachable on the next probe.
    w.relay.set_session(MACHINE, true, Some(1_700_000_100));
    w.spaces.list_all().await.unwrap();
    tokio::time::timeout(PROMPT, w.spaces.space(SPACE))
        .await
        .expect("no wait once it is back")
        .expect("back online, it connects");
}

/// The relay lists it offline, but its connection still answers (the LAN
/// mesh carries it while its link to the relay is down): it stays usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offline_computer_whose_connection_answers_stays_reachable() {
    let w = world().await;
    let first = w.spaces.space(SPACE).await.unwrap();
    // The path it answers on is not the relay's tunnel (which is down):
    // here the fake relay keeps carrying it, as the LAN mesh would.
    w.relay.set_refuse_offline(false);
    w.relay.set_online(MACHINE, false, "");
    w.spaces.list_all().await.unwrap();
    let again = tokio::time::timeout(PROMPT, w.spaces.space(SPACE))
        .await
        .expect("answered in time")
        .expect("its connection answers");
    assert_eq!(first.id(), again.id());
}
