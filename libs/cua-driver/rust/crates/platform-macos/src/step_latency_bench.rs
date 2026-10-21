//! Micro-benchmark: how long one step of a batch is held up by the agent
//! cursor's glide and by the post-action window watch, before and after the
//! batch pacing in `cua_driver_core::step_pacing`.
//!
//! It runs the driver's own waits with no accessibility or screen-recording
//! permission and no window: the glide time comes from the cursor planner with
//! the default `steady` timing, the arrival is delivered when that glide ends,
//! and the window watch polls a synthetic window list that never changes (an
//! action that opens nothing). Run it on any macOS host:
//!
//! ```sh
//! cd libs/cua-driver/rust
//! cargo test --release -p platform-macos --lib step_latency_bench \
//!     -- --ignored --nocapture
//! ```
//!
//! Each row is one JSON line (`distance_px`, `glide_ms`, `direct_ms`,
//! `batched_ms`), followed by the totals for a typical pointer step and for a
//! key step. It measures only the two waits. It does not time the action
//! itself (an AX press takes about 80 ms) or a real glide on a live overlay.

use std::time::{Duration, Instant};

use cua_cursor_motion::{plan_move, MotionParams, MoveRequest, Pt};
use cua_driver_core::step_pacing::{
    report_window, scope, StepPacing, BATCH_GLIDE_WAIT, BATCH_REPORT_WINDOW,
};
use cua_driver_core::window_observation::WindowObservationBounds;

use crate::cursor::overlay::await_arrival;
use crate::window_change_detector::{observe_quiet_window_list, DIRECT_REPORT_WINDOW};

const DISTANCES_PX: [f64; 4] = [120.0, 320.0, 640.0, 1100.0];
const RUNS: usize = 5;

/// How long one default-timing cursor move of `distance` points takes to
/// arrive, in ms. The overlay releases the waiting action at arrival; the
/// settle after it plays during the action.
fn glide_ms(distance: f64) -> f64 {
    plan_move(
        &MotionParams::default(),
        &MoveRequest::new(Pt::new(100.0, 400.0), Pt::new(100.0 + distance, 400.0)),
    )
    .arrival_t
        * 1000.0
}

/// Median of `RUNS` timings of `once`, in ms.
fn median_ms(mut once: impl FnMut() -> Duration) -> f64 {
    let mut runs: Vec<f64> = (0..RUNS).map(|_| once().as_secs_f64() * 1000.0).collect();
    runs.sort_by(|a, b| a.total_cmp(b));
    runs[RUNS / 2]
}

/// How long a step with this pacing waits for a glide of `glide` to arrive.
fn glide_hold_up(pacing: StepPacing, glide: Duration) -> Duration {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(scope(pacing, async move {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        std::thread::spawn(move || {
            std::thread::sleep(glide);
            let _ = tx.send(());
        });
        let started = Instant::now();
        await_arrival(rx, true).await;
        started.elapsed()
    }))
}

/// How long a step with this pacing waits for the window watch to report a
/// quiet window list.
fn watch_hold_up(pacing: StepPacing) -> Duration {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let report = runtime.block_on(scope(pacing, async { report_window(DIRECT_REPORT_WINDOW) }));
    let bounds = WindowObservationBounds::from_raw(
        None,
        None,
        Duration::from_millis(1000),
        Duration::from_millis(25),
    );
    let started = Instant::now();
    observe_quiet_window_list(bounds, report);
    started.elapsed()
}

#[test]
#[ignore = "benchmark: run with --ignored --nocapture"]
fn batched_steps_are_held_up_less_by_the_glide_and_the_window_watch() {
    println!(
        "step pacing: batch glide cap {} ms, batch report window {} ms, direct report window {} ms",
        BATCH_GLIDE_WAIT.as_millis(),
        BATCH_REPORT_WINDOW.as_millis(),
        DIRECT_REPORT_WINDOW.as_millis()
    );

    let mut glide_direct_total = 0.0;
    let mut glide_batched_total = 0.0;
    for distance in DISTANCES_PX {
        let glide = glide_ms(distance);
        let glide_duration = Duration::from_secs_f64(glide / 1000.0);
        let direct = median_ms(|| glide_hold_up(StepPacing::Single, glide_duration));
        let batched = median_ms(|| glide_hold_up(StepPacing::Batched, glide_duration));
        println!(
            "{{\"wait\":\"cursor_glide\",\"distance_px\":{distance},\"glide_ms\":{glide:.0},\
             \"direct_ms\":{direct:.0},\"batched_ms\":{batched:.0}}}"
        );
        assert!(batched <= direct + 5.0);
        glide_direct_total += direct;
        glide_batched_total += batched;
    }
    let glide_direct = glide_direct_total / DISTANCES_PX.len() as f64;
    let glide_batched = glide_batched_total / DISTANCES_PX.len() as f64;

    let watch_direct = median_ms(|| watch_hold_up(StepPacing::Single));
    let watch_batched = median_ms(|| watch_hold_up(StepPacing::Batched));
    println!(
        "{{\"wait\":\"window_watch\",\"direct_ms\":{watch_direct:.0},\"batched_ms\":{watch_batched:.0}}}"
    );
    assert!(watch_batched < watch_direct);

    let pointer_direct = glide_direct + watch_direct;
    let pointer_batched = glide_batched + watch_batched;
    println!(
        "{{\"step\":\"pointer (click, type_text, set_value, scroll)\",\"direct_ms\":{pointer_direct:.0},\
         \"batched_ms\":{pointer_batched:.0},\"saved_ms\":{:.0}}}",
        pointer_direct - pointer_batched
    );
    println!(
        "{{\"step\":\"key (press_key, hotkey)\",\"direct_ms\":{watch_direct:.0},\
         \"batched_ms\":{watch_batched:.0},\"saved_ms\":{:.0}}}",
        watch_direct - watch_batched
    );
}
