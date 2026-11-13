// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform semantic cursor showcase for release evidence.

use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine as _;
use cua_driver_testkit::e2e::{
    execute_case, recording_evidence, CaseSpec, Delivery, DriverRoute, Evidence, Observation,
    OracleKind, Scope, Targeting,
};
use cua_driver_testkit::{ax, Driver, McpDriver, ToolResponse};
use cursor_overlay::{BADGE_CURSOR_GAP, BADGE_HEIGHT, BADGE_MAX_WIDTH};
use image::RgbaImage;

const CELL_ID: &str = "desktop-agent-cursor-showcase-px";
const SESSION: &str = "Cursor showcase";
// MoveTo anchors the artwork `POINTER_ANCHOR_OFFSET` points from the requested
// coordinate at 45 degrees so the cursor tip lands on it. Each axis moves by
// that offset over sqrt(2), and the session badge follows the anchor.
const CURSOR_ANCHOR_OFFSET_PER_AXIS: f64 =
    cursor_overlay::POINTER_ANCHOR_OFFSET * std::f64::consts::FRAC_1_SQRT_2;
const POINTER_ORACLE_RADIUS: f64 = 24.0;
const BADGE_CURSOR_EXCLUSION: f64 = 34.0;
/// Fewest changed pixels that count as the pointer, and as the badge, in their
/// oracle regions. A resting pointer changes about 200 pixels and the smallest
/// badge (its chips alone) several hundred, so neither threshold is close.
const MIN_POINTER_PIXELS: usize = 12;
const MIN_BADGE_PIXELS: usize = 24;
/// Least chroma (max minus min channel) an overlay pixel has. Every session
/// colour's pointer fill is above 110 and its badge's outer wings above 40;
/// greys, black and white are 0.
const MIN_OVERLAY_CHROMA: u8 = 32;

#[test]
#[ignore]
fn semantic_cursor_showcase_records_session_and_action_states() {
    let case = CaseSpec::delivered(
        CELL_ID,
        "desktop",
        platform_toolkit(),
        "agent_cursor_showcase",
        Targeting::Px,
        Delivery::Foreground,
        Scope::Desktop,
        platform_route(),
        vec![OracleKind::Pixels],
    );
    execute_case(case, |evidence| {
        let mut driver = spawn_driver();
        *evidence = recording_evidence(driver.recording_dir());

        // Capture the empty desktop before declaring the explicit session.
        // `start_session` may revive and materialize the session-owned overlay
        // at the current pointer position, which can otherwise put the badge
        // in both frames when a previous showcase left the pointer at this
        // deterministic target.
        let (baseline_png, width, height) = capture_desktop_png(&mut driver);
        let baseline = image::load_from_memory(&baseline_png)
            .expect("decode baseline desktop screenshot")
            .to_rgba8();
        assert!(
            width >= 640.0 && height >= 480.0,
            "showcase requires a normal desktop, got {width}x{height}"
        );

        // The showcase asserts the session badge at rest, which only the
        // verbose badge mode draws; minimal (the default) shows no badge
        // until a foreground moment or a key combo.
        call_ok(
            &mut driver,
            "set_config",
            serde_json::json!({ "cursor.badges": "verbose" }),
        );

        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({
                "session": SESSION
            }),
        );

        call_ok(
            &mut driver,
            "set_agent_cursor_enabled",
            serde_json::json!({
                "session": SESSION,
                "enabled": true
            }),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_motion",
            serde_json::json!({
                "session": SESSION,
                "glide_duration_ms": 420,
                "idle_hide_ms": 0
            }),
        );

        let center_x = width * 0.55;
        let center_y = height * 0.45;
        driver.start_behavior_recording();

        call_ok(
            &mut driver,
            "move_cursor",
            serde_json::json!({
                "session": SESSION,
                "x": center_x - 180.0,
                "y": center_y - 80.0
            }),
        );
        // move_cursor waits for the configured glide, but X11/Wayland capture
        // still needs a compositor round-trip before the overlay is guaranteed
        // to appear in the driver-owned screenshot. The badge remains fully
        // visible for two seconds, so this settle stays inside that window.
        settle(900);

        let cursor_png = capture_cursor_oracle_png(&mut driver, width, height);
        let cursor_frame = image::load_from_memory(&cursor_png)
            .expect("decode cursor desktop screenshot")
            .to_rgba8();
        assert_cursor_and_badge_pixels_changed(
            &baseline,
            &cursor_frame,
            center_x - 180.0,
            center_y - 80.0,
            width,
            height,
        );
        let screenshot_path = driver
            .recording_dir()
            .expect("showcase recording directory")
            .join("cursor-oracle.png");
        std::fs::write(&screenshot_path, cursor_png).expect("write cursor oracle screenshot");
        evidence.screenshot = Some(screenshot_path.display().to_string());

        // The cursor and pill keep resting there, yet the capture the Driver
        // hands the agent must not contain them (D-WL5).
        assert_driver_capture_excludes_overlay(
            &mut driver,
            &baseline,
            &cursor_frame,
            center_x - 180.0,
            center_y - 80.0,
            width,
            height,
        );

        call_ok(
            &mut driver,
            "click",
            serde_json::json!({
                "session": SESSION,
                "target": {"kind": "desktop", "display_id": "primary"},
                "x": center_x,
                "y": center_y,
                "delivery_mode": "foreground"
            }),
        );
        settle(900);

        let typed = driver.call(
            "type_text",
            serde_json::json!({
                "session": SESSION,
                "target": {"kind": "desktop", "display_id": "primary"},
                "text": "cua",
                "delivery_mode": "foreground"
            }),
        );
        if niri_session() {
            // The click landed on the empty desktop, so no window has
            // keyboard focus. niri reports that and types nothing, where
            // other compositors accept text that reaches no one.
            assert!(
                typed.is_error()
                    && typed
                        .text()
                        .starts_with("foreground_unavailable: no niri window has keyboard focus"),
                "type_text on the empty niri desktop did not refuse exactly: {}",
                typed.text()
            );
        } else {
            assert!(!typed.is_error(), "type_text failed: {}", typed.text());
        }
        settle(900);

        call_ok(
            &mut driver,
            "scroll",
            serde_json::json!({
                "session": SESSION,
                "target": {"kind": "desktop", "display_id": "primary"},
                "x": center_x,
                "y": center_y,
                "direction": "down",
                "amount": 4,
                "delivery_mode": "foreground"
            }),
        );
        settle(900);

        call_ok(
            &mut driver,
            "drag",
            serde_json::json!({
                "session": SESSION,
                "target": {"kind": "desktop", "display_id": "primary"},
                "from_x": center_x - 90.0,
                "from_y": center_y + 80.0,
                "to_x": center_x + 120.0,
                "to_y": center_y + 20.0,
                "duration_ms": 700,
                "steps": 28,
                "delivery_mode": "foreground"
            }),
        );
        settle(1_100);

        Observation::delivered(vec![OracleKind::Pixels], Evidence::default())
    });
}

const FIRST_ACTIONS_CELL_ID: &str = "window-agent-cursor-first-actions-video";
/// Rate the behavioral video is sampled at for the overlay oracle.
const VIDEO_SAMPLE_FPS: u32 = 10;
/// Consecutive sampled frames (half a second) the cursor must rest at one
/// place to count. A target repainting under a press, or the real pointer
/// passing through, does not last that long.
const MIN_REST_FRAMES: usize = 5;
/// Time each action leaves the cursor at rest before the next one moves it.
/// The keyboard action's pointer and badge must show within this window.
const ACTION_DWELL_MS: u64 = 1_500;

/// The places a session's first actions send its cursor, in action order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstAction {
    Keyboard,
    Semantic,
    Coordinate,
}

impl FirstAction {
    const ORDER: [Self; 3] = [Self::Keyboard, Self::Semantic, Self::Coordinate];

    fn name(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard press_key",
            Self::Semantic => "semantic element_token click",
            Self::Coordinate => "coordinate click",
        }
    }

    fn evidence_file(self) -> &'static str {
        match self {
            Self::Keyboard => "cursor-oracle-keyboard.png",
            Self::Semantic => "cursor-oracle-semantic.png",
            Self::Coordinate => "cursor-oracle-coordinate.png",
        }
    }

    /// The recording starts on the keyboard action, so its cursor reveal is
    /// the startup frame that must carry the session badge as well.
    fn needs_badge(self) -> bool {
        self == Self::Keyboard
    }
}

/// A declared session's first recorded actions, a keyboard action, a semantic
/// accessibility action and a coordinate action, each show its agent cursor
/// in the recorded MP4, and the first one shows the session badge with it.
/// No `move_cursor` comes first: the keyboard action is the cursor's first
/// placement.
#[test]
#[ignore]
fn session_first_actions_show_pointer_and_badge_in_the_video() {
    let fixture = native_fixture();
    let case = CaseSpec::delivered(
        FIRST_ACTIONS_CELL_ID,
        fixture.toolkit,
        fixture.toolkit,
        "first_actions_cursor_video",
        Targeting::NotApplicable,
        Delivery::Foreground,
        Scope::Window,
        platform_route(),
        vec![OracleKind::Pixels],
    );
    execute_case(case, |evidence| {
        let mut driver = spawn_driver_named(FIRST_ACTIONS_CELL_ID);
        *evidence = recording_evidence(driver.recording_dir());
        let recording_dir = driver
            .recording_dir()
            .map(Path::to_path_buf)
            .expect("the first-actions row reads its MP4: set CUA_E2E_RECORDINGS_ROOT");
        let (pid, window_id) = launch_native_fixture(&mut driver, &fixture);
        // A fixture launched after another one in the same process may not
        // hold the foreground yet (Windows then stalls foreground input), so
        // raise it once before any session action, as the WPF suite does.
        let _ = driver.call(
            "bring_to_front",
            serde_json::json!({"pid": pid, "window_id": window_id}),
        );
        settle(300);
        let (_, screen_width, screen_height) = capture_desktop_png(&mut driver);

        // Verbose badges show the session name when the cursor is revealed.
        call_ok(
            &mut driver,
            "set_config",
            serde_json::json!({ "cursor.badges": "verbose" }),
        );
        let session = format!("First actions cursor {}", std::process::id());
        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({"session": session}),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_enabled",
            serde_json::json!({"session": session, "enabled": true}),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_motion",
            serde_json::json!({"session": session, "glide_duration_ms": 420, "idle_hide_ms": 0}),
        );
        let before = agent_cursor_state(&mut driver, &session);
        assert!(
            before["position"].is_null(),
            "the session cursor must start unplaced so the recorded actions reveal it: {before}"
        );

        driver.start_behavior_recording();
        call_ok(
            &mut driver,
            "press_key",
            serde_json::json!({
                "session": session,
                "pid": pid,
                "window_id": window_id,
                "key": "escape",
                "delivery_mode": "foreground"
            }),
        );
        settle(ACTION_DWELL_MS);
        let keyboard_point = cursor_position(&agent_cursor_state(&mut driver, &session));

        let state = driver.call(
            "get_window_state",
            serde_json::json!({"session": session, "pid": pid, "window_id": window_id}),
        );
        assert!(
            !state.is_error(),
            "get_window_state failed: {}",
            state.text()
        );
        let (token, semantic_point) = element_target(&state, fixture.click_target);
        // Background delivery keeps this click on the accessibility route
        // (UIA Invoke, AXPress, AT-SPI action) on every platform.
        call_ok(
            &mut driver,
            "click",
            serde_json::json!({
                "session": session,
                "pid": pid,
                "window_id": window_id,
                "element_token": token,
                "delivery_mode": "background"
            }),
        );
        settle(ACTION_DWELL_MS);

        let ((x, y), coordinate_point) = empty_window_point(
            &state,
            (screen_width, screen_height),
            &[keyboard_point, semantic_point],
        );
        call_ok(
            &mut driver,
            "click",
            serde_json::json!({
                "session": session,
                "pid": pid,
                "window_id": window_id,
                "x": x,
                "y": y,
                "delivery_mode": "foreground"
            }),
        );
        settle(ACTION_DWELL_MS);

        // Park the cursor away from all three places before ending the
        // session. macOS ScreenCaptureKit can drop the last two seconds of a
        // clip, so the coordinate rest must not be the end of the video, and
        // its last frame, the baseline, must not show the cursor at a place
        // the oracle checks.
        let places = [keyboard_point, semantic_point, coordinate_point];
        let parking = parking_point((screen_width, screen_height), &places);
        call_ok(
            &mut driver,
            "move_cursor",
            serde_json::json!({"session": session, "x": parking.0, "y": parking.1}),
        );
        settle(ACTION_DWELL_MS);
        call_ok(
            &mut driver,
            "end_session",
            serde_json::json!({"session": session}),
        );
        settle(700);
        let video = driver.finish_behavior_recording().unwrap_or_else(|| {
            panic!(
                "the behavioral video did not finalize: {}",
                std::fs::read_to_string(recording_dir.join("recording-error.txt"))
                    .unwrap_or_default()
            )
        });

        for (index, a) in places.iter().enumerate() {
            for b in &places[index + 1..] {
                assert!(
                    !overlay_footprints_overlap(*a, *b),
                    "first-action places {places:?} overlap, so one cursor could count for two"
                );
            }
        }
        let rests = assert_first_actions_in_video(
            &video,
            &recording_dir,
            (screen_width, screen_height),
            places,
        );
        evidence.screenshot = Some(
            recording_dir
                .join(FirstAction::Keyboard.evidence_file())
                .display()
                .to_string(),
        );
        eprintln!("[first-actions] cursor rests start at sampled frames {rests:?}");
        Observation::delivered(vec![OracleKind::Pixels], Evidence::default())
    });
}

fn cursor_position(state: &serde_json::Value) -> (f64, f64) {
    let (Some(x), Some(y)) = (
        state["position"]["x"].as_f64(),
        state["position"]["y"].as_f64(),
    ) else {
        panic!("the keyboard action left the session cursor unplaced: {state}");
    };
    (x, y)
}

/// The element token of `identifier` and the screen centre of its frame,
/// where a semantic click sends the cursor.
fn element_target(state: &ToolResponse, identifier: &str) -> (String, (f64, f64)) {
    let index = ax::element_index_by_id(state.tree_text(), identifier)
        .or_else(|| ax::element_index_containing(state.tree_text(), identifier))
        .unwrap_or_else(|| panic!("{identifier} is not in the tree: {}", state.tree_text()));
    let element = state.structured()["elements"]
        .as_array()
        .and_then(|elements| {
            elements
                .iter()
                .find(|element| element["element_index"].as_u64() == Some(index))
        })
        .unwrap_or_else(|| panic!("{identifier} [{index}] has no structured element"));
    let frame = &element["frame"];
    let (Some(x), Some(y), Some(width), Some(height)) = (
        frame["x"].as_f64(),
        frame["y"].as_f64(),
        frame["w"].as_f64(),
        frame["h"].as_f64(),
    ) else {
        panic!("{identifier} has no screen frame: {element}");
    };
    (
        state.element_token(index).to_owned(),
        (x + width / 2.0, y + height / 2.0),
    )
}

/// A screen point whose overlay footprint misses those of `places` and stays
/// on screen.
fn parking_point((width, height): (f64, f64), places: &[(f64, f64)]) -> (f64, f64) {
    [
        (0.25, 0.25),
        (0.75, 0.25),
        (0.25, 0.7),
        (0.75, 0.7),
        (0.5, 0.25),
        (0.5, 0.7),
    ]
    .into_iter()
    .map(|(x, y)| (width * x, height * y))
    .find(|point| {
        places
            .iter()
            .all(|place| !overlay_footprints_overlap(*place, *point))
    })
    .unwrap_or_else(|| panic!("no parking point clear of {places:?} on a {width}x{height} screen"))
}

/// Whether a cursor resting at `a` and one resting at `b` (screen points)
/// could draw into each other's oracle regions: each footprint covers the
/// pointer square and the badge band below it, with the badge at full width.
fn overlay_footprints_overlap(a: (f64, f64), b: (f64, f64)) -> bool {
    let footprint = |(x, y): (f64, f64)| {
        let (anchor_x, anchor_y) = (
            x + CURSOR_ANCHOR_OFFSET_PER_AXIS,
            y + CURSOR_ANCHOR_OFFSET_PER_AXIS,
        );
        let half_width = POINTER_ORACLE_RADIUS.max(f64::from(BADGE_MAX_WIDTH) * 0.5);
        [
            anchor_x - half_width,
            anchor_y - POINTER_ORACLE_RADIUS,
            anchor_x + half_width,
            anchor_y + f64::from(BADGE_CURSOR_GAP + BADGE_HEIGHT),
        ]
    };
    let ([ax0, ay0, ax1, ay1], [bx0, by0, bx1, by1]) = (footprint(a), footprint(b));
    ax0 < bx1 && bx0 < ax1 && ay0 < by1 && by0 < ay1
}

/// A point of the window's screenshot that no control covers, whose overlay
/// footprint misses those of the points in `avoid`, and the screen point it
/// maps to. Clicking it changes nothing the oracle could mistake for a cursor.
///
/// The point is the candidate nearest the window centre that stays clear of
/// the screen's edges, where the menu bar, notification banners, the Dock and
/// the taskbar sit; none of those is in the fixture's tree, and clicking one
/// opens another app over the fixture.
fn empty_window_point(
    state: &ToolResponse,
    (screen_width, screen_height): (f64, f64),
    avoid: &[(f64, f64)],
) -> ((f64, f64), (f64, f64)) {
    let structured = state.structured();
    let bounds = &structured["window_bounds"];
    let (Some(shot_width), Some(shot_height), Some(x), Some(y), Some(width), Some(height)) = (
        structured["screenshot_width"].as_f64(),
        structured["screenshot_height"].as_f64(),
        bounds["x"].as_f64(),
        bounds["y"].as_f64(),
        bounds["width"].as_f64(),
        bounds["height"].as_f64(),
    ) else {
        panic!("get_window_state reported no screenshot or window bounds: {structured}");
    };
    // Controls, not the containers (window, document, scroll panes) that
    // hold them.
    let controls: Vec<[f64; 4]> = structured["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|element| {
            let frame = &element["frame"];
            Some([
                frame["x"].as_f64()?,
                frame["y"].as_f64()?,
                frame["w"].as_f64()?,
                frame["h"].as_f64()?,
            ])
        })
        .filter(|[_, _, w, h]| w * h < width * height / 4.0)
        .collect();
    let margin = 8.0;
    let centre = (x + width / 2.0, y + height / 2.0);
    let mut candidates: Vec<((f64, f64), (f64, f64))> = (1..20)
        .flat_map(|column| (1..20).map(move |row| (column, row)))
        .map(|(column, row)| {
            let pixel = (
                (shot_width * f64::from(column) / 20.0).round(),
                (shot_height * f64::from(row) / 20.0).round(),
            );
            let screen = (
                x + pixel.0 * width / shot_width,
                y + pixel.1 * height / shot_height,
            );
            (pixel, screen)
        })
        .filter(|(_, (sx, sy))| {
            *sx > screen_width * 0.15
                && *sx < screen_width * 0.85
                && *sy > screen_height * 0.2
                && *sy < screen_height * 0.8
                && controls.iter().all(|[cx, cy, cw, ch]| {
                    *sx < cx - margin
                        || *sx > cx + cw + margin
                        || *sy < cy - margin
                        || *sy > cy + ch + margin
                })
                && avoid
                    .iter()
                    .all(|place| !overlay_footprints_overlap(*place, (*sx, *sy)))
        })
        .collect();
    let distance = |(sx, sy): (f64, f64)| (sx - centre.0).hypot(sy - centre.1);
    candidates.sort_by(|left, right| distance(left.1).total_cmp(&distance(right.1)));
    candidates
        .first()
        .copied()
        .unwrap_or_else(|| panic!("no empty point in the fixture window: {structured}"))
}

/// Read the recorded MP4 and require the cursor to rest at each first-action
/// place in order, with the keyboard action's badge. Writes the baseline and
/// the first frame of each rest next to the video, and returns those frames'
/// indices.
fn assert_first_actions_in_video(
    video: &Path,
    recording_dir: &Path,
    (screen_width, screen_height): (f64, f64),
    places: [(f64, f64); 3],
) -> Vec<usize> {
    let baseline_path = recording_dir.join("cursor-oracle-baseline.png");
    extract_last_frame(video, &baseline_path);
    let baseline = image::open(&baseline_path)
        .expect("decode the video's last frame")
        .to_rgba8();
    let regions = places.map(|(x, y)| {
        cursor_oracle_regions(
            baseline.width(),
            baseline.height(),
            x,
            y,
            screen_width,
            screen_height,
        )
    });
    let mut frames = Vec::new();
    for_each_video_frame(video, baseline.dimensions(), |_, frame| {
        frames.push(regions.map(|region| overlay_pixels(&baseline, frame, &region)));
    });
    let rests = first_action_rests(&frames).unwrap_or_else(|failure| {
        panic!(
            "{failure}; {} sampled frames at {VIDEO_SAMPLE_FPS} fps, places {places:?} in a \
             {screen_width}x{screen_height} screen, video {}x{}",
            frames.len(),
            baseline.width(),
            baseline.height(),
        )
    });
    for_each_video_frame(video, baseline.dimensions(), |index, frame| {
        for (action, rest) in FirstAction::ORDER.iter().zip(&rests) {
            if *rest == index {
                frame
                    .save(recording_dir.join(action.evidence_file()))
                    .expect("write a cursor oracle frame");
            }
        }
    });
    rests
}

/// The first sampled frame of the cursor's first rest at each place, in
/// action order. A rest is [`MIN_REST_FRAMES`] frames with the pointer at
/// that place (and its badge, for the keyboard action), and each rest must
/// start after the previous one.
fn first_action_rests(frames: &[[OverlayPixels; 3]]) -> Result<Vec<usize>, String> {
    let mut rests: Vec<usize> = Vec::new();
    for (place, action) in FirstAction::ORDER.into_iter().enumerate() {
        let resting = |pixels: &[OverlayPixels; 3]| {
            pixels[place].pointer_visible()
                && (!action.needs_badge() || pixels[place].badge_visible())
        };
        let Some(start) = frames
            .windows(MIN_REST_FRAMES)
            .position(|run| run.iter().all(resting))
        else {
            let count = |test: fn(OverlayPixels) -> bool| {
                frames.iter().filter(|pixels| test(pixels[place])).count()
            };
            return Err(format!(
                "the {} never showed its cursor for {MIN_REST_FRAMES} frames: the pointer \
                 showed in {} frames and the badge in {}, both in {}",
                action.name(),
                count(OverlayPixels::pointer_visible),
                count(OverlayPixels::badge_visible),
                count(|pixels| pixels.pointer_visible() && pixels.badge_visible()),
            ));
        };
        if let Some(&previous) = rests.last() {
            if start <= previous {
                return Err(format!(
                    "the {} cursor rested from frame {start}, not after the {} cursor \
                     (frame {previous})",
                    action.name(),
                    FirstAction::ORDER[place - 1].name(),
                ));
            }
        }
        rests.push(start);
    }
    Ok(rests)
}

/// Decode the video's final frame to `frame`, the overlay oracle's baseline.
fn extract_last_frame(video: &Path, frame: &Path) {
    // A VFR recording's final frame can precede its duration by more than a
    // seek offset. Decode in order and overwrite one image to keep the last.
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
        .arg(video)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "image2",
            "-update",
            "1",
        ])
        .arg(frame)
        .stdout(Stdio::null())
        .output()
        .expect("ffmpeg is required to read the behavioral video");
    assert!(
        output.status.success() && frame.is_file(),
        "could not extract the video's last frame: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Visit the video's frames, sampled at [`VIDEO_SAMPLE_FPS`], in order.
fn for_each_video_frame(
    video: &Path,
    (width, height): (u32, u32),
    mut visit: impl FnMut(usize, &RgbaImage),
) {
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(video)
        .args(["-map", "0:v:0", "-vf"])
        .arg(format!("fps={VIDEO_SAMPLE_FPS}"))
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ffmpeg is required to read the behavioral video");
    let mut stdout = child.stdout.take().expect("ffmpeg stdout");
    let mut bytes = vec![0; width as usize * height as usize * 4];
    let mut index = 0;
    while stdout.read_exact(&mut bytes).is_ok() {
        let frame = RgbaImage::from_raw(width, height, std::mem::take(&mut bytes))
            .expect("a whole RGBA frame");
        visit(index, &frame);
        bytes = frame.into_raw();
        index += 1;
    }
    let output = child.wait_with_output().expect("wait for ffmpeg");
    assert!(
        output.status.success() && index > 0,
        "could not decode the behavioral video ({index} frames): {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

const KEYBOARD_FIRST_CELL_ID: &str = "window-agent-cursor-keyboard-first-placement";
/// Largest distance, per axis and in screen units, between the reported cursor
/// point and the centre of the window's `list_windows` bounds. Each platform
/// reads the centre from its own native geometry (Windows `GetWindowRect`
/// includes the invisible resize border that the DWM frame bounds leave out),
/// so the two can differ by a few units; a cursor left at the pointer, at a
/// stale session point, or unplaced is far outside it.
const WINDOW_CENTRE_TOLERANCE: f64 = 12.0;

/// A named session whose first action is an untargeted `press_key` shows its
/// agent cursor on the target window before the key is delivered: no element,
/// no pixel target, and no remembered position leave the shared keyboard
/// placement policy with the window centre.
#[test]
#[ignore]
fn keyboard_first_session_places_its_cursor_on_the_target_window() {
    let fixture = native_fixture();
    let case = CaseSpec::delivered(
        KEYBOARD_FIRST_CELL_ID,
        fixture.toolkit,
        fixture.toolkit,
        "press_key_cursor_placement",
        Targeting::NotApplicable,
        Delivery::Foreground,
        Scope::Window,
        platform_route(),
        vec![OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        let mut driver = spawn_driver_named(KEYBOARD_FIRST_CELL_ID);
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, window_id) = launch_native_fixture(&mut driver, &fixture);

        // A per-run name keeps a long-lived daemon from handing this row a
        // cursor position that an earlier run left behind.
        let session = format!("Keyboard-first cursor {}", std::process::id());
        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({"session": session}),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_enabled",
            serde_json::json!({"session": session, "enabled": true}),
        );
        let before = agent_cursor_state(&mut driver, &session);
        assert!(
            before["position"].is_null(),
            "the session cursor must start unplaced so press_key is its first placement: {before}"
        );

        driver.start_behavior_recording();
        call_ok(
            &mut driver,
            "press_key",
            serde_json::json!({
                "session": session,
                "pid": pid,
                "window_id": window_id,
                "key": "escape",
                "delivery_mode": "foreground"
            }),
        );

        let after = agent_cursor_state(&mut driver, &session);
        assert_eq!(
            after["enabled"], true,
            "keyboard-first cursor is hidden: {after}"
        );
        let (Some(x), Some(y)) = (
            after["position"]["x"].as_f64(),
            after["position"]["y"].as_f64(),
        ) else {
            panic!("a keyboard-first press_key left the session cursor unplaced: {after}");
        };
        let (centre_x, centre_y) = window_centre(&mut driver, pid, window_id);
        assert!(
            (x - centre_x).abs() <= WINDOW_CENTRE_TOLERANCE
                && (y - centre_y).abs() <= WINDOW_CENTRE_TOLERANCE,
            "keyboard-first cursor landed at ({x:.1}, {y:.1}), expected the target window \
             centre ({centre_x:.1}, {centre_y:.1}) within {WINDOW_CENTRE_TOLERANCE}: {after}"
        );

        call_ok(
            &mut driver,
            "end_session",
            serde_json::json!({"session": session}),
        );
        Observation::delivered(vec![OracleKind::Protocol], Evidence::default())
    });
}

#[cfg(not(target_os = "linux"))]
const DESKTOP_KEYBOARD_FIRST_CELL_ID: &str = "desktop-agent-cursor-keyboard-first-placement";

/// A named session whose first action is a desktop-scope `press_key` (no pid,
/// no window_id) shows its agent cursor. On macOS and Windows the key goes to
/// the frontmost window, so the cursor lands on that window's centre.
///
/// Its name sorts after `keyboard_first_session_places_its_cursor_on_the_target_window`
/// on purpose: on the Windows lane, running this row first made that row's
/// foreground `press_key` refuse with `input_delivery_stalled`.
#[cfg(not(target_os = "linux"))]
#[test]
#[ignore]
fn session_cursor_follows_a_desktop_scope_keyboard_first_action() {
    let fixture = native_fixture();
    let case = CaseSpec::delivered(
        DESKTOP_KEYBOARD_FIRST_CELL_ID,
        fixture.toolkit,
        fixture.toolkit,
        "press_key_cursor_placement",
        Targeting::NotApplicable,
        Delivery::Foreground,
        Scope::Desktop,
        platform_route(),
        vec![OracleKind::Protocol],
    );
    execute_case(case, |evidence| {
        let mut driver = spawn_driver_named(DESKTOP_KEYBOARD_FIRST_CELL_ID);
        *evidence = recording_evidence(driver.recording_dir());
        let (pid, window_id) = launch_native_fixture(&mut driver, &fixture);
        call_ok(
            &mut driver,
            "bring_to_front",
            serde_json::json!({"pid": pid, "window_id": window_id}),
        );
        settle(300);

        let session = format!("Desktop keyboard-first cursor {}", std::process::id());
        call_ok(
            &mut driver,
            "start_session",
            serde_json::json!({"session": session}),
        );
        call_ok(
            &mut driver,
            "set_agent_cursor_enabled",
            serde_json::json!({"session": session, "enabled": true}),
        );
        let before = agent_cursor_state(&mut driver, &session);
        assert!(
            before["position"].is_null(),
            "the session cursor must start unplaced so press_key is its first placement: {before}"
        );

        driver.start_behavior_recording();
        call_ok(
            &mut driver,
            "press_key",
            serde_json::json!({"session": session, "scope": "desktop", "key": "escape"}),
        );

        let after = agent_cursor_state(&mut driver, &session);
        let (Some(x), Some(y)) = (
            after["position"]["x"].as_f64(),
            after["position"]["y"].as_f64(),
        ) else {
            panic!("a desktop-scope keyboard-first press_key left the cursor unplaced: {after}");
        };
        let (centre_x, centre_y) = window_centre(&mut driver, pid, window_id);
        assert!(
            (x - centre_x).abs() <= WINDOW_CENTRE_TOLERANCE
                && (y - centre_y).abs() <= WINDOW_CENTRE_TOLERANCE,
            "desktop keyboard-first cursor landed at ({x:.1}, {y:.1}), expected the \
             frontmost window centre ({centre_x:.1}, {centre_y:.1}): {after}"
        );

        call_ok(
            &mut driver,
            "end_session",
            serde_json::json!({"session": session}),
        );
        Observation::delivered(vec![OracleKind::Protocol], Evidence::default())
    });
}

fn agent_cursor_state(driver: &mut McpDriver, session: &str) -> serde_json::Value {
    let response = driver.call(
        "get_agent_cursor_state",
        serde_json::json!({"session": session}),
    );
    assert!(
        !response.is_error(),
        "get_agent_cursor_state failed: {}",
        response.text()
    );
    response.structured().clone()
}

/// Centre of the target window's `list_windows` bounds.
fn window_centre(driver: &mut McpDriver, pid: u32, window_id: u64) -> (f64, f64) {
    let windows = driver.call("list_windows", serde_json::json!({"pid": pid}));
    let window = windows.structured()["windows"]
        .as_array()
        .and_then(|windows| {
            windows
                .iter()
                .find(|window| window["window_id"].as_u64() == Some(window_id))
        })
        .unwrap_or_else(|| panic!("target window {window_id} is not listed: {}", windows.raw))
        .clone();
    let bounds = &window["bounds"];
    let (Some(x), Some(y), Some(width), Some(height)) = (
        bounds["x"].as_f64(),
        bounds["y"].as_f64(),
        bounds["width"].as_f64(),
        bounds["height"].as_f64(),
    ) else {
        panic!("target window has no bounds: {window}");
    };
    assert!(
        width > 0.0 && height > 0.0,
        "target window is empty: {window}"
    );
    (x + width / 2.0, y + height / 2.0)
}

/// The repo-local native harness the platform's native lane stages.
struct NativeFixture {
    toolkit: &'static str,
    executable: std::path::PathBuf,
    title: &'static str,
    /// A button whose accessibility action only updates a counter.
    click_target: &'static str,
}

#[cfg(target_os = "macos")]
fn native_fixture() -> NativeFixture {
    let app = std::env::var("HARNESS_APPKIT_APP")
        .map(std::path::PathBuf::from)
        .ok()
        .filter(|path| path.exists())
        .unwrap_or_else(|| {
            cua_driver_testkit::harness_app("harness-appkit", "CuaTestHarness.AppKit.app")
        });
    NativeFixture {
        toolkit: "appkit",
        executable: app.join("Contents/MacOS/CuaTestHarness.AppKit"),
        title: "CuaTestHarness AppKit",
        click_target: "btn-increment",
    }
}

#[cfg(target_os = "windows")]
fn native_fixture() -> NativeFixture {
    NativeFixture {
        toolkit: "wpf",
        executable: cua_driver_testkit::harness_app("harness-wpf", "CuaTestHarness.Wpf.exe"),
        title: "CuaTestHarness WPF",
        // Near the top of the window, so it is on screen without scrolling.
        click_target: "border-click-target",
    }
}

#[cfg(target_os = "linux")]
fn native_fixture() -> NativeFixture {
    NativeFixture {
        toolkit: "gtk3",
        executable: std::env::var("HARNESS_GTK3_EXE")
            .map(std::path::PathBuf::from)
            .ok()
            .filter(|path| path.exists())
            .unwrap_or_else(|| {
                cua_driver_testkit::harness_app("harness-gtk3", "CuaTestHarness.Gtk3")
            }),
        title: "CuaTestHarness GTK3",
        click_target: "btn-increment",
    }
}

/// Launch the fixture and return the pid and id of its main window.
fn launch_native_fixture(driver: &mut McpDriver, fixture: &NativeFixture) -> (u32, u64) {
    assert!(
        fixture.executable.exists(),
        "required {} harness is missing at {:?}; run the fixture build",
        fixture.toolkit,
        fixture.executable
    );
    let listed_window_ids = |driver: &mut McpDriver| -> std::collections::HashSet<u64> {
        driver
            .call("list_windows", serde_json::json!({}))
            .structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|window| window["window_id"].as_u64())
            .collect()
    };
    let earlier_windows = listed_window_ids(driver);
    let child = cua_driver_testkit::spawn_in_job(
        std::process::Command::new(&fixture.executable)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .unwrap_or_else(|error| panic!("launch {} harness: {error}", fixture.toolkit));
    driver.reaper().push(child);

    // Take the new window with the fixture title, whatever process owns it:
    // a toolkit launcher can hand the window to another process, and an
    // earlier fixture window must not be mistaken for this one.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let windows = driver.call("list_windows", serde_json::json!({}));
        let found = windows.structured()["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| {
                window["title"]
                    .as_str()
                    .is_some_and(|title| title.contains(fixture.title))
                    && window["window_id"]
                        .as_u64()
                        .is_some_and(|id| !earlier_windows.contains(&id))
            })
            .and_then(|window| {
                Some((
                    u32::try_from(window["pid"].as_u64()?).ok()?,
                    window["window_id"].as_u64()?,
                ))
            });
        if let Some((pid, window_id)) =
            found.filter(|(pid, window_id)| *pid != 0 && *window_id != 0)
        {
            driver.reaper().track_pid(pid);
            #[cfg(target_os = "macos")]
            while !cua_driver_testkit::observer::macos::application_presented(pid) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "AppKit harness pid {pid} did not finish launching"
                );
                settle(50);
            }
            settle(500);
            return (pid, window_id);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{} harness window never appeared",
            fixture.toolkit
        );
        settle(250);
    }
}

/// Changed pixels in the pointer region and in the session badge's region for
/// a cursor resting at `logical_x, logical_y`. This is the one pixel oracle for
/// every platform, for still captures and for decoded video frames alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverlayPixels {
    pointer: usize,
    badge: usize,
}

impl OverlayPixels {
    fn pointer_visible(self) -> bool {
        self.pointer >= MIN_POINTER_PIXELS
    }

    fn badge_visible(self) -> bool {
        self.badge >= MIN_BADGE_PIXELS
    }
}

fn overlay_pixels(
    baseline: &RgbaImage,
    frame: &RgbaImage,
    regions: &CursorOracleRegions,
) -> OverlayPixels {
    assert_eq!(
        baseline.dimensions(),
        frame.dimensions(),
        "desktop dimensions changed while checking the cursor overlay"
    );
    OverlayPixels {
        pointer: changed_pixels_in_rect(baseline, frame, regions.pointer),
        badge: changed_pixels_in_rect(baseline, frame, regions.badge_left)
            + changed_pixels_in_rect(baseline, frame, regions.badge_right),
    }
}

fn assert_cursor_and_badge_pixels_changed(
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    logical_x: f64,
    logical_y: f64,
    logical_width: f64,
    logical_height: f64,
) {
    let regions = cursor_oracle_regions(
        baseline.width(),
        baseline.height(),
        logical_x,
        logical_y,
        logical_width,
        logical_height,
    );
    let pixels = overlay_pixels(baseline, cursor_frame, &regions);
    assert!(
        pixels.pointer_visible() && pixels.badge_visible(),
        "agent cursor overlay was incomplete near ({logical_x:.0},{logical_y:.0}): \
         pointer region changed {} pixels (minimum {MIN_POINTER_PIXELS}), \
         badge region changed {} pixels (minimum {MIN_BADGE_PIXELS}); \
         image={}x{}, logical={logical_width}x{logical_height}, {regions:?}",
        pixels.pointer,
        pixels.badge,
        baseline.width(),
        baseline.height(),
    );
}

#[derive(Clone, Copy, Debug)]
struct PixelRect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

#[derive(Clone, Copy, Debug)]
struct CursorOracleRegions {
    anchor: (i64, i64),
    scale_x: f64,
    pointer: PixelRect,
    badge_left: PixelRect,
    badge_right: PixelRect,
}

fn cursor_oracle_regions(
    image_width: u32,
    image_height: u32,
    logical_x: f64,
    logical_y: f64,
    logical_width: f64,
    logical_height: f64,
) -> CursorOracleRegions {
    let scale_x = f64::from(image_width) / logical_width;
    let scale_y = f64::from(image_height) / logical_height;
    let anchor_x = (logical_x + CURSOR_ANCHOR_OFFSET_PER_AXIS) * scale_x;
    let anchor_y = (logical_y + CURSOR_ANCHOR_OFFSET_PER_AXIS) * scale_y;

    // The production artwork is 42 points across. A 24-point radius includes
    // its outline while remaining one logical point above the badge. Floor the
    // pointer bottom and ceil the badge top so fractional and unequal scales
    // cannot round the two regions onto the same pixel row.
    let pointer = PixelRect {
        x0: (anchor_x - POINTER_ORACLE_RADIUS * scale_x).floor() as i64,
        y0: (anchor_y - POINTER_ORACLE_RADIUS * scale_y).floor() as i64,
        x1: (anchor_x + POINTER_ORACLE_RADIUS * scale_x).ceil() as i64,
        y1: (anchor_y + POINTER_ORACLE_RADIUS * scale_y).floor() as i64,
    };
    let badge_half_width = f64::from(BADGE_MAX_WIDTH) * 0.5 * scale_x;
    let badge_exclusion = BADGE_CURSOR_EXCLUSION * scale_x;
    let badge_top = (anchor_y + f64::from(BADGE_CURSOR_GAP) * scale_y).ceil() as i64;
    let badge_bottom =
        (anchor_y + f64::from(BADGE_CURSOR_GAP + BADGE_HEIGHT) * scale_y).ceil() as i64;
    // Ignore the center corridor where the pointer's lower edge or glow could
    // overlap the pill. Requiring changed pixels in the badge's outer wings
    // makes this an independent badge assertion.
    let badge_left = PixelRect {
        x0: (anchor_x - badge_half_width).floor() as i64,
        y0: badge_top,
        x1: (anchor_x - badge_exclusion).floor() as i64,
        y1: badge_bottom,
    };
    let badge_right = PixelRect {
        x0: (anchor_x + badge_exclusion).ceil() as i64,
        y0: badge_top,
        x1: (anchor_x + badge_half_width).ceil() as i64,
        y1: badge_bottom,
    };

    CursorOracleRegions {
        anchor: (anchor_x.round() as i64, anchor_y.round() as i64),
        scale_x,
        pointer,
        badge_left,
        badge_right,
    }
}

/// Pixels in `rect` that differ from the baseline by a visible amount and are
/// tinted. The agent pointer and badge are drawn in the session's colour,
/// while the OS pointer that a foreground click moves, window titles, text and
/// clocks are monochrome, so those changes cannot pass for the overlay.
fn changed_pixels_in_rect(baseline: &RgbaImage, frame: &RgbaImage, rect: PixelRect) -> usize {
    let x0 = rect.x0.clamp(0, i64::from(baseline.width())) as u32;
    let x1 = rect.x1.clamp(0, i64::from(baseline.width())) as u32;
    let y0 = rect.y0.clamp(0, i64::from(baseline.height())) as u32;
    let y1 = rect.y1.clamp(0, i64::from(baseline.height())) as u32;
    (y0..y1)
        .flat_map(|pixel_y| (x0..x1).map(move |pixel_x| (pixel_x, pixel_y)))
        .filter(|(pixel_x, pixel_y)| {
            let before = baseline.get_pixel(*pixel_x, *pixel_y).0;
            let after = frame.get_pixel(*pixel_x, *pixel_y).0;
            let changed = before
                .iter()
                .zip(after.iter())
                .map(|(left, right)| u16::from(left.abs_diff(*right)))
                .sum::<u16>()
                >= 80;
            changed && chroma(after) >= MIN_OVERLAY_CHROMA
        })
        .count()
}

fn chroma([red, green, blue, _]: [u8; 4]) -> u8 {
    red.max(green).max(blue) - red.min(green).min(blue)
}

/// Check the Driver's own desktop capture against the external cursor frame:
/// every pixel the overlay changed on screen must show the desktop there.
///
/// Native Wayland compositors draw the cursor into the captured output; there
/// the Driver must say so instead of claiming a clean capture.
fn assert_driver_capture_excludes_overlay(
    driver: &mut McpDriver,
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    logical_x: f64,
    logical_y: f64,
    logical_width: f64,
    logical_height: f64,
) {
    let (driver_png, _, _, report) = capture_desktop_png_with_report(driver);
    let status = report["status"].as_str().unwrap_or("missing");
    if wayland_session() {
        assert_eq!(
            status, "not_excluded",
            "a Wayland desktop capture must report that it could not exclude the overlay: \
             {report}"
        );
        return;
    }
    assert_eq!(
        status, "excluded",
        "desktop capture did not exclude the agent cursor overlay: {report}"
    );
    let driver_frame = image::load_from_memory(&driver_png)
        .expect("decode driver-owned desktop screenshot")
        .to_rgba8();
    assert_eq!(
        driver_frame.dimensions(),
        cursor_frame.dimensions(),
        "driver capture and cursor oracle are not in the same pixel frame"
    );
    let regions = cursor_oracle_regions(
        baseline.width(),
        baseline.height(),
        logical_x,
        logical_y,
        logical_width,
        logical_height,
    );
    let mut overlay_pixels = 0usize;
    let mut desktop_pixels = 0usize;
    for rect in [regions.pointer, regions.badge_left, regions.badge_right] {
        let (overlay, desktop) =
            overlay_pixels_resolved_to_desktop(baseline, cursor_frame, &driver_frame, rect);
        overlay_pixels += overlay;
        desktop_pixels += desktop;
    }
    assert!(
        overlay_pixels >= 36,
        "the cursor oracle no longer shows the overlay ({overlay_pixels} pixels)"
    );
    assert!(
        desktop_pixels * 10 >= overlay_pixels * 9,
        "driver desktop capture still shows the agent cursor overlay: only {desktop_pixels} \
         of {overlay_pixels} overlay pixels show the desktop; report={report}"
    );
}

/// Pixels in `rect` the overlay changed (baseline vs cursor frame), and how
/// many of those the driver capture shows closer to the desktop than to the
/// overlay. "Closer" keeps the check independent of small color differences
/// between capture pipelines.
fn overlay_pixels_resolved_to_desktop(
    baseline: &RgbaImage,
    cursor_frame: &RgbaImage,
    driver_frame: &RgbaImage,
    rect: PixelRect,
) -> (usize, usize) {
    let distance = |a: &image::Rgba<u8>, b: &image::Rgba<u8>| -> u16 {
        a.0.iter()
            .zip(b.0.iter())
            .take(3)
            .map(|(a, b)| u16::from(a.abs_diff(*b)))
            .sum()
    };
    let x0 = rect.x0.clamp(0, i64::from(baseline.width())) as u32;
    let x1 = rect.x1.clamp(0, i64::from(baseline.width())) as u32;
    let y0 = rect.y0.clamp(0, i64::from(baseline.height())) as u32;
    let y1 = rect.y1.clamp(0, i64::from(baseline.height())) as u32;
    let mut overlay = 0;
    let mut desktop = 0;
    for pixel_y in y0..y1 {
        for pixel_x in x0..x1 {
            let before = baseline.get_pixel(pixel_x, pixel_y);
            let on_screen = cursor_frame.get_pixel(pixel_x, pixel_y);
            if distance(before, on_screen) < 80 {
                continue;
            }
            overlay += 1;
            let captured = driver_frame.get_pixel(pixel_x, pixel_y);
            if distance(captured, before) < distance(captured, on_screen) {
                desktop += 1;
            }
        }
    }
    (overlay, desktop)
}

fn wayland_session() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn capture_desktop_png(driver: &mut McpDriver) -> (Vec<u8>, f64, f64) {
    let (png, width, height, _) = capture_desktop_png_with_report(driver);
    (png, width, height)
}

fn capture_desktop_png_with_report(
    driver: &mut McpDriver,
) -> (Vec<u8>, f64, f64, serde_json::Value) {
    let response = driver.call("get_desktop_state", serde_json::json!({}));
    assert!(
        !response.is_error(),
        "driver-owned desktop capture failed: {}",
        response.text()
    );
    let image_base64 = response.raw["result"]["content"]
        .as_array()
        .and_then(|content| {
            content.iter().find_map(|item| {
                (item["type"].as_str() == Some("image")
                    && item["mimeType"].as_str() == Some("image/png"))
                .then(|| item["data"].as_str())
                .flatten()
            })
        })
        .expect("driver-owned desktop capture returned no PNG image");
    let png = base64::engine::general_purpose::STANDARD
        .decode(image_base64)
        .expect("decode driver-owned desktop screenshot");
    let width = response.structured()["screen_width"]
        .as_f64()
        .or_else(|| response.structured()["screenshot_width"].as_f64())
        .expect("desktop capture returned no logical width");
    let height = response.structured()["screen_height"]
        .as_f64()
        .or_else(|| response.structured()["screenshot_height"].as_f64())
        .expect("desktop capture returned no logical height");
    let report = response.structured()["agent_overlay_capture"].clone();
    (png, width, height, report)
}

fn capture_cursor_oracle_png(driver: &mut McpDriver, width: f64, height: f64) -> Vec<u8> {
    #[cfg(target_os = "linux")]
    {
        // Native Wayland has no X11 DISPLAY to hand to x11grab. The driver's
        // display capture is the composed-screen oracle for that lane; keep
        // ffmpeg only for the canonical X11 path where it is available.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            return capture_desktop_png(driver).0;
        }
        // XGetImage root reads can omit a shaped overlay client's pixels on a
        // compositor-less X11 server. Capture the composed display exactly as
        // the behavioral recording does so the oracle observes what a user sees.
        let display = std::env::var("DISPLAY").expect("Linux cursor showcase requires DISPLAY");
        let video_size = format!("{}x{}", width.round() as u32, height.round() as u32);
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "x11grab",
                "-draw_mouse",
                "0",
                "-video_size",
                &video_size,
                "-i",
                &display,
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "pipe:1",
            ])
            .output()
            .expect("launch ffmpeg X11 display capture");
        assert!(
            output.status.success() && !output.stdout.is_empty(),
            "ffmpeg X11 display capture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    // The Driver's own desktop capture leaves the overlay out, so the oracle
    // for what a user sees comes from another capture path: a BitBlt from this
    // (different) process, which the overlay's temporary capture exclusion
    // does not apply to.
    #[cfg(target_os = "windows")]
    {
        let _ = (driver, width, height);
        platform_windows::capture::screenshot_display_bytes()
            .expect("external Windows display capture failed")
    }

    // Only the Driver holds Screen Recording permission on the macOS lanes.
    // Its per-turn recording capture deliberately keeps the overlay, so the
    // move's post-action screenshot is what a user saw.
    #[cfg(target_os = "macos")]
    {
        let _ = (width, height);
        recorded_turn_screenshot(driver, "move_cursor")
    }
}

/// The newest recorded turn's post-action screenshot for `tool`.
#[cfg(target_os = "macos")]
fn recorded_turn_screenshot(driver: &McpDriver, tool: &str) -> Vec<u8> {
    let recording_dir = driver
        .recording_dir()
        .expect("showcase recording directory")
        .to_path_buf();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut turns: Vec<_> = std::fs::read_dir(&recording_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("turn-"))
            })
            .collect();
        turns.sort();
        for turn in turns.iter().rev() {
            let Ok(action) = std::fs::read(turn.join("action.json")) else {
                continue;
            };
            let Ok(action) = serde_json::from_slice::<serde_json::Value>(&action) else {
                continue;
            };
            if action["tool"].as_str() != Some(tool) {
                continue;
            }
            if let Ok(png) = std::fs::read(turn.join("after.png")) {
                return png;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no recorded {tool} turn with an after.png under {}",
            recording_dir.display()
        );
        settle(100);
    }
}

fn call_ok(driver: &mut McpDriver, tool: &str, arguments: serde_json::Value) {
    let response = driver.call(tool, arguments);
    assert!(!response.is_error(), "{tool} failed: {}", response.text());
}

fn niri_session() -> bool {
    #[cfg(target_os = "linux")]
    {
        platform_linux::wayland::is_wayland() && platform_linux::wayland::niri::is_session()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn settle(milliseconds: u64) {
    std::thread::sleep(Duration::from_millis(milliseconds));
}

fn spawn_driver() -> McpDriver {
    spawn_driver_named(CELL_ID)
}

#[cfg(target_os = "macos")]
fn spawn_driver_named(cell_id: &str) -> McpDriver {
    McpDriver::spawn_macos_daemon_proxy_named(cell_id).expect("start installed macOS daemon proxy")
}

#[cfg(not(target_os = "macos"))]
fn spawn_driver_named(cell_id: &str) -> McpDriver {
    McpDriver::spawn_named_with_overlay(cell_id)
        .expect("start source-built driver with native cursor overlay")
}

#[cfg(target_os = "macos")]
fn platform_toolkit() -> &'static str {
    "appkit"
}

#[cfg(target_os = "windows")]
fn platform_toolkit() -> &'static str {
    "win32"
}

#[cfg(target_os = "linux")]
fn platform_toolkit() -> &'static str {
    "gtk3"
}

#[cfg(target_os = "macos")]
fn platform_route() -> DriverRoute {
    DriverRoute::Composite
}

#[cfg(target_os = "windows")]
fn platform_route() -> DriverRoute {
    DriverRoute::WindowsOverlay
}

#[cfg(target_os = "linux")]
fn platform_route() -> DriverRoute {
    if std::env::var_os("CUA_INJECT_SOCKET").is_some() {
        DriverRoute::LinuxCuaCompositorInject
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        DriverRoute::LinuxWaylandVirtualPointer
    } else {
        DriverRoute::LinuxXTest
    }
}

#[cfg(test)]
mod pixel_oracle_tests {
    use super::*;
    use image::Rgba;

    const CURSOR_X: f64 = 200.0;
    const CURSOR_Y: f64 = 150.0;

    #[test]
    fn accepts_colocated_pointer_and_badge_at_1x() {
        assert_colocated_overlay(400, 300, 400.0, 300.0, (211, 161));
    }

    #[test]
    fn accepts_colocated_pointer_and_badge_at_2x() {
        assert_colocated_overlay(800, 600, 400.0, 300.0, (423, 323));
    }

    #[test]
    fn accepts_colocated_pointer_and_badge_at_fractional_unequal_scale() {
        assert_colocated_overlay(500, 525, 400.0, 300.0, (264, 282));
    }

    #[test]
    fn rejects_meaningfully_desynchronized_pointer() {
        let (baseline, mut overlay, regions) = oracle_images(500, 525, 400.0, 300.0);
        paint_badge(&mut overlay, regions);
        paint_changed_rect_i64(
            &mut overlay,
            regions.anchor.0 + (30.0 * regions.scale_x).round() as i64,
            regions.anchor.1 - 2,
            4,
            4,
        );

        let failure = std::panic::catch_unwind(|| {
            assert_cursor_and_badge_pixels_changed(
                &baseline, &overlay, CURSOR_X, CURSOR_Y, 400.0, 300.0,
            );
        });
        assert!(
            failure.is_err(),
            "desynchronized pointer unexpectedly passed"
        );
    }

    #[test]
    fn rejects_meaningfully_desynchronized_badge() {
        let (baseline, mut overlay, regions) = oracle_images(800, 600, 400.0, 300.0);
        paint_pointer(&mut overlay, regions);
        paint_changed_rect_i64(
            &mut overlay,
            regions.badge_left.x0 + 2,
            regions.badge_left.y1 + 8,
            6,
            4,
        );

        let failure = std::panic::catch_unwind(|| {
            assert_cursor_and_badge_pixels_changed(
                &baseline, &overlay, CURSOR_X, CURSOR_Y, 400.0, 300.0,
            );
        });
        assert!(failure.is_err(), "desynchronized badge unexpectedly passed");
    }

    #[test]
    fn badge_cannot_satisfy_pointer_oracle_at_supported_scales() {
        for (width, height, logical_width, logical_height) in [
            (400, 300, 400.0, 300.0),
            (800, 600, 400.0, 300.0),
            (500, 525, 400.0, 300.0),
        ] {
            let (baseline, mut badge_only, regions) =
                oracle_images(width, height, logical_width, logical_height);
            assert!(regions.pointer.y1 < regions.badge_left.y0);
            paint_badge(&mut badge_only, regions);

            let failure = std::panic::catch_unwind(|| {
                assert_cursor_and_badge_pixels_changed(
                    &baseline,
                    &badge_only,
                    CURSOR_X,
                    CURSOR_Y,
                    logical_width,
                    logical_height,
                );
            });
            assert!(failure.is_err(), "badge-only overlay unexpectedly passed");
        }
    }

    #[test]
    fn monochrome_changes_do_not_count_as_the_overlay() {
        // A foreground click moves the OS pointer and repaints controls in
        // greys; neither may pass for the agent pointer or its badge.
        let (baseline, mut frame, regions) = oracle_images(400, 300, 400.0, 300.0);
        paint_rect_color(&mut frame, regions.pointer, Rgba([255, 255, 255, 255]));
        paint_rect_color(&mut frame, regions.badge_left, Rgba([20, 20, 20, 255]));
        assert_eq!(
            overlay_pixels(&baseline, &frame, &regions),
            OverlayPixels {
                pointer: 0,
                badge: 0
            }
        );
    }

    const SEEN: OverlayPixels = OverlayPixels {
        pointer: MIN_POINTER_PIXELS,
        badge: MIN_BADGE_PIXELS,
    };
    const POINTER_ONLY: OverlayPixels = OverlayPixels {
        pointer: MIN_POINTER_PIXELS,
        badge: 0,
    };
    const BADGE_ONLY: OverlayPixels = OverlayPixels {
        pointer: 0,
        badge: MIN_BADGE_PIXELS,
    };
    const NONE: OverlayPixels = OverlayPixels {
        pointer: 0,
        badge: 0,
    };

    /// `frames` copies of the per-place overlay `at`.
    fn rest(at: [OverlayPixels; 3], frames: usize) -> Vec<[OverlayPixels; 3]> {
        vec![at; frames]
    }

    fn timeline(parts: &[Vec<[OverlayPixels; 3]>]) -> Vec<[OverlayPixels; 3]> {
        parts.concat()
    }

    #[test]
    fn a_video_with_each_first_action_in_order_passes() {
        let frames = timeline(&[
            rest([NONE; 3], 3),
            rest([SEEN, NONE, NONE], 8),
            rest([POINTER_ONLY, NONE, NONE], 6),
            rest([NONE, POINTER_ONLY, NONE], 14),
            rest([NONE, NONE, POINTER_ONLY], 14),
            rest([NONE; 3], 5),
        ]);
        assert_eq!(first_action_rests(&frames), Ok(vec![3, 17, 31]));
    }

    #[test]
    fn a_keyboard_cursor_without_its_badge_fails() {
        let frames = timeline(&[
            rest([POINTER_ONLY, NONE, NONE], 15),
            rest([NONE, POINTER_ONLY, NONE], 15),
            rest([NONE, NONE, POINTER_ONLY], 15),
        ]);
        let failure = first_action_rests(&frames).unwrap_err();
        assert!(
            failure.starts_with("the keyboard press_key never showed its cursor")
                && failure.contains("the badge in 0"),
            "{failure}"
        );
    }

    #[test]
    fn a_keyboard_badge_without_its_pointer_fails() {
        let frames = timeline(&[
            rest([BADGE_ONLY, NONE, NONE], 15),
            rest([NONE, POINTER_ONLY, NONE], 15),
            rest([NONE, NONE, POINTER_ONLY], 15),
        ]);
        let failure = first_action_rests(&frames).unwrap_err();
        assert!(
            failure.starts_with("the keyboard press_key never showed its cursor")
                && failure.contains("the pointer showed in 0 frames"),
            "{failure}"
        );
    }

    #[test]
    fn an_overlay_that_appears_only_with_the_coordinate_action_fails() {
        // The reported X11 recording: keyboard and semantic actions ran with no
        // overlay, and the cursor first appeared for a later coordinate click.
        let frames = timeline(&[rest([NONE; 3], 30), rest([NONE, NONE, SEEN], 15)]);
        let failure = first_action_rests(&frames).unwrap_err();
        assert!(
            failure.starts_with("the keyboard press_key never showed its cursor"),
            "{failure}"
        );
    }

    #[test]
    fn a_semantic_action_that_leaves_the_cursor_behind_fails() {
        let frames = timeline(&[
            rest([SEEN, NONE, NONE], 15),
            rest([POINTER_ONLY, NONE, NONE], 15),
            rest([NONE, NONE, POINTER_ONLY], 15),
        ]);
        let failure = first_action_rests(&frames).unwrap_err();
        assert!(
            failure.starts_with("the semantic element_token click never showed its cursor"),
            "{failure}"
        );
    }

    #[test]
    fn a_flash_shorter_than_a_rest_does_not_count() {
        let frames = timeline(&[
            rest([SEEN, NONE, NONE], MIN_REST_FRAMES - 1),
            rest([NONE; 3], 10),
            rest([NONE, POINTER_ONLY, NONE], 15),
            rest([NONE, NONE, POINTER_ONLY], 15),
        ]);
        assert!(first_action_rests(&frames).is_err());
    }

    #[test]
    fn a_cursor_that_visits_the_places_out_of_order_fails() {
        let frames = timeline(&[
            rest([SEEN, NONE, NONE], 10),
            rest([NONE, NONE, POINTER_ONLY], 10),
            rest([NONE, POINTER_ONLY, NONE], 10),
        ]);
        let failure = first_action_rests(&frames).unwrap_err();
        assert!(
            failure.starts_with("the coordinate click cursor rested from frame 10, not after"),
            "{failure}"
        );
    }

    fn paint_rect_color(image: &mut RgbaImage, rect: PixelRect, color: Rgba<u8>) {
        for y in rect.y0.max(0)..rect.y1.min(i64::from(image.height())) {
            for x in rect.x0.max(0)..rect.x1.min(i64::from(image.width())) {
                image.put_pixel(x as u32, y as u32, color);
            }
        }
    }

    #[test]
    fn a_badge_band_under_another_place_counts_as_overlap() {
        // The keyboard cursor's badge under it would pass for a pointer at a
        // place just below it.
        assert!(overlay_footprints_overlap((512.0, 360.0), (512.0, 421.0)));
        assert!(overlay_footprints_overlap((512.0, 421.0), (512.0, 360.0)));
        assert!(!overlay_footprints_overlap((512.0, 360.0), (232.0, 377.0)));
        assert!(!overlay_footprints_overlap((512.0, 360.0), (512.0, 460.0)));
    }

    fn assert_colocated_overlay(
        width: u32,
        height: u32,
        logical_width: f64,
        logical_height: f64,
        expected_anchor: (i64, i64),
    ) {
        let (baseline, mut overlay, regions) =
            oracle_images(width, height, logical_width, logical_height);
        assert_eq!(regions.anchor, expected_anchor);
        assert!(regions.pointer.y1 < regions.badge_left.y0);
        paint_pointer(&mut overlay, regions);
        paint_badge(&mut overlay, regions);

        assert_cursor_and_badge_pixels_changed(
            &baseline,
            &overlay,
            CURSOR_X,
            CURSOR_Y,
            logical_width,
            logical_height,
        );
    }

    fn oracle_images(
        width: u32,
        height: u32,
        logical_width: f64,
        logical_height: f64,
    ) -> (RgbaImage, RgbaImage, CursorOracleRegions) {
        let baseline = RgbaImage::new(width, height);
        let overlay = baseline.clone();
        let regions = cursor_oracle_regions(
            width,
            height,
            CURSOR_X,
            CURSOR_Y,
            logical_width,
            logical_height,
        );
        (baseline, overlay, regions)
    }

    fn paint_pointer(image: &mut RgbaImage, regions: CursorOracleRegions) {
        paint_changed_rect_i64(image, regions.anchor.0 - 2, regions.anchor.1 - 2, 4, 4);
    }

    fn paint_badge(image: &mut RgbaImage, regions: CursorOracleRegions) {
        paint_changed_rect_i64(
            image,
            regions.badge_left.x0 + 2,
            regions.badge_left.y0 + 2,
            6,
            4,
        );
    }

    fn paint_changed_rect_i64(image: &mut RgbaImage, x: i64, y: i64, width: u32, height: u32) {
        let x = u32::try_from(x).expect("test rectangle x");
        let y = u32::try_from(y).expect("test rectangle y");
        paint_changed_rect(image, x, y, x + width, y + height);
    }

    fn paint_changed_rect(image: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgba([94, 192, 232, 255]));
            }
        }
    }
}
