// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Stream connect timing against a real linux container (opt-in: set
//! `CUA_SPACES_E2E_URL` and `CUA_SPACES_E2E_TOKEN`, as for
//! `e2e_docker_stream`; skipped without them).
//!
//! It opens the Desktop stream the way the apps do, closes it, and opens it
//! again, printing the time of each phase (run with `--nocapture`):
//! the ticket (`StreamService.OpenMedia`, which also waits for the first
//! frame inside the guest), the media socket attach, and the first frame
//! reaching the client. The thresholds are generous: they catch a fixed
//! sleep or a retry loop creeping into the path, not machine noise.

use cua_spaces::stream::{FrameSink, StreamEvent, StreamOptions, StreamTarget, VideoFrame};
use cua_spaces::{Space, Spaces};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct FirstFrame {
    at: Mutex<Option<Instant>>,
}

impl FrameSink for FirstFrame {
    fn on_frame(&self, _f: VideoFrame) {
        self.at.lock().unwrap().get_or_insert_with(Instant::now);
    }
    fn on_event(&self, _e: StreamEvent) {}
}

fn target() -> Option<(String, String)> {
    let url = std::env::var("CUA_SPACES_E2E_URL")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some((
        url,
        std::env::var("CUA_SPACES_E2E_TOKEN").unwrap_or_default(),
    ))
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
    let t = Instant::now();
    let info = spaces
        .add(&url, Some(token), Some("timing".into()))
        .await
        .unwrap();
    let space = spaces.space(&info.id).await.unwrap();
    eprintln!(
        "TIMING add+resolve {:>6.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    (spaces, space)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn options() -> StreamOptions {
    StreamOptions {
        max_fps: 30,
        ..Default::default()
    }
}

#[tokio::test]
async fn desktop_stream_connect_phases() {
    if target().is_none() {
        eprintln!("skipped: set CUA_SPACES_E2E_URL (see run-docker-e2e.sh)");
        return;
    }
    let reg = tempfile::tempdir().unwrap();
    let (_spaces, space) = connect(reg.path()).await;
    let mut totals = Vec::new();
    // The viewer joins presence before it opens its stream (so the Space
    // attributes its input), so the join is part of every cold open.
    for round in 0..2 {
        let t = Instant::now();
        let presence = space
            .join_presence(
                cua_spaces::presence::Identity {
                    id: format!("timing-{round}"),
                    display_name: "Timing".into(),
                    color: String::new(),
                    agent: false,
                },
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        eprintln!(
            "TIMING presence join round {round}: {:>6.0} ms",
            ms(t.elapsed())
        );
        let t = Instant::now();
        let _ = presence.leave().await;
        eprintln!(
            "TIMING presence leave round {round}: {:>6.0} ms",
            ms(t.elapsed())
        );
    }
    for round in 0..4 {
        // Phase 1 and 2 split: the ticket alone (and give it back), then the
        // whole open with a first-frame clock.
        let t = Instant::now();
        let ticket = space
            .open_stream(StreamTarget::Display(None), options())
            .await
            .unwrap();
        let ticket_ms = ms(t.elapsed());
        space.close_stream(&ticket.media_session_id).await.unwrap();

        let sink = Arc::new(FirstFrame::default());
        let t = Instant::now();
        let session = space
            .stream_session(StreamTarget::Display(None), options(), sink.clone(), None)
            .await
            .unwrap();
        let open_ms = ms(t.elapsed());
        let mut first = None;
        for _ in 0..200 {
            if let Some(at) = *sink.at.lock().unwrap() {
                first = Some(at);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let first_ms = ms(first.expect("a first frame arrives").duration_since(t));
        let t = Instant::now();
        session.close().await.unwrap();
        let close_ms = ms(t.elapsed());
        eprintln!(
            "TIMING round {round}: ticket {ticket_ms:>6.0} ms | open (ticket + socket) {open_ms:>6.0} ms | first frame {first_ms:>6.0} ms | close {close_ms:>5.0} ms"
        );
        totals.push(first_ms);
    }
    // Generous: a fixed multi-second sleep or retry loop would break these.
    assert!(totals[0] < 15_000.0, "first connect took {} ms", totals[0]);
    for (i, t) in totals.iter().enumerate().skip(1) {
        assert!(*t < 10_000.0, "reconnect {i} took {t} ms");
    }
}
