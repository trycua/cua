// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live streaming e2e against a real linux container (the streaming client
//! ships with Cua Spaces: `cua_spaces_ext::stream`). Run through
//! `libs/cua/crates/cua-spaces/tests/e2e/run-docker-e2e.sh`, which starts
//! the container with
//! `--memory=4g` and exports:
//!
//! - `CUA_SPACES_E2E_URL`   `http://127.0.0.1:<published 3211>`
//! - `CUA_SPACES_E2E_TOKEN` the spacesd token it started the guest with
//!
//! Without them every test here is skipped (prints why and passes).
//! Host safety: the only host-side effects are temp directories, a
//! loopback HTTP server, and a hotspot dialer that refuses everything but
//! that server..

use cua_spaces::presence::{Identity, PresenceEvent};
use cua_spaces::stream::{FrameSink, StreamEvent, StreamOptions, StreamTarget, VideoFrame};
use cua_spaces::{Space, Spaces};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let token = std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default();
    Some((url, token))
}

macro_rules! require_target {
    () => {
        match target() {
            Some(t) => t,
            None => {
                eprintln!("skipped: set CUA_SPACES_E2E_URL (run tests/e2e/run-docker-e2e.sh)");
                return;
            }
        }
    };
}

async fn connect(reg: &std::path::Path) -> (Spaces, Space) {
    cua_spaces_ext::stream::register();
    let (url, token) = target().unwrap();
    let spaces = Spaces::builder()
        .home(reg)
        .download_dir(reg.join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("e2e".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    (spaces, space)
}

async fn sh(space: &Space, cmd: &str) -> String {
    let out = space.bash(cmd, Duration::from_secs(120)).await.unwrap();
    assert!(out.success(), "{cmd}: {}", out.render());
    out.stdout
}

/// Counts frames; keeps no pixels (memory-bounded by construction).
#[derive(Default)]
struct Counter {
    frames: AtomicU64,
    keyframes: AtomicU64,
    first_is_key: Mutex<Option<bool>>,
    size: Mutex<(u32, u32)>,
    closed: AtomicU64,
}

impl FrameSink for Counter {
    fn on_frame(&self, f: VideoFrame) {
        self.first_is_key.lock().unwrap().get_or_insert(f.keyframe);
        *self.size.lock().unwrap() = (f.width, f.height);
        self.frames.fetch_add(1, Ordering::SeqCst);
        if f.keyframe {
            self.keyframes.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn on_event(&self, e: StreamEvent) {
        if matches!(e, StreamEvent::Closed { .. }) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

async fn wait_frames(c: &Counter, n: u64) {
    // Bounded: 300 polls of 100 ms.
    for _ in 0..300 {
        if c.frames.load(Ordering::SeqCst) >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("only {} frames arrived", c.frames.load(Ordering::SeqCst));
}

#[tokio::test]
async fn e2e_stream_a_window_and_the_desktop() {
    require_target!();
    let reg = tempfile::tempdir().unwrap();
    let (_spaces, space) = connect(reg.path()).await;
    sh(&space, "cua-fixtures start grid >/dev/null 2>&1 || true").await;

    let mut window = None;
    for _ in 0..60 {
        if let Ok(w) = space.find_window("grid").await {
            window = Some(w);
            break;
        }
        // The fixture's app name may be python; fall back to the title.
        if let Some(w) = space
            .windows(None)
            .await
            .unwrap()
            .into_iter()
            .find(|w| w.title.contains("CUA Fixture Grid"))
        {
            window = Some(w);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let window = window.expect("the grid fixture window is listed");

    let counter = Arc::new(Counter::default());
    let session = space
        .stream_session(
            StreamTarget::Window(window.window_id.clone()),
            StreamOptions {
                max_fps: 10,
                ..Default::default()
            },
            counter.clone(),
            None,
        )
        .await
        .unwrap();
    wait_frames(&counter, 1).await;
    assert_eq!(
        *counter.first_is_key.lock().unwrap(),
        Some(true),
        "keyframe first"
    );
    let (w, h) = *counter.size.lock().unwrap();
    assert!(w > 0 && h > 0);
    // Capture is damage-driven: a static window sends one frame. Asking for
    // a keyframe must produce another (the resync path a decoder uses).
    session.request_keyframe().unwrap();
    wait_frames(&counter, 2).await;
    let stats = session.close().await.unwrap();
    assert!(stats.frames >= 2 && stats.keyframes >= 2, "{stats:?}");
    assert_eq!(stats.keyframe_requests, 1);

    let desktop = Arc::new(Counter::default());
    let session = space
        .stream_session(
            StreamTarget::Display(None),
            StreamOptions {
                max_fps: 5,
                max_dimension: 800,
                ..Default::default()
            },
            desktop.clone(),
            None,
        )
        .await
        .unwrap();
    wait_frames(&desktop, 1).await;
    assert_eq!(*desktop.first_is_key.lock().unwrap(), Some(true));
    session.close().await.unwrap();
    sh(&space, "cua-fixtures stop grid >/dev/null 2>&1 || true").await;
}

/// Presence tests run one at a time: a process names the participant it
/// last joined a Space as on the media sessions it opens there.
static PRESENCE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Collects frames and the media socket's input acknowledgements.
#[derive(Default)]
struct Viewer {
    frames: AtomicU64,
    acks: Mutex<Vec<serde_json::Value>>,
}

impl FrameSink for Viewer {
    fn on_frame(&self, _f: VideoFrame) {
        self.frames.fetch_add(1, Ordering::SeqCst);
    }
    fn on_event(&self, e: StreamEvent) {
        if let StreamEvent::Message(v) = e
            && v["type"] == "interactive_input_acknowledgement"
        {
            self.acks.lock().unwrap().push(v["payload"].clone());
        }
    }
}

impl Viewer {
    async fn frame(&self) {
        for _ in 0..300 {
            if self.frames.load(Ordering::SeqCst) > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("no frame arrived");
    }

    async fn ack(&self) -> serde_json::Value {
        for _ in 0..300 {
            if let Some(ack) = self.acks.lock().unwrap().first() {
                return ack.clone();
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("no input acknowledgement arrived");
    }
}

fn click(x: f64, y: f64) -> Vec<cua_media_protocol::InteractiveInputEvent> {
    use cua_media_protocol::{InputPointerButton, InputPointerPhase, InteractiveInputEvent};
    [InputPointerPhase::Down, InputPointerPhase::Up]
        .into_iter()
        .map(|phase| InteractiveInputEvent::Pointer {
            phase,
            button: Some(InputPointerButton::Left),
            x_normalized: x,
            y_normalized: y,
            modifiers: Vec::new(),
        })
        .collect()
}

/// A viewer's stream input is the viewer's own: attributed to its presence
/// participant (the input lease it takes names it) and never shown as an
/// agent. Regression: every viewer's input was anonymous, and on Hyprland it
/// added a second cursor named "CUA agent".
#[tokio::test]
async fn e2e_viewer_input_is_attributed_to_its_presence() {
    require_target!();
    let _presence = PRESENCE.lock().await;
    let reg_a = tempfile::tempdir().unwrap();
    let reg_b = tempfile::tempdir().unwrap();
    let (_a, space_a) = connect(reg_a.path()).await;
    let (_b, space_b) = connect(reg_b.path()).await;
    let t = Duration::from_secs(20);
    let human = |id: &str, name: &str| Identity {
        id: id.into(),
        display_name: name.into(),
        ..Default::default()
    };
    let options = || StreamOptions {
        max_fps: 5,
        max_dimension: 800,
        policy: Some(cua_spacesd_client::pb::SessionPolicy::AllowActivation),
        ..Default::default()
    };

    // Dana joins presence, then opens the desktop stream (naming her
    // participant) and clicks through it.
    let mut dana = space_a
        .join_presence(human("cua-e2e-dana", "Dana"), t)
        .await
        .unwrap();
    assert_eq!(
        space_a.presence_participant().as_deref(),
        Some(dana.me().participant_id.as_str())
    );
    let dana_view = Arc::new(Viewer::default());
    let dana_stream = space_a
        .stream_session(
            StreamTarget::Display(None),
            options(),
            dana_view.clone(),
            None,
        )
        .await
        .unwrap();
    dana_view.frame().await;
    dana_stream.send_input(click(0.5, 0.5)).unwrap();
    let ack = dana_view.ack().await;
    assert_eq!(ack["delivered"], true, "{ack}");

    // Bob, another viewer, cannot take the desktop while Dana holds it: the
    // lease is Dana's by name, so her input was attributed to her.
    let bob = space_b
        .join_presence(human("cua-e2e-bob", "Bob"), t)
        .await
        .unwrap();
    let bob_view = Arc::new(Viewer::default());
    let bob_stream = space_b
        .stream_session(
            StreamTarget::Display(None),
            options(),
            bob_view.clone(),
            None,
        )
        .await
        .unwrap();
    bob_view.frame().await;
    // Dana keeps driving (a lease frees after 5 s without input).
    dana_view.acks.lock().unwrap().clear();
    dana_stream.send_input(click(0.5, 0.5)).unwrap();
    assert_eq!(dana_view.ack().await["delivered"], true);
    bob_stream.send_input(click(0.5, 0.5)).unwrap();
    let refused = bob_view.ack().await;
    assert_eq!(refused["delivered"], false, "{refused}");
    let message = refused["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("leased to Dana"), "{refused}");

    // Neither viewer's input added an agent to presence.
    let agent = dana
        .wait_for(
            Duration::from_secs(3),
            200,
            |e| matches!(e, PresenceEvent::Joined { participant } if participant.kind == "agent"),
        )
        .await;
    assert!(agent.is_err(), "a viewer's input added an agent: {agent:?}");
    assert!(
        dana.roster().iter().all(|(p, _)| p.kind != "agent"),
        "{:?}",
        dana.roster()
    );

    bob_stream.close().await.unwrap();
    dana_stream.close().await.unwrap();
    bob.leave().await.unwrap();
    dana.leave().await.unwrap();
}
