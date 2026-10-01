// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Presence v5 against a real local Linux or macOS Space, through the SDK
//! only (no host pointer, no host input). Opt-in: without
//! `CUA_PRESENCE_LIVE_URL` every test prints why and passes.
//!
//! Env:
//! - `CUA_PRESENCE_LIVE_URL`, `CUA_PRESENCE_LIVE_TOKEN`: the Space.
//! - `CUA_PRESENCE_LIVE_POINTS`: `name=x,y;...` normalized points over the
//!   primary display. Linux: `terminal`, `entry`, `button`, `desktop`,
//!   `edge` (a window's left border). macOS (the test opens its own
//!   TextEdit fixture): `text` (a text view), `entry` (a text field),
//!   `link`, `button`, `desktop`.
//! - `CUA_PRESENCE_LIVE_EXPECT`: `name=shape;...`, the shape the
//!   hit-test must report at each point (defaults per OS).
//! - `CUA_PRESENCE_LIVE_EXPECT_PROBE`: the same for the real cursor the
//!   idle probe reads (defaults per OS: what that OS really draws; macOS
//!   shows the hand over a link, the arrow over a native button).
//! - `CUA_PRESENCE_LIVE_EVIDENCE`: directory for the JSON results.
//!
//! Run with `--ignored --test-threads=1` (the tests share one guest pointer).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use cua_spaces::presence::{
    Cursor, CursorShape, Identity, PresenceEvent, PresenceSession, ShapeSource, now_ms,
};
use cua_spaces::{Space, Spaces};
use serde_json::json;

const T: Duration = Duration::from_secs(10);

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// The guest OS family the Space reports (`linux`, `macos`, `windows`).
async fn guest_os(space: &Space) -> String {
    let caps = space.spacesd().unwrap().capabilities().await.unwrap();
    let os = caps
        .os
        .map(|o| o.family().as_str_name().to_lowercase())
        .unwrap_or_default();
    if os.contains("mac") || os.contains("darwin") {
        "macos".into()
    } else if os.contains("windows") {
        "windows".into()
    } else {
        "linux".into()
    }
}

async fn space() -> Option<(tempfile::TempDir, Space)> {
    cua_spaces_ext::presence_datagrams::register();
    let Some(url) = env("CUA_PRESENCE_LIVE_URL") else {
        eprintln!("skipped: set CUA_PRESENCE_LIVE_URL to a local Space");
        return None;
    };
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let info = spaces
        .add(
            &url,
            env("CUA_PRESENCE_LIVE_TOKEN"),
            Some("cua-e2e-presence-live".into()),
        )
        .await
        .expect("add the Space");
    let space = spaces.space(&info.id).await.unwrap();
    Some((home, space))
}

/// `name=a,b;...` pairs.
fn pairs(raw: &str) -> Vec<(String, String)> {
    raw.split(';')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// One point: its name, where it is, the shape the hit-test must report
/// and, when checked, the real cursor the probe must read there.
struct Case {
    name: String,
    at: (f64, f64),
    hit: String,
    probe: Option<String>,
}

/// The points to probe, in order (consecutive shapes differ, so every point
/// yields a change), and the shapes each must show.
fn cases(os: &str) -> Vec<Case> {
    let (points, hit, probe) = match os {
        // The TextEdit fixture on a 1024x768 display (see `macos_fixture`).
        // The hit-test reads a button as clickable (the hand, a product
        // decision); the real macOS cursor over a native button is the
        // arrow, and the hand only over a link.
        "macos" => (
            "link=0.166,0.263;text=0.2,0.44;desktop=0.84,0.33;entry=0.398,0.104;button=0.095,0.104",
            "link=pointer;text=text;desktop=arrow;entry=text;button=pointer",
            "link=pointer;text=text;desktop=arrow;entry=text;button=arrow",
        ),
        _ => (
            "terminal=0.71875,0.45;entry=0.2578125,0.195;button=0.2578125,0.2625;desktop=0.234375,0.8125;edge=0.0640625,0.3375",
            "terminal=text;entry=text;button=pointer;desktop=arrow;edge=resize_ew",
            "terminal=text",
        ),
    };
    let points = points_of(&env("CUA_PRESENCE_LIVE_POINTS").unwrap_or_else(|| points.into()));
    let map = |var: &str, default: &str| -> BTreeMap<String, String> {
        pairs(&env(var).unwrap_or_else(|| default.into()))
            .into_iter()
            .collect()
    };
    let hit = map("CUA_PRESENCE_LIVE_EXPECT", hit);
    let probe = map("CUA_PRESENCE_LIVE_EXPECT_PROBE", probe);
    points
        .into_iter()
        .filter_map(|(name, at)| {
            Some(Case {
                hit: hit.get(&name)?.clone(),
                probe: probe.get(&name).cloned(),
                name,
                at,
            })
        })
        .collect()
}

fn points_of(raw: &str) -> Vec<(String, (f64, f64))> {
    raw.split(';')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let (x, y) = v.split_once(',')?;
            Some((
                k.trim().into(),
                (x.trim().parse().ok()?, y.trim().parse().ok()?),
            ))
        })
        .collect()
}

fn evidence(name: &str, value: serde_json::Value) {
    println!("{name}: {}", serde_json::to_string_pretty(&value).unwrap());
    if let Some(dir) = env("CUA_PRESENCE_LIVE_EVIDENCE") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            format!("{dir}/{name}.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
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

/// Runs a shell line in the guest; its trimmed stdout.
async fn guest_shell(space: &Space, line: &str) -> String {
    let out = space
        .spacesd()
        .unwrap()
        .run(cua_spacesd_client::Command::shell(line).timeout(Duration::from_secs(20)))
        .await
        .expect("run in the guest");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The guest's real pointer, read inside the guest (`xdotool` on Linux,
/// AppKit's `NSEvent.mouseLocation` on macOS), so the test can prove probes
/// put it back and hit-tests never move it.
async fn guest_pointer(space: &Space, os: &str) -> String {
    match os {
        "macos" => {
            guest_shell(
                space,
                r#"osascript -l JavaScript -e 'ObjC.import("AppKit"); var p = $.NSEvent.mouseLocation; Math.round(p.x) + " " + Math.round(p.y)'"#,
            )
            .await
        }
        _ => {
            guest_shell(
                space,
                "desktop-env xdotool getmouselocation 2>/dev/null | cut -d' ' -f1,2",
            )
            .await
        }
    }
}

/// macOS: a TextEdit document with a text view, a link and the format bar
/// (a text field and buttons), alone and in front, where the default points
/// expect them (TextEdit is restarted in the guest; idempotent).
async fn macos_fixture(space: &Space) {
    let rtf = r#"{\rtf1\ansi\deff0{\fonttbl{\f0 Helvetica;}}
\f0\fs48 Cua presence fixture. Type here.\par
\par
{\field{\*\fldinst{HYPERLINK "https://cua.ai/"}}{\fldrslt Cua website link}}\par
\par
More text for the I-beam.\par
}"#;
    guest_shell(
        space,
        &format!(
            "p=$(pgrep -x TextEdit); [ -z \"$p\" ] || {{ kill $p; sleep 2; }}\n\
             defaults write com.apple.TextEdit NSQuitAlwaysKeepsWindows -bool false\n\
             mkdir -p /tmp/cua-presence-live && cat > /tmp/cua-presence-live/fixture.rtf <<'RTF'\n{rtf}\nRTF\n\
             open -a TextEdit /tmp/cua-presence-live/fixture.rtf && sleep 4"
        ),
    )
    .await;
}

/// Moves `me` to `at` and returns its own shape there: the last
/// `ShapeChanged` for the caller within a settle window, or `previous` when
/// the shape did not change (the server only reports changes).
async fn shape_at(
    s: &mut PresenceSession,
    at: (f64, f64),
    previous: &mut (CursorShape, ShapeSource),
) -> (CursorShape, ShapeSource, Option<f64>) {
    let me = s.me().participant_id.clone();
    let started = Instant::now();
    s.update_cursor(&Cursor::at(at.0, at.1)).await.unwrap();
    let mut first_ms = None;
    // Bounded: 1.5 s and 200 events.
    for _ in 0..200 {
        let left = Duration::from_millis(1_500).saturating_sub(started.elapsed());
        if left.is_zero() {
            break;
        }
        if let Ok(Some(PresenceEvent::ShapeChanged {
            participant_id,
            shape,
            source,
        })) = s.next_event(left).await
            && participant_id == me
        {
            first_ms.get_or_insert(started.elapsed().as_secs_f64() * 1e3);
            *previous = (shape, source);
        }
    }
    (previous.0, previous.1, first_ms)
}

#[tokio::test]
#[ignore]
async fn live_shapes_hit_test_and_probe() {
    let Some((_home, space)) = space().await else {
        return;
    };
    let caps = space.spacesd().unwrap().capabilities().await.unwrap();
    let feature = caps
        .features
        .iter()
        .find(|f| f.name == "presence.cursor_shape")
        .cloned()
        .expect("the Space advertises presence.cursor_shape");
    assert!(feature.supported, "{feature:?}");
    let os = guest_os(&space).await;
    if os == "macos" {
        macos_fixture(&space).await;
    }
    let cases = cases(&os);
    let mut results = serde_json::Map::new();
    results.insert("os".into(), json!(os));
    results.insert(
        "capability".into(),
        json!({"attributes": feature.attributes, "limitation": feature.limitation}),
    );
    let pointer_before = guest_pointer(&space, &os).await;

    // Hit-test only: the guest pointer must never move.
    space.set_presence_settings(Some(false)).await.unwrap();
    let mut s = space
        .join_presence_with(who("live-hit"), T, false)
        .await
        .unwrap();
    let mut hit = serde_json::Map::new();
    let mut prev = (CursorShape::Arrow, ShapeSource::Unspecified);
    for c in &cases {
        let (shape, source, ms) = shape_at(&mut s, c.at, &mut prev).await;
        hit.insert(
            c.name.clone(),
            json!({"shape": shape.as_str(), "source": source, "changed_after_ms": ms}),
        );
    }
    let pointer_after_hit = guest_pointer(&space, &os).await;
    s.leave().await.unwrap();
    results.insert("hit_test".into(), json!(hit));
    assert_eq!(
        pointer_before, pointer_after_hit,
        "the hit-test never moves the guest pointer"
    );
    evidence(&format!("shapes-hit-test-{os}"), json!(hit));
    let shape = |n: &str| hit[n]["shape"].as_str().unwrap_or("none").to_owned();
    for c in &cases {
        assert_eq!(
            shape(&c.name),
            c.hit,
            "{os} {} -> {}: {hit:?}",
            c.name,
            c.hit
        );
    }

    // Idle warp probe (the default): exact shapes, pointer restored.
    space.set_presence_settings(Some(true)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(900)).await;
    let mut s = space
        .join_presence_with(who("live-probe"), T, false)
        .await
        .unwrap();
    let mut probe = serde_json::Map::new();
    let mut prev = (CursorShape::Arrow, ShapeSource::Unspecified);
    for c in cases.iter().filter(|c| c.name != "edge") {
        let (shape, source, ms) = shape_at(&mut s, c.at, &mut prev).await;
        probe.insert(
            c.name.clone(),
            json!({"shape": shape.as_str(), "source": source, "changed_after_ms": ms}),
        );
    }
    // The pointer is restored once the probe settles.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let pointer_after_probe = guest_pointer(&space, &os).await;
    s.leave().await.unwrap();
    results.insert("probe".into(), json!(probe));
    results.insert(
        "guest_pointer".into(),
        json!({"before": pointer_before, "after_hit_test": pointer_after_hit, "after_probe": pointer_after_probe}),
    );
    evidence(&format!("shapes-{os}"), json!(results));
    assert_eq!(
        pointer_before, pointer_after_probe,
        "the probe restores the guest pointer exactly"
    );
    // The real cursor, read by warping the guest pointer: every point shows
    // what the OS draws there.
    for c in &cases {
        let Some(want) = &c.probe else { continue };
        assert_eq!(
            probe[c.name.as_str()]["shape"].as_str(),
            Some(want.as_str()),
            "{os} {}: the real cursor is {want}: {probe:?}",
            c.name
        );
    }
}

fn stats(mut v: Vec<f64>) -> serde_json::Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    json!({"n": v.len(), "mean": v.iter().sum::<f64>() / v.len() as f64,
           "p50": p(0.5), "p95": p(0.95), "max": p(1.0)})
}

/// The true path of the sender: a circle, one turn per 2 s.
fn path(t_ms: f64) -> (f64, f64) {
    let a = t_ms / 2000.0 * std::f64::consts::TAU;
    (0.5 + 0.2 * a.cos(), 0.5 + 0.2 * a.sin())
}

async fn measure(space: &Space, datagrams: bool) -> serde_json::Value {
    let mut observer = space
        .join_presence_with(who("live-observer"), T, datagrams)
        .await
        .unwrap();
    let sender = space
        .join_presence_with(who("live-sender"), T, datagrams)
        .await
        .unwrap();
    let sender_id = sender.me().participant_id.clone();
    let mut view = observer.view();
    let tx = sender.sender();
    let t0 = now_ms();
    let run_ms = 6_000.0;
    // Send at 120 Hz (the sender throttles to 30 Hz newest-wins), and record
    // what was sent when, keyed by quantized position.
    let sent: std::sync::Arc<std::sync::Mutex<Vec<(f64, f64, f64)>>> = Default::default();
    let sent_w = sent.clone();
    let producer = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_micros(8_333));
        loop {
            tick.tick().await;
            let t = now_ms() - t0;
            if t > run_ms {
                break;
            }
            let (x, y) = path(t);
            sent_w.lock().unwrap().push((now_ms(), x, y));
            let _ = tx.update(&Cursor::at(x, y)).await;
        }
    });
    let mut latencies = vec![];
    let mut arrivals = vec![];
    let mut frames: Vec<(f64, f64, f64)> = vec![];
    let mut next_frame = now_ms();
    let deadline = Instant::now() + Duration::from_millis(run_ms as u64 + 1_500);
    for _ in 0..100_000 {
        if Instant::now() > deadline {
            break;
        }
        if let Ok(Some(e)) = observer.next_event(Duration::from_millis(4)).await {
            let now = now_ms();
            view.apply(&e, now);
            if let PresenceEvent::CursorMoved {
                participant_id,
                cursor,
            } = &e
                && *participant_id == sender_id
            {
                arrivals.push(now);
                let sent = sent.lock().unwrap();
                // The newest sent sample at this position (quantized on
                // datagrams): latency = arrival - send time.
                if let Some((at, _, _)) = sent
                    .iter()
                    .rev()
                    .find(|(_, x, y)| (x - cursor.x).abs() < 2e-5 && (y - cursor.y).abs() < 2e-5)
                {
                    latencies.push(now - at);
                }
            }
        }
        let now = now_ms();
        if now >= next_frame {
            next_frame = now + 8.0;
            if let Some(d) = view
                .drawables(now, None)
                .into_iter()
                .find(|d| d.participant_id == sender_id)
            {
                frames.push((now - t0, d.x, d.y));
            }
        }
    }
    producer.await.unwrap();
    let gaps: Vec<f64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
    // Visual lag: the delay tau that best explains the rendered path, and the
    // residual error there (in pixels of a 1280 px wide display).
    let steady: Vec<_> = frames
        .iter()
        .filter(|(t, _, _)| *t > 500.0 && *t < run_ms)
        .collect();
    let err_at = |tau: f64| {
        steady
            .iter()
            .map(|(t, x, y)| {
                let (px, py) = path(t - tau);
                ((x - px).powi(2) + (y - py).powi(2)).sqrt() * 1280.0
            })
            .collect::<Vec<_>>()
    };
    let (tau, _) = (0..=150)
        .map(|i| f64::from(i) * 2.0)
        .map(|tau| (tau, err_at(tau).iter().sum::<f64>()))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap_or((0.0, 0.0));
    let jumps: Vec<f64> = steady
        .windows(2)
        .map(|w| ((w[1].1 - w[0].1).powi(2) + (w[1].2 - w[0].2).powi(2)).sqrt() * 1280.0)
        .collect();
    let result = json!({
        "transport": if observer.uses_datagrams() { "quic-datagrams" } else { "grpc-stream" },
        "sender_uses_datagrams": sender.uses_datagrams(),
        "network_latency_ms": stats(latencies),
        "inter_arrival_ms": stats(gaps),
        "render_lag_ms": tau,
        "render_error_px_at_lag": stats(err_at(tau)),
        "frame_step_px": stats(jumps),
        "frames": steady.len(),
    });
    sender.leave().await.unwrap();
    observer.leave().await.unwrap();
    result
}

#[tokio::test]
#[ignore]
async fn live_cursor_latency_and_smoothness() {
    let Some((_home, space)) = space().await else {
        return;
    };
    // Measure cursors only: no probes moving the guest pointer meanwhile.
    space.set_presence_settings(Some(false)).await.unwrap();
    let stream = measure(&space, false).await;
    let datagrams = measure(&space, true).await;
    space.set_presence_settings(Some(true)).await.unwrap();
    evidence("latency", json!({"stream": stream, "datagrams": datagrams}));
    assert_eq!(datagrams["transport"], "quic-datagrams");
    assert!(stream["network_latency_ms"]["p50"].as_f64().unwrap() < 100.0);
}

#[tokio::test]
#[ignore]
async fn live_no_stale_cursors_after_a_run_ends() {
    let Some((_home, space)) = space().await else {
        return;
    };
    let url = env("CUA_PRESENCE_LIVE_URL").unwrap();
    let token = env("CUA_PRESENCE_LIVE_TOKEN").unwrap_or_default();
    let mut observer = space
        .join_presence_with(who("live-stale"), T, false)
        .await
        .unwrap();
    let mut view = observer.view();
    let run = format!("run-live{:08x}", now_ms() as u64 & 0xffff_ffff);
    let http = reqwest::Client::new();
    let mcp = format!("{}/mcp", url.trim_end_matches('/'));
    let call = |id: u32, method: &str, params: serde_json::Value| {
        http.post(&mcp)
            .bearer_auth(&token)
            .header("X-Cua-Agent-Session", &run)
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .send()
    };
    // An agent run acts through /mcp the way cua agent runs do.
    let init = call(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {},
               "clientInfo": {"name": "cua-e2e-presence-live", "version": "0"}}),
    )
    .await
    .unwrap();
    assert!(init.status().is_success(), "initialize: {}", init.status());
    let moved = call(
        2,
        "tools/call",
        json!({"name": "move_cursor", "arguments": {"x": 300, "y": 600}}),
    )
    .await
    .unwrap();
    assert!(
        moved.status().is_success(),
        "move_cursor: {}",
        moved.status()
    );
    let _ = moved.text().await;
    // The observer sees the run's agent cursor.
    let started = Instant::now();
    let mut agent = None;
    for _ in 0..500 {
        let Ok(Some(e)) = observer.next_event(Duration::from_secs(1)).await else {
            if started.elapsed() > T {
                break;
            }
            continue;
        };
        view.apply(&e, now_ms());
        if let PresenceEvent::Joined { participant } = &e
            && participant.kind == "agent"
        {
            eprintln!("agent joined: {participant:?}");
            agent = Some(participant.participant_id.clone());
            break;
        }
        if started.elapsed() > T {
            break;
        }
    }
    let agent = agent.expect("the agent cursor joined presence");
    // The run ends: the runner sends DELETE /mcp with its agent session.
    let ended_at = Instant::now();
    let del = http
        .delete(&mcp)
        .bearer_auth(&token)
        .header("X-Cua-Agent-Session", &run)
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success(), "DELETE /mcp: {}", del.status());
    let mut left = None;
    for _ in 0..500 {
        let Ok(Some(e)) = observer.next_event(Duration::from_secs(1)).await else {
            if ended_at.elapsed() > T {
                break;
            }
            continue;
        };
        view.apply(&e, now_ms());
        if let PresenceEvent::Left {
            participant_id,
            reason,
        } = &e
            && *participant_id == agent
        {
            left = Some((reason.clone(), ended_at.elapsed().as_secs_f64() * 1e3));
            break;
        }
        if ended_at.elapsed() > T {
            break;
        }
    }
    let (reason, ms) = left.expect("the run's cursor left");
    let remaining: Vec<String> = view
        .drawables(now_ms(), None)
        .into_iter()
        .map(|d| d.participant_id)
        .filter(|id| *id != observer.me().participant_id)
        .collect();
    evidence(
        "stale",
        json!({"agent": agent, "leave_reason": reason, "left_after_ms": ms, "cursors_remaining_besides_me": remaining}),
    );
    assert_eq!(reason, "run_ended");
    assert!(remaining.is_empty(), "no stale cursors: {remaining:?}");
    observer.leave().await.unwrap();
}

/// Another participant tours the points (`CUA_PRESENCE_LIVE_DWELL_MS` at
/// each, default 0) so a viewer in an app can watch its shaped cursor; each
/// point's shape is checked as it goes. `CUA_PRESENCE_LIVE_NAME` names it.
#[tokio::test]
#[ignore]
async fn live_shaped_cursor_tour() {
    let Some((_home, space)) = space().await else {
        return;
    };
    let os = guest_os(&space).await;
    let dwell = Duration::from_millis(
        env("CUA_PRESENCE_LIVE_DWELL_MS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    );
    let name = env("CUA_PRESENCE_LIVE_NAME").unwrap_or_else(|| "Alex".into());
    let mut s = space
        .join_presence_with(
            Identity {
                id: format!("live-tour-{}", name.to_lowercase()),
                display_name: name,
                color: "#0A84FF".into(),
                agent: false,
            },
            T,
            false,
        )
        .await
        .unwrap();
    let mut prev = (CursorShape::Arrow, ShapeSource::Unspecified);
    let mut tour = serde_json::Map::new();
    for c in cases(&os).iter().filter(|c| c.name != "edge") {
        let (shape, source, _) = shape_at(&mut s, c.at, &mut prev).await;
        eprintln!("tour {} {} ({source:?})", c.name, shape.as_str());
        tour.insert(
            c.name.clone(),
            json!({"shape": shape.as_str(), "source": source}),
        );
        // Keep the cursor there (heartbeats) while a viewer looks.
        let until = Instant::now() + dwell;
        while Instant::now() < until {
            s.update_cursor(&Cursor::at(c.at.0, c.at.1)).await.unwrap();
            let _ = s.next_event(Duration::from_millis(500)).await;
        }
    }
    s.leave().await.unwrap();
    evidence(&format!("tour-{os}"), json!(tour));
    for c in cases(&os).iter().filter(|c| c.name != "edge") {
        let got = tour[c.name.as_str()]["shape"].as_str().unwrap_or("none");
        assert!(
            got == c.hit || c.probe.as_deref() == Some(got),
            "{os} {}: {got}, want {} (hit-test) or {:?} (probe)",
            c.name,
            c.hit,
            c.probe
        );
    }
}
