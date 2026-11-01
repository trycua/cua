//! A process stream survives HTTP/2 flood protection closing its connection
//! (`GOAWAY ENHANCE_YOUR_CALM`, which tonic reports as `ResourceExhausted`):
//! it resumes with `ConnectProcess` from the last offset, with every byte
//! delivered exactly once.
//!
//! The server is a bare `h2` server speaking just enough gRPC to serve
//! `ConnectProcess`, so the test controls the frames: one tiny DATA frame
//! per output line, the way a command printing many short lines reaches a
//! guest spacesd that does not coalesce them.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use cua_spacesd_client::{
    ConnectOptions, ProcessEvent, ProcessRef, Replay, RetryPolicy, SpacesdClient, Transport,
    TransportPreference, pb,
};
use prost::Message as _;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;

/// More lines than the stream window holds (4 MiB / ~25 bytes per message),
/// so a reader that stalls is either flooded or stops the sender by flow
/// control: never a race.
const LINES: u64 = 200_000;
const EXIT_CODE: i32 = 7;

fn line(i: u64) -> Vec<u8> {
    format!("line {i:06}\n").into_bytes()
}

fn expected() -> Vec<u8> {
    (0..LINES).flat_map(line).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Keep sending tiny frames; the client's own h2 closes the connection
    /// when it falls behind.
    Flood,
    /// After this many lines the server closes the first connection with
    /// `GOAWAY ENHANCE_YOUR_CALM` itself.
    GoAway(u64),
    /// As `GoAway`, and the resumed stream starts this many lines later:
    /// the Space's scrollback had moved on.
    GoAwayThenSkip(u64, u64),
}

struct Server {
    addr: SocketAddr,
    connects: Arc<AtomicU64>,
    /// Output frames sent on the first connection.
    sent: Arc<AtomicU64>,
}

/// Tiny frames that must sit unread in the client before h2's flood
/// protection fires: its budget is half the connection window, and a DATA
/// frame of `len` < 256 bytes costs `256 - len` (h2 0.4 `counts.rs`
/// `record_data_frame`). Each output line here is one gRPC message of
/// about 25 bytes; 10% margin on top.
fn flood_frames() -> u64 {
    let budget = u64::from(cua_spacesd_client::transport::CONNECTION_WINDOW) / 2;
    budget / (256 - 25) * 11 / 10
}

fn grpc_frame(msg: &impl prost::Message) -> Bytes {
    let body = msg.encode_to_vec();
    let mut b = BytesMut::with_capacity(5 + body.len());
    b.put_u8(0);
    b.put_u32(body.len() as u32);
    b.extend_from_slice(&body);
    b.freeze()
}

fn event(e: pb::process_event::Event) -> Bytes {
    grpc_frame(&pb::ConnectProcessResponse {
        event: Some(pb::ProcessEvent { event: Some(e) }),
    })
}

async fn send(out: &mut h2::SendStream<Bytes>, data: Bytes) -> Result<(), h2::Error> {
    out.reserve_capacity(data.len());
    while out.capacity() < data.len() {
        std::future::poll_fn(|cx| out.poll_capacity(cx))
            .await
            .ok_or_else(|| h2::Error::from(h2::Reason::CANCEL))??;
    }
    out.send_data(data, false)
}

async fn serve_stream(
    mut req: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    mode: Mode,
    connects: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
    closer: Arc<tokio::sync::Notify>,
) -> Result<(), h2::Error> {
    let mut body = BytesMut::new();
    while let Some(chunk) = req.body_mut().data().await {
        let chunk = chunk?;
        req.body_mut()
            .flow_control()
            .release_capacity(chunk.len())?;
        body.extend_from_slice(&chunk);
    }
    body.advance(5);
    let r = pb::ConnectProcessRequest::decode(body.freeze()).expect("request");
    let n = connects.fetch_add(1, Ordering::SeqCst);
    let from = r.replay_from_offset.unwrap_or(0);
    let response = http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    let mut out = respond.send_response(response, false)?;
    send(
        &mut out,
        event(pb::process_event::Event::Start(pb::ProcessStart {
            pid: 42,
            tag: "flood".into(),
            output_end_offset: 12 * LINES,
            ..Default::default()
        })),
    )
    .await?;
    let mut first = (from / 12).min(LINES);
    // Only the first resume finds the scrollback moved on; a later resume
    // (a slow reader can be flooded again) continues where it asked.
    if let Mode::GoAwayThenSkip(_, skip) = mode
        && n == 1
    {
        first += skip;
    }
    let cut = match mode {
        Mode::GoAway(at) | Mode::GoAwayThenSkip(at, _) => Some(at),
        Mode::Flood => None,
    };
    for i in first..LINES {
        if cut == Some(i - first) && n == 0 {
            // The reader has consumed what was sent; then the peer says it
            // has had enough.
            tokio::time::sleep(Duration::from_millis(300)).await;
            closer.notify_one();
            // Let the GOAWAY go out before this stream is dropped.
            tokio::time::sleep(Duration::from_secs(5)).await;
            return Ok(());
        }
        send(
            &mut out,
            event(pb::process_event::Event::Data(pb::ProcessData {
                offset: i * 12,
                output: Some(pb::process_data::Output::Stdout(line(i))),
            })),
        )
        .await?;
        if n == 0 {
            sent.fetch_add(1, Ordering::SeqCst);
        }
    }
    send(
        &mut out,
        event(pb::process_event::Event::End(pb::ProcessEnd {
            exit_code: Some(EXIT_CODE),
            ..Default::default()
        })),
    )
    .await?;
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", "0".parse().unwrap());
    out.send_trailers(trailers)
}

async fn start(mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connects = Arc::new(AtomicU64::new(0));
    let sent = Arc::new(AtomicU64::new(0));
    let (c, sc) = (connects.clone(), sent.clone());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let (connects, sent, closer) =
                (c.clone(), sc.clone(), Arc::new(tokio::sync::Notify::new()));
            tokio::spawn(async move {
                let Ok(mut conn) = h2::server::handshake(tcp).await else {
                    return;
                };
                loop {
                    tokio::select! {
                        next = conn.accept() => {
                            let Some(Ok((req, respond))) = next else { return };
                            let (connects, sent, closer) =
                                (connects.clone(), sent.clone(), closer.clone());
                            tokio::spawn(async move {
                                let _ =
                                    serve_stream(req, respond, mode, connects, sent, closer).await;
                            });
                        }
                        _ = closer.notified() => {
                            conn.abrupt_shutdown(h2::Reason::ENHANCE_YOUR_CALM);
                            let _ = std::future::poll_fn(|cx| conn.poll_closed(cx)).await;
                            return;
                        }
                    }
                }
            });
        }
    });
    Server {
        addr,
        connects,
        sent,
    }
}

async fn drain(mode: Mode, slow_first: bool) {
    let skipped = match mode {
        Mode::GoAwayThenSkip(_, lines) => lines,
        _ => 0,
    };
    let server = start(mode).await;
    let opts = ConnectOptions::parse(&format!("http://{}", server.addr))
        .unwrap()
        .transport(TransportPreference::Native)
        .probe(false)
        .retry(RetryPolicy {
            max_attempts: 8,
            initial_backoff: Duration::from_millis(20),
            max_backoff: Duration::from_millis(200),
        });
    let client = SpacesdClient::connect(opts).await.expect("connect");
    assert_eq!(client.transport(), Transport::Native);
    let mut handle = client
        .attach(ProcessRef::Tag("flood".into()), Replay::All)
        .await
        .expect("attach");
    let mut got = Vec::new();
    let mut exit = None;
    let mut slowed = false;
    while let Some(ev) = tokio::time::timeout(Duration::from_secs(60), handle.next_event())
        .await
        .expect("stream stalled")
        .expect("the stream must survive the flood GOAWAY")
    {
        match ev {
            ProcessEvent::Stdout { offset, data } => {
                if skipped == 0 {
                    assert_eq!(offset, got.len() as u64, "no gap, no duplicate");
                }
                got.extend_from_slice(&data);
                if slow_first && !slowed {
                    // A reader that stops polling while the server writes,
                    // until the server can send no more: either h2's flood
                    // protection closed the connection (the expected case,
                    // after flood_frames() unread frames) or flow control
                    // stopped it (no flood: the assertion below fails).
                    // Deterministic, not a race against a fixed sleep.
                    slowed = true;
                    assert!(
                        flood_frames() < LINES,
                        "the test must send more lines than the budget"
                    );
                    let (mut last, mut still) = (u64::MAX, 0);
                    for _ in 0..1200 {
                        let now = server.sent.load(Ordering::SeqCst);
                        still = if now == last { still + 1 } else { 0 };
                        if still >= 20 {
                            break;
                        }
                        last = now;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
            ProcessEvent::Exit(s) => exit = Some(s),
            other => panic!("unexpected {other:?}"),
        }
    }
    if skipped == 0 {
        assert_eq!(handle.dropped_bytes(), 0);
        assert_eq!(got.len(), expected().len());
        assert!(got == expected(), "output differs");
    } else {
        // The skipped lines are reported as dropped, never silently absent;
        // everything else is exact.
        assert_eq!(handle.dropped_bytes(), skipped * 12);
        let all = expected();
        let cut = got.len() - (all.len() - (60_000 + skipped as usize) * 12);
        assert_eq!(&got[..cut], &all[..cut]);
        assert_eq!(&got[cut..], &all[(60_000 + skipped as usize) * 12..]);
    }
    assert_eq!(exit.expect("exit").code, Some(EXIT_CODE));
    assert!(
        handle.reconnects() >= 1 && server.connects.load(Ordering::SeqCst) >= 2,
        "the flood GOAWAY must have forced a resume (reconnects {})",
        handle.reconnects()
    );
}

/// The client's own h2 flood protection fires: the reader stops polling for
/// 1.5 s while the server sends one tiny frame per line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_real_flood_of_tiny_frames_is_resumed_without_loss() {
    drain(Mode::Flood, true).await;
}

/// The peer closes the connection with `GOAWAY ENHANCE_YOUR_CALM` (what a
/// server running h2's flood protection answers a slow reader with).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_goaway_enhance_your_calm_is_resumed_without_loss() {
    drain(Mode::GoAway(60_000), false).await;
}

/// The resumed stream starts after the offset asked for (the Space's
/// scrollback ring had moved past it): the lost bytes are counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_the_space_no_longer_holds_is_counted_not_hidden() {
    drain(Mode::GoAwayThenSkip(60_000, 100), false).await;
}
