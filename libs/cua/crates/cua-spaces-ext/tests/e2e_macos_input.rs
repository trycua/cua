// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Live, opt-in: interactive input on a macOS Space's desktop stream.
//!
//! The SwiftUI, Tauri and HTML5 viewers all stream the whole display and send
//! clicks as `interactive_input` batches on the media socket. These tests do
//! exactly that against a real macOS Space: open a desktop stream with the
//! viewers' desktop policy (`allow_activation`: a display has no window to
//! address background input to), click the Finder icon in the Dock, and
//! assert the batches were acknowledged as delivered, a Finder window
//! appeared and the screen changed, then close it with ⌘W through the same
//! session.
//!
//! Set, for a running macOS Space (for example a `cua-e2e-*` Lume VM):
//!
//! - `CUA_SPACES_MACOS_E2E_URL`   `http://<vm ip>:3211`
//! - `CUA_SPACES_MACOS_E2E_TOKEN` its spacesd token (the `env_token` in
//!   `~/.cua/sandboxes/<name>.json`)
//!
//! Without them the tests are skipped. The only effects are inside the guest:
//! a Dock click, a pointer move, and ⌘W on the Finder window the click
//! opened. Use a fresh Space with no Finder window open, and run them one at
//! a time (`-- --test-threads=1`).

use cua_media_protocol::{
    InputKeyState, InputModifier, InputPointerButton, InputPointerPhase, InteractiveInputEvent,
};
use cua_spaces::stream::{
    FrameSink, StreamEvent, StreamOptions, StreamSession, StreamTarget, VideoFrame,
};
use cua_spaces::{Space, Spaces};
use cua_spacesd_client::pb;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_MACOS_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    let token = std::env::var("CUA_SPACES_MACOS_E2E_TOKEN").unwrap_or_default();
    Some((url, token))
}

macro_rules! require_target {
    () => {
        match target() {
            Some(t) => t,
            None => {
                eprintln!("skipped: set CUA_SPACES_MACOS_E2E_URL and CUA_SPACES_MACOS_E2E_TOKEN");
                return;
            }
        }
    };
}

#[derive(Default)]
struct Events {
    frames: Mutex<u64>,
    acks: Mutex<Vec<serde_json::Value>>,
}

impl FrameSink for Events {
    fn on_frame(&self, _frame: VideoFrame) {
        *self.frames.lock().unwrap() += 1;
    }
    fn on_event(&self, event: StreamEvent) {
        if let StreamEvent::Message(m) = event {
            let kind = m.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if kind.contains("interactive_input") || kind == "error" {
                self.acks.lock().unwrap().push(m);
            }
        }
    }
}

async fn connect((url, token): (String, String)) -> (tempfile::TempDir, Spaces, Space) {
    cua_spaces_ext::stream::register();
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .download_dir(reg.path().join("downloads"))
        .operator_display(Arc::new(cua_spaces::operator::NoDisplay))
        .probe_timeout(Duration::from_secs(20))
        .build();
    let info = spaces
        .add(&url, Some(token), Some("cua-e2e-macos-input".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    (reg, spaces, space)
}

/// A desktop stream that has delivered its first frame.
async fn open_desktop(space: &Space, policy: pb::SessionPolicy) -> (Arc<Events>, StreamSession) {
    let events = Arc::new(Events::default());
    let session = space
        .stream_session(
            StreamTarget::Display(None),
            StreamOptions {
                max_fps: 10,
                policy: Some(policy),
                ..Default::default()
            },
            events.clone(),
            None,
        )
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while *events.frames.lock().unwrap() == 0 {
        assert!(Instant::now() < deadline, "no frame within 20 s");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (events, session)
}

/// The first input acknowledgement (or error) of a session, within 10 s.
async fn first_ack(events: &Events) -> serde_json::Value {
    for _ in 0..100 {
        if let Some(a) = events.acks.lock().unwrap().first() {
            return a.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no input acknowledgement within 10 s");
}

async fn finder_windows(space: &Space) -> usize {
    space
        .windows(Some("Finder"))
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.on_screen && !w.title.is_empty())
        .count()
}

fn pointer(
    phase: InputPointerPhase,
    button: Option<InputPointerButton>,
    x: f64,
    y: f64,
) -> InteractiveInputEvent {
    InteractiveInputEvent::Pointer {
        phase,
        button,
        x_normalized: x,
        y_normalized: y,
        modifiers: vec![],
    }
}

#[tokio::test]
async fn e2e_macos_desktop_stream_click_on_the_dock_opens_finder() {
    let (_reg, _spaces, space) = connect(require_target!()).await;

    // Run it on a fresh Space (no Finder window open), so the click has a
    // visible effect: a new Finder window. No AppleScript: scripting the
    // Finder from the spacesd raises an Automation consent prompt.
    let before = finder_windows(&space).await;
    let shot = screenshot(&space).await;
    let (events, session) = open_desktop(&space, pb::SessionPolicy::AllowActivation).await;

    // The Finder is the Dock's first icon: 60 px in, 33 px up from the
    // bottom edge, on the image's default 1024x768 display. Normalize
    // against the screenshot, the same surface the stream encodes.
    let x = 60.0 / f64::from(shot.width);
    let y = (f64::from(shot.height) - 33.0) / f64::from(shot.height);
    session
        .send_input(vec![
            pointer(InputPointerPhase::Move, None, x, y),
            pointer(
                InputPointerPhase::Down,
                Some(InputPointerButton::Left),
                x,
                y,
            ),
        ])
        .unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    session
        .send_input(vec![pointer(
            InputPointerPhase::Up,
            Some(InputPointerButton::Left),
            x,
            y,
        )])
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let acks = events.acks.lock().unwrap().clone();
        let refused = acks
            .iter()
            .any(|a| a["payload"]["delivered"] == false || a["type"] == "error");
        assert!(!refused, "input was not delivered: {acks:#?}");
        if acks
            .iter()
            .filter(|a| a["payload"]["delivered"] == true)
            .count()
            >= 2
        {
            break;
        }
        assert!(Instant::now() < deadline, "no acknowledgement: {acks:#?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let mut after = before;
    let mut changed = 0.0;
    for _ in 0..50 {
        after = finder_windows(&space).await;
        changed = changed_fraction(&shot, &screenshot(&space).await);
        if after > before && changed > 0.05 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Keyboard through the same session: ⌘W closes the window it opened.
    if after > before {
        let key = |state| InteractiveInputEvent::Key {
            key: "w".into(),
            state,
            modifiers: vec![InputModifier::Command],
            repeat: false,
        };
        session
            .send_input(vec![key(InputKeyState::Down), key(InputKeyState::Up)])
            .unwrap();
    }
    let mut closed = after;
    for _ in 0..50 {
        closed = finder_windows(&space).await;
        if closed <= before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    session.close().await.unwrap();
    assert!(
        after > before,
        "clicking the Dock's Finder icon opened a Finder window ({before} -> {after})"
    );
    assert!(
        changed > 0.05,
        "the screen changed ({:.1}% of pixels)",
        changed * 100.0
    );
    assert!(closed <= before, "⌘W closed it ({after} -> {closed})");
}

/// The SwiftUI viewer sends its batches as raw text in the v1 envelope; the
/// Space takes that shape on a desktop stream. And `background_only` (what
/// that viewer used to ask for) is refused as would-require-activation,
/// explicitly, rather than a click that silently goes nowhere.
#[tokio::test]
async fn e2e_macos_desktop_stream_takes_the_swift_viewer_wire_shape() {
    let (_reg, _spaces, space) = connect(require_target!()).await;
    // LiveStreamSession.send (interactiveInputText) in shape: a pointer move
    // to the middle of the screen.
    let batch = |session: &StreamSession| {
        serde_json::json!({
            "direction": "client",
            "message": {
                "type": "interactive_input",
                "payload": {
                    "session_id": session.media_session_id(),
                    "first_sequence": 1,
                    "events": [{"kind": "pointer", "phase": "move", "button": null,
                                "x_normalized": 0.5, "y_normalized": 0.5, "modifiers": []}],
                },
            },
        })
        .to_string()
    };

    let (events, session) = open_desktop(&space, pb::SessionPolicy::AllowActivation).await;
    session.send_text(batch(&session)).unwrap();
    let ack = first_ack(&events).await;
    session.close().await.unwrap();
    assert_eq!(ack["type"], "interactive_input_acknowledgement", "{ack}");
    assert_eq!(ack["payload"]["delivered"], true, "{ack}");

    let (events, session) = open_desktop(&space, pb::SessionPolicy::BackgroundOnly).await;
    session.send_text(batch(&session)).unwrap();
    let ack = first_ack(&events).await;
    session.close().await.unwrap();
    assert_eq!(ack["payload"]["delivered"], false, "{ack}");
    assert_eq!(
        ack["payload"]["error"]["code"], "would_require_activation",
        "{ack}"
    );
}

struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

async fn screenshot(space: &Space) -> Pixels {
    let shot = space
        .spacesd()
        .unwrap()
        .screenshot(cua_spacesd_client::ScreenshotOptions {
            format: pb::ImageFormat::Png,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut decoder = png::Decoder::new(std::io::Cursor::new(shot.image.to_vec()));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    let channels = info.color_type.samples();
    let rgba = buf
        .chunks(channels)
        .flat_map(|p| [p[0], p[1 % channels], p[2 % channels], 255])
        .collect();
    Pixels {
        width: info.width,
        height: info.height,
        rgba,
    }
}

/// The fraction of pixels below the menu bar (whose clock ticks) that differ.
fn changed_fraction(a: &Pixels, b: &Pixels) -> f64 {
    if (a.width, a.height) != (b.width, b.height) {
        return 1.0;
    }
    let row = a.width as usize * 4;
    let skip = row * (a.height as usize / 20);
    let (a, b) = (&a.rgba[skip..], &b.rgba[skip..]);
    let differ = a
        .chunks(4)
        .zip(b.chunks(4))
        .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 24))
        .count();
    differ as f64 / (a.len() / 4) as f64
}
