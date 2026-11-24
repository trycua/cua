//! Presence v5 against the in-process spacesd mock: shapes, heartbeats,
//! leave reasons, batches, the send throttle and the PresenceView the apps
//! draw from. No network beyond loopback, no host effects.

use std::time::{Duration, Instant};

use cua_spaces::Spaces;
use cua_spaces::presence::{
    Cursor, CursorShape, Identity, PresenceEvent, SEND_INTERVAL, ShapeSource, now_ms,
};
use cua_spacesd_client::pb;
use cua_spacesd_client::testing::{MockAuth, MockServer};

const TOKEN: &str = "presence-v5-token-0123456789abcdef";

async fn space() -> (MockServer, tempfile::TempDir, cua_spaces::Space) {
    let srv = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        ..Default::default()
    })
    .await;
    srv.state.advertise(&["presence"]);
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let info = spaces
        .add(
            &format!("http://{}", srv.addr),
            Some(TOKEN.into()),
            Some("cua-e2e-presence".into()),
        )
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    (srv, home, space)
}

fn who(id: &str) -> Identity {
    Identity {
        id: id.into(),
        display_name: id.into(),
        color: String::new(),
        agent: false,
    }
}

const T: Duration = Duration::from_secs(5);

#[tokio::test]
async fn shapes_heartbeats_batches_and_leave_reasons_reach_the_view() {
    let (srv, _home, space) = space().await;
    let mut alice = space.join_presence(who("alice"), T).await.unwrap();
    let bob = space.join_presence(who("bob"), T).await.unwrap();
    let bob_id = bob.me().participant_id.clone();
    let alice_id = alice.me().participant_id.clone();
    assert!(!alice.uses_datagrams(), "the mock has no QUIC listener");
    let mut view = alice.view();
    let joined = alice
        .wait_for(T, 20, |e| matches!(e, PresenceEvent::Joined { .. }))
        .await
        .unwrap();
    view.apply(&joined, now_ms());

    // A cursor with a server timestamp and shape.
    bob.update_cursor(&Cursor::at(0.25, 0.5)).await.unwrap();
    let moved = alice
        .wait_for(T, 20, |e| {
            matches!(e, PresenceEvent::CursorMoved { participant_id, .. } if *participant_id == bob_id)
        })
        .await
        .unwrap();
    if let PresenceEvent::CursorMoved { cursor, .. } = &moved {
        assert!(cursor.at_ms > 0.0, "server time is carried: {cursor:?}");
        assert!(cursor.received_ms > 0.0);
    }
    view.apply(&moved, now_ms());

    // A batch of two moves arrives as two CursorMoved events, in order.
    let at = |ms: i64| pbjson_types::Timestamp {
        seconds: ms / 1000,
        nanos: ((ms % 1000) * 1_000_000) as i32,
    };
    let base = now_ms() as i64;
    let mv = |ms: i64, x: f64, shape: pb::CursorShape| pb::CursorMoved {
        participant_id: bob_id.clone(),
        cursor: Some(pb::CursorPosition {
            position: Some(pb::Point { x, y: 0.5 }),
            visible: true,
            shape: shape as i32,
            shape_source: pb::CursorShapeSource::HitTest as i32,
            ..Default::default()
        }),
        at: Some(at(ms)),
    };
    srv.state
        .presence_inject(pb::join_response::Event::CursorBatch(pb::CursorBatch {
            moves: vec![
                mv(base + 10, 0.3, pb::CursorShape::Text),
                mv(base + 20, 0.35, pb::CursorShape::Pointer),
            ],
            tick: 7,
            at: Some(at(base + 20)),
        }));
    let mut xs = vec![];
    for _ in 0..20 {
        match alice.next_event(T).await.unwrap().unwrap() {
            e @ PresenceEvent::CursorMoved { .. } => {
                if let PresenceEvent::CursorMoved { cursor, .. } = &e {
                    xs.push((cursor.x, cursor.shape));
                }
                view.apply(&e, now_ms());
                if xs.len() == 2 {
                    break;
                }
            }
            _ => continue,
        }
    }
    assert_eq!(
        xs,
        vec![(0.3, CursorShape::Text), (0.35, CursorShape::Pointer)]
    );
    assert_eq!(
        view.shape_of(&bob_id),
        Some((CursorShape::Pointer, ShapeSource::HitTest))
    );

    // The caller's own shape comes from CursorShapeChanged.
    srv.state
        .presence_inject(pb::join_response::Event::CursorShapeChanged(
            pb::CursorShapeChanged {
                participant_id: alice_id.clone(),
                shape: pb::CursorShape::ResizeEw as i32,
                source: pb::CursorShapeSource::System as i32,
                at: None,
            },
        ));
    let e = alice
        .wait_for(T, 20, |e| matches!(e, PresenceEvent::ShapeChanged { .. }))
        .await
        .unwrap();
    view.apply(&e, now_ms());
    let now = now_ms();
    let drawn = view.drawables(now + 500.0, Some((0.9, 0.9)));
    let me = drawn.iter().find(|d| d.is_me).expect("own cursor drawn");
    assert_eq!((me.x, me.y, me.shape), (0.9, 0.9, CursorShape::ResizeEw));
    let b = drawn.iter().find(|d| d.participant_id == bob_id).unwrap();
    assert_eq!(b.shape, CursorShape::Pointer);
    assert!(
        (b.x - 0.35).abs() < 1e-9,
        "settled at the newest sample: {b:?}"
    );

    // A heartbeat that no longer lists bob removes him.
    srv.state
        .presence_inject(pb::join_response::Event::RosterHeartbeat(
            pb::RosterHeartbeat {
                participant_ids: vec![alice_id.clone()],
                at: None,
            },
        ));
    let e = alice
        .wait_for(T, 20, |e| matches!(e, PresenceEvent::Heartbeat { .. }))
        .await
        .unwrap();
    view.apply(&e, now_ms());
    assert!(view.participant(&bob_id).is_none());

    // Leave reasons are carried.
    srv.state
        .presence_inject(pb::join_response::Event::ParticipantLeft(
            pb::ParticipantLeft {
                participant_id: "agent-x".into(),
                reason: pb::LeaveReason::RunEnded as i32,
            },
        ));
    let e = alice
        .wait_for(T, 20, |e| matches!(e, PresenceEvent::Left { .. }))
        .await
        .unwrap();
    assert_eq!(
        e,
        PresenceEvent::Left {
            participant_id: "agent-x".into(),
            reason: "run_ended".into()
        }
    );
    bob.leave().await.unwrap();
}

#[tokio::test]
async fn the_send_throttle_is_newest_wins_with_edges_immediate() {
    let (_srv, _home, space) = space().await;
    let mut alice = space.join_presence(who("alice"), T).await.unwrap();
    let bob = space.join_presence(who("bob"), T).await.unwrap();
    let bob_id = bob.me().participant_id.clone();
    let sender = bob.sender();
    // 30 moves in ~0.15 s: at 33 ms at most ~6 plus the edges go out, and the
    // final position always does.
    let started = Instant::now();
    for i in 0..30 {
        sender
            .update(&Cursor::at(f64::from(i) / 100.0, 0.5))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let elapsed = started.elapsed();
    // The held final position goes out at the end of its interval.
    tokio::time::sleep(Duration::from_millis(100)).await;
    sender
        .update(&Cursor::at(0.29, 0.5).hidden())
        .await
        .unwrap();
    let mut xs = vec![];
    let mut hidden = false;
    // Bounded: 200 events or the timeout.
    for _ in 0..200 {
        match alice.next_event(Duration::from_millis(400)).await {
            Ok(Some(PresenceEvent::CursorMoved {
                participant_id,
                cursor,
            })) if participant_id == bob_id => {
                if !cursor.visible {
                    hidden = true;
                    break;
                }
                xs.push(cursor.x);
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
    assert!(hidden, "the hide edge is never dropped");
    let budget = (elapsed.as_secs_f64() / SEND_INTERVAL.as_secs_f64()).ceil() as usize + 2;
    assert!(
        xs.len() <= budget && xs.len() >= 2,
        "{} sends in {elapsed:?}: {xs:?}",
        xs.len()
    );
    assert!(
        (xs.last().unwrap() - 0.29).abs() < 1e-9,
        "newest wins: {xs:?}"
    );
}

#[tokio::test]
async fn presence_settings_go_through_init() {
    let (_srv, _home, space) = space().await;
    space.set_presence_settings(Some(false)).await.unwrap();
}
