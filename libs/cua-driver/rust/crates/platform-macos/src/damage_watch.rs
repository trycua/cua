// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! ScreenCaptureKit frame watcher behind
//! [`cua_driver_core::damage_settle`].
//!
//! One `SCStream` per display, scaled to [`WATCH_WIDTH`] pixels wide, with the
//! cursor and the driver's own windows (the agent cursor overlay) left out.
//! ScreenCaptureKit delivers a complete frame only when the display content
//! changed, and names the rectangles it redrew, so a frame's digest and dirty
//! rectangles are the damage signal. A caller arms it for the rectangle of a
//! window nothing covers, so a repaint in another window (the user's blinking
//! caret) does not end its wait.
//!
//! A stream starts on the first arm for its display, in the background: that
//! arm gets `None` and its caller sleeps the fixed time, so starting never
//! delays an action. A stream with no arm for
//! [`IDLE_STOP`](cua_driver_core::damage_settle::IDLE_STOP) stops, and the
//! next arm starts it again.
//!
//! Nothing starts without the Screen Recording grant and a recorded
//! direct-capture consent ([`crate::capture_consent::direct_capture_consented`]),
//! so a background action never raises that alert. A display gets no new
//! stream while its last one is still listed or stopping, so two never run at
//! once.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cua_driver_core::damage_settle::{frame_digest, Armed, DamageSignal, Region};
use screencapturekit::cm::SCFrameStatus;
use screencapturekit::prelude::{
    CGRect, CMSampleBuffer, CMSampleBufferExt, CMSampleBufferSCExt, CMTime, ErrorHandler,
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutputType,
};

use crate::windows::WindowBounds;

/// Width of a watch frame in pixels; the height keeps the display's aspect.
const WATCH_WIDTH: u32 = 256;
/// Frames ScreenCaptureKit may queue for the watcher.
const QUEUE_DEPTH: u32 = 3;
/// How often idle streams are looked for.
const IDLE_CHECK: Duration = Duration::from_secs(5);

struct Watcher {
    stream: SCStream,
    signal: Arc<DamageSignal>,
    /// The display's frame in global points.
    display: CGRect,
}

impl Watcher {
    /// `bounds` (global points) in this stream's frame pixels.
    fn region(&self, bounds: &WindowBounds) -> Region {
        let scale = f64::from(WATCH_WIDTH) / self.display.size.width.max(1.0);
        Region {
            x: (bounds.x - self.display.origin.x) * scale,
            y: (bounds.y - self.display.origin.y) * scale,
            width: bounds.width * scale,
            height: bounds.height * scale,
        }
    }

    /// Stop the stream; whether ScreenCaptureKit confirmed it stopped.
    fn stop(self) -> bool {
        self.signal.fail();
        match self.stream.stop_capture() {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(%error, "damage watch stream did not confirm its stop");
                false
            }
        }
    }
}

struct Watchers {
    running: Vec<(u32, Watcher)>,
    starting: Vec<u32>,
    stopping: Vec<u32>,
    reaping: bool,
}

static WATCHERS: Mutex<Watchers> = Mutex::new(Watchers {
    running: Vec::new(),
    starting: Vec::new(),
    stopping: Vec::new(),
    reaping: false,
});

fn watchers() -> MutexGuard<'static, Watchers> {
    WATCHERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Watch the window with global `bounds` before an action, on the display
/// that holds its centre. Damage anywhere in `bounds` counts, so the caller
/// passes a window nothing else covers ([`crate::windows::uncovered_bounds`]).
/// `None` while no usable stream runs there (one is started unless one is
/// still starting or stopping), or when capture is not allowed; the caller
/// then sleeps the fixed time.
pub fn arm_for_window(bounds: &WindowBounds) -> Option<Armed> {
    if !crate::permissions::status::screen_recording_granted()
        || !crate::capture_consent::direct_capture_consented()
    {
        return None;
    }
    let display = display_at(
        bounds.x + bounds.width / 2.0,
        bounds.y + bounds.height / 2.0,
    )?;
    let mut watchers = watchers();
    if let Some((_, watcher)) = watchers.running.iter().find(|(id, _)| *id == display) {
        // A failed stream stays listed until the idle check stops it.
        return watcher.signal.arm(watcher.region(bounds));
    }
    if !watchers.starting.contains(&display) && !watchers.stopping.contains(&display) {
        watchers.starting.push(display);
        let spawned = std::thread::Builder::new()
            .name("damage-watch-start".into())
            .spawn(move || start_watching(display));
        if spawned.is_err() {
            watchers.starting.retain(|id| *id != display);
        }
    }
    None
}

/// The display that holds the point, in global display coordinates.
fn display_at(x: f64, y: f64) -> Option<u32> {
    use core_graphics::display::CGGetDisplaysWithRect;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};

    let rect = CGRect::new(&CGPoint::new(x, y), &CGSize::new(1.0, 1.0));
    let mut display = 0u32;
    let mut count = 0u32;
    // SAFETY: room for one display id and one count, as `max_displays` says.
    let error = unsafe { CGGetDisplaysWithRect(rect, 1, &mut display, &mut count) };
    (error == 0 && count > 0).then_some(display)
}

fn start_watching(display: u32) {
    let started = start(display);
    let mut watchers = watchers();
    watchers.starting.retain(|id| *id != display);
    let watcher = match started {
        Ok(watcher) => watcher,
        Err(error) => {
            tracing::debug!(display, %error, "damage watch did not start");
            return;
        }
    };
    watchers.running.push((display, watcher));
    if !watchers.reaping {
        watchers.reaping = std::thread::Builder::new()
            .name("damage-watch-idle".into())
            .spawn(stop_idle_watchers)
            .is_ok();
    }
}

fn start(display_id: u32) -> anyhow::Result<Watcher> {
    let content = SCShareableContent::get()
        .map_err(|e| anyhow::anyhow!("SCShareableContent::get failed: {e}"))?;
    let display = content
        .displays()
        .into_iter()
        .find(|display| display.display_id() == display_id)
        .ok_or_else(|| anyhow::anyhow!("ScreenCaptureKit does not list display {display_id}"))?;
    let own_pid = std::process::id() as i32;
    let own_app: Vec<_> = content
        .applications()
        .into_iter()
        .filter(|app| app.process_id() == own_pid)
        .collect();
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_applications(&own_app.iter().collect::<Vec<_>>(), &[])
        .build();

    let height = (u64::from(WATCH_WIDTH) * u64::from(display.height())
        / u64::from(display.width().max(1)))
    .max(1) as u32;
    let config = SCStreamConfiguration::new()
        .with_width(WATCH_WIDTH)
        .with_height(height)
        .with_shows_cursor(false)
        .with_queue_depth(QUEUE_DEPTH)
        .with_minimum_frame_interval(&CMTime::new(1, 60));

    let signal = Arc::new(DamageSignal::new());
    let on_error = Arc::clone(&signal);
    let mut stream = SCStream::new_with_delegate(
        &filter,
        &config,
        ErrorHandler::new(move |error| {
            tracing::debug!(%error, "damage watch stream stopped");
            on_error.fail();
        }),
    );
    let on_frame = Arc::clone(&signal);
    stream
        .add_output_handler(
            move |sample: CMSampleBuffer, _: SCStreamOutputType| record_frame(&on_frame, &sample),
            SCStreamOutputType::Screen,
        )
        .ok_or_else(|| anyhow::anyhow!("SCStream did not take the frame handler"))?;
    stream
        .start_capture()
        .map_err(|e| anyhow::anyhow!("SCStream::start_capture failed: {e}"))?;
    Ok(Watcher {
        stream,
        signal,
        display: display.frame(),
    })
}

/// Feed a complete frame's digest and dirty rectangles (frame pixels) to the
/// signal. Idle frames (nothing changed) carry no image and are skipped.
fn record_frame(signal: &DamageSignal, sample: &CMSampleBuffer) {
    if sample.frame_status() != Some(SCFrameStatus::Complete) {
        return;
    }
    let Some(buffer) = sample.image_buffer() else {
        return;
    };
    let Ok(pixels) = buffer.lock_read_only() else {
        return;
    };
    // BGRA, the stream's default pixel format: four bytes a pixel.
    let digest = frame_digest(
        pixels.as_slice(),
        pixels.width() * 4,
        pixels.bytes_per_row(),
        pixels.height(),
    );
    let dirty: Option<Vec<Region>> = sample.dirty_rects().map(|rects| {
        rects
            .iter()
            .map(|rect| Region {
                x: rect.origin.x,
                y: rect.origin.y,
                width: rect.size.width,
                height: rect.size.height,
            })
            .collect()
    });
    signal.frame(digest, dirty.as_deref());
}

/// Stop failed and idle streams until none runs. A stopped display stays in
/// `stopping` until its stream has stopped, so no arm starts another first.
/// A display whose stop ScreenCaptureKit did not confirm stays there for the
/// life of the process, and its actions keep the fixed wait.
fn stop_idle_watchers() {
    loop {
        std::thread::sleep(IDLE_CHECK);
        let now = Instant::now();
        let idle: Vec<(u32, Watcher)> = {
            let mut watchers = watchers();
            let (idle, busy): (Vec<_>, Vec<_>) = std::mem::take(&mut watchers.running)
                .into_iter()
                .partition(|(_, watcher)| watcher.signal.should_stop(now));
            watchers.running = busy;
            watchers.stopping.extend(idle.iter().map(|(id, _)| *id));
            idle
        };
        let stopped: Vec<u32> = idle
            .into_iter()
            .filter_map(|(id, watcher)| watcher.stop().then_some(id))
            .collect();
        let mut watchers = watchers();
        watchers.stopping.retain(|id| !stopped.contains(id));
        if watchers.running.is_empty() {
            watchers.reaping = false;
            return;
        }
    }
}
