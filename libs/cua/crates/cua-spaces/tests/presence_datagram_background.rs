//! A presence join never waits for the datagram channel.
//!
//! The viewer joins presence before it opens its desktop stream, so a join
//! that waited out a QUIC channel that cannot open (a container behind a
//! port forward has no UDP route) held every Desktop on "Connecting…" for
//! the whole join timeout (10 s). `join_presence` returns at once and opens
//! the channel behind it; cursors ride the stream until it is up.
//! (This binary registers the process's one datagram channel: a fake.)

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces::presence::{
    Cursor, CursorUplink, DatagramChannel, Identity, PresenceDatagrams, PresenceEvent,
    register_presence_datagrams,
};
use cua_spacesd_client::pb;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use tokio::sync::mpsc;

const TOKEN: &str = "presence-datagram-token-0123456789abcdef";
const T: Duration = Duration::from_secs(5);

struct Uplink;

#[async_trait::async_trait]
impl CursorUplink for Uplink {
    async fn send(&self, _cursor: &Cursor) -> cua_spaces::Result<bool> {
        Ok(true)
    }
}

/// A channel that takes `delay` to open, then hands out `events`' receiver.
struct Slow {
    delay: Duration,
    events: std::sync::Mutex<Option<mpsc::Receiver<PresenceEvent>>>,
    asked: AtomicBool,
}

impl PresenceDatagrams for Slow {
    fn connect<'a>(
        &'a self,
        _space: &'a cua_spaces::Space,
        _info: &'a pb::PresenceDatagrams,
        _timeout: Duration,
    ) -> cua_spaces::extension::BoxFuture<'a, cua_spaces::Result<DatagramChannel>> {
        Box::pin(async move {
            self.asked.store(true, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            let rx = self.events.lock().unwrap().take().expect("one connect");
            Ok((Arc::new(Uplink) as Arc<dyn CursorUplink>, rx))
        })
    }
}

fn who(id: &str) -> Identity {
    Identity {
        id: id.into(),
        display_name: id.into(),
        color: String::new(),
        agent: false,
    }
}

#[tokio::test]
async fn the_join_returns_at_once_and_the_channel_opens_behind_it() {
    let (events_tx, events_rx) = mpsc::channel(8);
    let slow = Arc::new(Slow {
        delay: Duration::from_millis(600),
        events: std::sync::Mutex::new(Some(events_rx)),
        asked: AtomicBool::new(false),
    });
    register_presence_datagrams(slow.clone());

    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    srv.state.advertise(&["presence"]);
    srv.state.offer_presence_datagrams(pb::PresenceDatagrams {
        endpoint: Some(pb::QuicEndpoint {
            port: 1,
            certificate_sha256: "00".repeat(32),
            alpn: String::new(),
        }),
        ticket: "t".into(),
        ..Default::default()
    });
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let info = spaces
        .add(
            &format!("http://{}", srv.addr),
            Some(TOKEN.into()),
            Some("cua-presence-datagrams".into()),
        )
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();

    // The join does not wait for the 600 ms channel.
    let began = Instant::now();
    let mut presence = space.join_presence(who("viewer"), T).await.unwrap();
    assert!(
        began.elapsed() < Duration::from_millis(300),
        "the join waited {:?} for the datagram channel",
        began.elapsed()
    );
    assert!(
        slow.asked.load(Ordering::SeqCst) || {
            tokio::time::sleep(Duration::from_millis(50)).await;
            slow.asked.load(Ordering::SeqCst)
        }
    );
    assert!(
        !presence.uses_datagrams(),
        "cursors ride the stream until the channel is up"
    );

    // It opens behind the join: the uplink attaches and its events arrive.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !presence.uses_datagrams() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(presence.uses_datagrams(), "the channel never opened");
    events_tx
        .send(PresenceEvent::CursorMoved {
            participant_id: "someone".into(),
            cursor: Cursor::at(0.5, 0.5),
        })
        .await
        .unwrap();
    let moved = presence
        .wait_for(T, 20, |e| matches!(e, PresenceEvent::CursorMoved { participant_id, .. } if participant_id == "someone"))
        .await
        .unwrap();
    assert!(matches!(moved, PresenceEvent::CursorMoved { .. }));
}
