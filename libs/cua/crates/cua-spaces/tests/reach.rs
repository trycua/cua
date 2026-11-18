//! Whether a Space can take work now (`Spaces::reach`), asked just before
//! a job is handed to it.
//!
//! The second device test of the new Home (10 Nov) switched off the computer
//! Run something had picked and pressed Run at once. The relay still listed
//! it online (it does for up to a minute after a hard power-off), the job
//! was posted to the thread of the agent already on it (the thread lives on
//! the relay, so the post succeeded) and sat queued for a computer that was
//! gone, never moved to another one. `reach` is what the apps ask first: it
//! finds such a computer gone within seconds, so the job moves.
//!
//! The proxy plays the mesh as in `offline_presence.rs`: on `cut`, the
//! Space's open connections go silent without a reset.

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use cua_spaces_contract::presence::says_listed_offline;
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

/// Within seconds: the health check plus one refused dial
/// (`cua_spaces::reach::REACH_WITHIN`).
const PROMPT: Duration = Duration::from_secs(9);

/// An address nothing listens on: the relay's tunnel to a computer that is
/// switched off.
async fn dead_address() -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    format!("http://{addr}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_computer_that_answers_is_reachable_at_once() {
    let w = world().await;
    let t = Instant::now();
    let r = w.spaces.reach(SPACE).await;
    assert!(r.reachable, "{r:?}");
    assert!(!r.listed_offline);
    assert_eq!(r.reason, None);
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
}

/// The computer goes away between being picked and the job starting: it was
/// connected (Run something picked it), then switched off. The relay still
/// lists it online, but `reach` finds it gone within seconds, so the app
/// moves the job instead of queueing it for a computer that is not there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_computer_switched_off_after_it_was_picked_is_unreachable_before_the_job_starts() {
    let w = world().await;
    assert!(w.spaces.reach(SPACE).await.reachable, "picked while online");
    w.spaces
        .space(SPACE)
        .await
        .expect("its connection is cached");

    // Switched off: its open connection goes silent, and the relay's tunnel
    // finds nothing, while the relay's directory still lists it online.
    w.proxy.cut();
    w.relay.tunnel(MACHINE, &dead_address().await);
    w.spaces.list_all().await.unwrap();

    let t = Instant::now();
    let r = w.spaces.reach(SPACE).await;
    assert!(
        !r.reachable,
        "a computer that is gone can't take the job: {r:?}"
    );
    assert!(!r.listed_offline, "the relay has not noticed yet");
    assert!(r.reason.is_some());
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());
}

/// The relay lists it offline and its connection does not answer: not
/// reachable, in the words the apps read as offline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_computer_the_relay_lists_offline_is_unreachable_and_says_so() {
    let w = world().await;
    w.spaces.space(SPACE).await.unwrap();
    w.proxy.cut();
    w.relay.set_online(MACHINE, false, "");
    w.spaces.list_all().await.unwrap();

    let t = Instant::now();
    let r = w.spaces.reach(SPACE).await;
    assert!(!r.reachable, "{r:?}");
    assert!(r.listed_offline);
    let reason = r.reason.expect("a reason");
    assert!(says_listed_offline(&reason), "{reason}");
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());

    // Back online: reachable again.
    w.relay.set_session(MACHINE, true, Some(1_700_000_100));
    w.spaces.list_all().await.unwrap();
    let r = tokio::time::timeout(PROMPT, w.spaces.reach(SPACE))
        .await
        .unwrap();
    assert!(r.reachable, "{r:?}");
}

/// The app method the apps call (`spaces.reach` through `CallSpaceTool`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_method_answers_with_the_reach() {
    let w = world().await;
    let out = cua_spaces::reach::app_call(
        &w.spaces,
        "spaces.reach",
        serde_json::json!({ "space": SPACE }),
    )
    .await
    .expect("an app method");
    assert!(!out.is_error);
    let v: serde_json::Value = serde_json::from_str(out.first_text().unwrap()).unwrap();
    assert_eq!(v["reachable"], true);
    assert!(
        cua_spaces::reach::app_call(&w.spaces, "list_spaces", serde_json::json!({}))
            .await
            .is_none()
    );
    let bad = cua_spaces::reach::app_call(&w.spaces, "spaces.reach", serde_json::json!({}))
        .await
        .unwrap();
    assert!(bad.is_error);
}
