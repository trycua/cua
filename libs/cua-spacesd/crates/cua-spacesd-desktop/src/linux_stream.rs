// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux X11 stream capture: XShm grabs driven by XDamage.
//!
//! One capture thread per media session owns its own X connection:
//!
//! - **Display targets** read the root window region of one monitor.
//! - **Window targets** redirect the window with Composite and read its
//!   off-screen pixmap, so overlapping windows do not leak into the stream.
//!   Without Composite the window drawable is read (occluded areas are then
//!   whatever the server returns) and the lease reports it.
//!
//! A frame is grabbed only after XDamage reports a change (or when a keyframe
//! is requested), and never faster than the frame-rate cap, so a static
//! screen costs almost nothing. The cursor is not in the image: cursors
//! travel out of band as presence. Frames go through the codec-neutral
//! `EncodingPipeline` (OpenH264 by default) or out as packed BGRA.

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cua_media_protocol::SurfaceGeometry;
use cua_spacesd_provider_api::{
    CaptureConfig, CaptureEvent, CaptureLease, CaptureSink, OwnedFrame, PixelFormat, ProviderError,
    ProviderErrorCode,
};
use cua_spacesd_session::media::clock;
use cua_spacesd_session::media::encoder::{
    downscale_bgra, fit_even, EncodingPipeline, FrameEncoderFactory, OwnedRawFrame,
};
use x11rb::connection::Connection;
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::linux_x11;

/// What one capture thread reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamSource {
    /// A root-window region (one monitor), in root coordinates.
    Display {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    },
    Window(u32),
}

struct Control {
    stop: AtomicBool,
    keyframe: AtomicBool,
    /// No socket attached: neither grab nor encode.
    paused: AtomicBool,
    max_fps: AtomicU16,
    bitrate_kbps: AtomicU32,
    frames: AtomicU64,
}

pub(crate) struct LinuxStreamLease {
    control: Arc<Control>,
    pipeline: Option<Arc<EncodingPipeline>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CaptureLease for LinuxStreamLease {
    fn request_keyframe(&self) {
        self.control.keyframe.store(true, Ordering::Release);
        if let Some(pipeline) = &self.pipeline {
            pipeline.request_keyframe();
        }
    }

    fn stop(&self) {
        self.control.stop.store(true, Ordering::Release);
        if let Some(pipeline) = &self.pipeline {
            pipeline.stop();
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            if worker.thread().id() != std::thread::current().id() {
                let _ = worker.join();
            }
        }
    }

    fn set_target_bitrate_kbps(&self, kbps: u32) {
        self.control.bitrate_kbps.store(kbps, Ordering::Relaxed);
        if let Some(pipeline) = &self.pipeline {
            pipeline.set_bitrate_kbps(kbps);
        }
    }

    fn set_max_fps(&self, fps: u16) {
        self.control.max_fps.store(fps.max(1), Ordering::Relaxed);
        if let Some(pipeline) = &self.pipeline {
            pipeline.set_max_fps(fps);
        }
    }

    fn set_paused(&self, paused: bool) {
        self.control.paused.store(paused, Ordering::Release);
        if !paused {
            // Resume with a fresh keyframe of the current picture.
            self.request_keyframe();
        }
    }

    fn encoder_name(&self) -> Option<String> {
        self.pipeline
            .as_ref()
            .map(|pipeline| pipeline.encoder_name().to_owned())
    }
}

impl Drop for LinuxStreamLease {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start capturing `source`. With an encoder factory the sink receives H.264;
/// without one it receives packed BGRA.
pub(crate) fn start(
    source: StreamSource,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    encoder: Option<Arc<dyn FrameEncoderFactory>>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let max_fps = config.max_fps.clamp(1, 120);
    let bitrate = config.target_bitrate_kbps.unwrap_or(4_000);
    let control = Arc::new(Control {
        stop: AtomicBool::new(false),
        keyframe: AtomicBool::new(true),
        paused: AtomicBool::new(false),
        max_fps: AtomicU16::new(max_fps),
        bitrate_kbps: AtomicU32::new(bitrate),
        frames: AtomicU64::new(0),
    });
    let pipeline = encoder.map(|factory| {
        Arc::new(EncodingPipeline::start(
            factory,
            max_fps,
            bitrate,
            sink.clone(),
        ))
    });
    // Connect and validate before returning so errors reach OpenMedia.
    let mut grabber = Grabber::new(source)?;
    let max_dimension = config.max_dimension;
    let worker_control = control.clone();
    let worker_pipeline = pipeline.clone();
    let worker = std::thread::Builder::new()
        .name("cua-spacesd-x11-capture".into())
        .spawn(move || {
            capture_loop(
                &mut grabber,
                &worker_control,
                worker_pipeline,
                sink,
                max_dimension,
            )
        })
        .map_err(|error| ProviderError::new(ProviderErrorCode::Internal, error.to_string()))?;
    Ok(Arc::new(LinuxStreamLease {
        control,
        pipeline,
        worker: Mutex::new(Some(worker)),
    }))
}

struct ShmBuffer {
    segment: u32,
    map: memmap2::MmapMut,
}

struct Grabber {
    conn: RustConnection,
    root: Window,
    source: StreamSource,
    damage: Option<damage::Damage>,
    composite: bool,
    pixmap: Option<Pixmap>,
    shm: Option<ShmBuffer>,
    shm_supported: bool,
    width: u32,
    height: u32,
    depth: u8,
    title: String,
    mapped: bool,
    /// Set when a new Composite pixmap was named and the client was asked to
    /// repaint into it; the first grab waits for that repaint's damage (or a
    /// short timeout) so it never shows an unpainted backing store.
    repaint_requested: Option<Instant>,
}

fn capture_error(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(
        ProviderErrorCode::CaptureFailed,
        format!("X11 capture: {error}"),
    )
}

impl Grabber {
    fn new(source: StreamSource) -> Result<Self, ProviderError> {
        let (conn, root) = linux_x11::connect()?;
        let shm_supported = conn
            .shm_query_version()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        let damage_supported = conn
            .damage_query_version(1, 1)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        let mut grabber = Self {
            conn,
            root,
            source,
            damage: None,
            composite: false,
            pixmap: None,
            shm: None,
            shm_supported,
            width: 0,
            height: 0,
            depth: 24,
            title: String::new(),
            mapped: true,
            repaint_requested: None,
        };
        let drawable = match source {
            StreamSource::Display { .. } => root,
            StreamSource::Window(window) => {
                let attributes = grabber
                    .conn
                    .get_window_attributes(window)
                    .map_err(capture_error)?
                    .reply()
                    .map_err(|_| {
                        ProviderError::new(ProviderErrorCode::TargetUnavailable, "window is gone")
                    })?;
                grabber.mapped = attributes.map_state == MapState::VIEWABLE;
                grabber.composite = grabber
                    .conn
                    .composite_query_version(0, 4)
                    .ok()
                    .and_then(|cookie| cookie.reply().ok())
                    .is_some()
                    && grabber
                        .conn
                        .composite_redirect_window(window, Redirect::AUTOMATIC)
                        .ok()
                        .and_then(|cookie| cookie.check().ok())
                        .is_some();
                let _ = grabber.conn.change_window_attributes(
                    window,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
                );
                grabber.title = linux_x11::window_title(&grabber.conn, window);
                window
            }
        };
        if damage_supported {
            let id = grabber.conn.generate_id().map_err(capture_error)?;
            if grabber
                .conn
                .damage_create(id, drawable, damage::ReportLevel::NON_EMPTY)
                .ok()
                .and_then(|cookie| cookie.check().ok())
                .is_some()
            {
                grabber.damage = Some(id);
            }
        }
        grabber.refresh_geometry()?;
        grabber.conn.flush().map_err(capture_error)?;
        Ok(grabber)
    }

    fn drawable(&self) -> Drawable {
        match self.source {
            StreamSource::Display { .. } => self.root,
            StreamSource::Window(window) => self.pixmap.unwrap_or(window),
        }
    }

    /// Re-read the source size; returns true when it changed.
    fn refresh_geometry(&mut self) -> Result<bool, ProviderError> {
        let (width, height, depth) = match self.source {
            StreamSource::Display { width, height, .. } => {
                let geometry = self
                    .conn
                    .get_geometry(self.root)
                    .map_err(capture_error)?
                    .reply()
                    .map_err(capture_error)?;
                (u32::from(width), u32::from(height), geometry.depth)
            }
            StreamSource::Window(window) => {
                let geometry = self
                    .conn
                    .get_geometry(window)
                    .map_err(capture_error)?
                    .reply()
                    .map_err(|_| {
                        ProviderError::new(ProviderErrorCode::TargetUnavailable, "window is gone")
                    })?;
                (
                    u32::from(geometry.width),
                    u32::from(geometry.height),
                    geometry.depth,
                )
            }
        };
        let changed = (width, height) != (self.width, self.height);
        if changed || self.pixmap.is_none() {
            self.width = width;
            self.height = height;
            self.depth = depth;
            if let StreamSource::Window(window) = self.source {
                if self.composite && self.mapped {
                    if let Some(old) = self.pixmap.take() {
                        let _ = self.conn.free_pixmap(old);
                    }
                    let pixmap = self.conn.generate_id().map_err(capture_error)?;
                    if self
                        .conn
                        .composite_name_window_pixmap(window, pixmap)
                        .ok()
                        .and_then(|cookie| cookie.check().ok())
                        .is_some()
                    {
                        self.pixmap = Some(pixmap);
                        // A fresh backing pixmap starts unpainted: ask the
                        // client to repaint (an Expose for the whole window).
                        let _ = self.conn.clear_area(true, window, 0, 0, 0, 0);
                        let _ = linux_x11::sync(&self.conn);
                        self.repaint_requested = Some(Instant::now());
                    }
                }
            }
        }
        Ok(changed)
    }

    fn ensure_shm(&mut self, bytes: usize) -> bool {
        if !self.shm_supported {
            return false;
        }
        if self.shm.as_ref().is_some_and(|shm| shm.map.len() >= bytes) {
            return true;
        }
        if let Some(old) = self.shm.take() {
            let _ = self.conn.shm_detach(old.segment);
        }
        let Ok(segment) = self.conn.generate_id() else {
            return false;
        };
        let Some(reply) = self
            .conn
            .shm_create_segment(segment, bytes as u32, false)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            self.shm_supported = false;
            return false;
        };
        let file = std::fs::File::from(reply.shm_fd);
        // SAFETY: the X server created a segment of at least `bytes`; the
        // mapping lives as long as this buffer and is detached on drop.
        match unsafe { memmap2::MmapOptions::new().len(bytes).map_mut(&file) } {
            Ok(map) => {
                self.shm = Some(ShmBuffer { segment, map });
                true
            }
            Err(_) => {
                let _ = self.conn.shm_detach(segment);
                self.shm_supported = false;
                false
            }
        }
    }

    /// Grab the current image as tightly packed BGRA.
    fn grab(&mut self) -> Result<Vec<u8>, ProviderError> {
        let (x, y) = match self.source {
            StreamSource::Display { x, y, .. } => (x, y),
            StreamSource::Window(_) => (0, 0),
        };
        let (width, height) = (self.width as u16, self.height as u16);
        let bytes = self.width as usize * self.height as usize * 4;
        let drawable = self.drawable();
        let data = if self.ensure_shm(bytes) {
            let segment = self.shm.as_ref().expect("shm ensured").segment;
            let reply = self
                .conn
                .shm_get_image(
                    drawable,
                    x,
                    y,
                    width,
                    height,
                    !0,
                    ImageFormat::Z_PIXMAP.into(),
                    segment,
                    0,
                )
                .map_err(capture_error)?
                .reply()
                .map_err(capture_error)?;
            let size = reply.size as usize;
            let shm = self.shm.as_ref().expect("shm ensured");
            shm.map[..size.min(shm.map.len())].to_vec()
        } else {
            self.conn
                .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, width, height, !0)
                .map_err(capture_error)?
                .reply()
                .map_err(capture_error)?
                .data
        };
        let stride = if self.height == 0 {
            0
        } else {
            data.len() / self.height as usize
        };
        if stride < self.width as usize * 4 {
            return Err(capture_error(format!(
                "unsupported pixel layout: {} bytes for {}x{} (depth {})",
                data.len(),
                self.width,
                self.height,
                self.depth
            )));
        }
        let mut packed = Vec::with_capacity(bytes);
        for row in data.chunks(stride).take(self.height as usize) {
            packed.extend_from_slice(&row[..self.width as usize * 4]);
        }
        for pixel in packed.as_chunks_mut::<4>().0 {
            pixel[3] = 0xff;
        }
        Ok(packed)
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        if let Some(shm) = self.shm.take() {
            let _ = self.conn.shm_detach(shm.segment);
        }
        if let Some(pixmap) = self.pixmap.take() {
            let _ = self.conn.free_pixmap(pixmap);
        }
        if let Some(damage) = self.damage.take() {
            let _ = self.conn.damage_destroy(damage);
        }
        if let StreamSource::Window(window) = self.source {
            if self.composite {
                let _ = self
                    .conn
                    .composite_unredirect_window(window, Redirect::AUTOMATIC);
            }
        }
        let _ = linux_x11::sync(&self.conn);
    }
}

enum Observed {
    Nothing,
    Damaged,
    Resized,
    Title(String),
    Unmapped,
    Mapped,
    Closed,
}

fn drain_events(grabber: &mut Grabber) -> Vec<Observed> {
    let mut observed = Vec::new();
    // Bounded per tick so an event storm cannot starve the capture loop.
    for _ in 0..4096 {
        let Ok(Some(event)) = grabber.conn.poll_for_event() else {
            break;
        };
        match (event, grabber.source) {
            (Event::DamageNotify(_), _) => observed.push(Observed::Damaged),
            (Event::ConfigureNotify(configure), StreamSource::Window(window))
                if configure.window == window =>
            {
                if (u32::from(configure.width), u32::from(configure.height))
                    != (grabber.width, grabber.height)
                {
                    observed.push(Observed::Resized);
                }
            }
            (Event::DestroyNotify(destroy), StreamSource::Window(window))
                if destroy.window == window =>
            {
                observed.push(Observed::Closed)
            }
            (Event::UnmapNotify(unmap), StreamSource::Window(window)) if unmap.window == window => {
                observed.push(Observed::Unmapped)
            }
            (Event::MapNotify(map), StreamSource::Window(window)) if map.window == window => {
                observed.push(Observed::Mapped)
            }
            (Event::PropertyNotify(property), StreamSource::Window(window))
                if property.window == window =>
            {
                let title = linux_x11::window_title(&grabber.conn, window);
                if title != grabber.title {
                    grabber.title = title.clone();
                    observed.push(Observed::Title(title));
                }
            }
            _ => observed.push(Observed::Nothing),
        }
    }
    if let Some(damage) = grabber.damage {
        if observed
            .iter()
            .any(|event| matches!(event, Observed::Damaged))
        {
            let _ = grabber
                .conn
                .damage_subtract(damage, x11rb::NONE, x11rb::NONE);
            let _ = grabber.conn.flush();
        }
    }
    observed
}

fn capture_loop(
    grabber: &mut Grabber,
    control: &Control,
    pipeline: Option<Arc<EncodingPipeline>>,
    sink: Arc<dyn CaptureSink>,
    max_dimension: u32,
) {
    let mut dirty = true;
    let mut dirty_at: Option<Instant> = None;
    // When the content first changed since the last grab. Frames are stamped
    // with it, so the video timestamp is the change time rather than the
    // (up to one frame interval later) grab time; audio stamps its capture
    // time the same way, which keeps A/V aligned.
    let mut changed_since_grab: Option<Instant> = None;
    let mut suspended = false;
    let mut last_geometry_check = Instant::now();
    let mut last_frame = Instant::now() - Duration::from_secs(1);
    let damage_driven = grabber.damage.is_some();
    // Without XDamage fall back to a slow poll so the stream still updates.
    let poll_without_damage = Duration::from_millis(250);
    while !control.stop.load(Ordering::Acquire) {
        let fps = control.max_fps.load(Ordering::Relaxed).max(1);
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
        for event in drain_events(grabber) {
            match event {
                Observed::Damaged | Observed::Resized => {
                    dirty = true;
                    let now = Instant::now();
                    dirty_at = Some(now);
                    changed_since_grab.get_or_insert(now);
                }
                Observed::Title(title) => sink.on_event(CaptureEvent::TitleChanged(title)),
                Observed::Unmapped => {
                    grabber.mapped = false;
                    if !suspended {
                        suspended = true;
                        sink.on_event(CaptureEvent::Suspended("minimized".into()));
                    }
                }
                Observed::Mapped => {
                    grabber.mapped = true;
                    grabber.pixmap = None;
                    dirty = true;
                }
                Observed::Closed => {
                    sink.on_event(CaptureEvent::Closed);
                    return;
                }
                Observed::Nothing => {}
            }
        }
        if !damage_driven && last_frame.elapsed() >= poll_without_damage {
            dirty = true;
        }
        // Display resolution changes (xrandr) are polled once a second.
        if last_geometry_check.elapsed() >= Duration::from_secs(1) || dirty {
            last_geometry_check = Instant::now();
            match grabber.refresh_geometry() {
                Ok(true) => dirty = true,
                Ok(false) => {}
                Err(error) if error.code == ProviderErrorCode::TargetUnavailable => {
                    sink.on_event(CaptureEvent::Closed);
                    return;
                }
                Err(_) => {}
            }
        }
        if control.paused.load(Ordering::Acquire) {
            // Events above keep being drained; grab once resumed.
            dirty = true;
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let keyframe_wanted = control.keyframe.swap(false, Ordering::AcqRel);
        if keyframe_wanted && pipeline.is_none() {
            // Raw frames are independent: resend the current image.
            dirty = true;
        }
        let due = last_frame.elapsed() >= interval;
        if let Some(requested) = grabber.repaint_requested {
            let waited = requested.elapsed();
            let repainted =
                damaged_since(requested, &mut dirty_at) && waited >= Duration::from_millis(50);
            if waited < Duration::from_millis(300) && !repainted {
                std::thread::sleep(Duration::from_millis(4));
                continue;
            }
            grabber.repaint_requested = None;
            dirty = true;
        }
        if dirty && due && grabber.mapped {
            dirty = false;
            last_frame = Instant::now();
            match grabber.grab() {
                Ok(bgra) => {
                    if suspended {
                        suspended = false;
                        sink.on_event(CaptureEvent::Resumed);
                    }
                    control.frames.fetch_add(1, Ordering::Relaxed);
                    let changed = changed_since_grab.take().unwrap_or_else(Instant::now);
                    publish(
                        grabber,
                        bgra,
                        pipeline.as_deref(),
                        &sink,
                        max_dimension,
                        clock::to_media_us(changed),
                    );
                }
                Err(error) => {
                    tracing::debug!(target: "cua_spacesd_client::linux_stream", %error, "grab failed");
                    if !suspended {
                        suspended = true;
                        sink.on_event(CaptureEvent::Suspended("capture_failed".into()));
                    }
                    grabber.pixmap = None;
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(4).min(interval));
        }
    }
}

fn damaged_since(since: Instant, dirty_at: &mut Option<Instant>) -> bool {
    dirty_at.is_some_and(|at| at > since)
}

fn publish(
    grabber: &Grabber,
    bgra: Vec<u8>,
    pipeline: Option<&EncodingPipeline>,
    sink: &Arc<dyn CaptureSink>,
    max_dimension: u32,
    capture_us: u64,
) {
    let (width, height) = fit_even(grabber.width, grabber.height, max_dimension);
    let (bgra, stride) = if (width, height) == (grabber.width, grabber.height) {
        (bgra, grabber.width * 4)
    } else if width <= grabber.width
        && height <= grabber.height
        && (width, height) == (grabber.width & !1, grabber.height & !1)
    {
        // Only odd extents to crop: keep the source stride.
        (bgra, grabber.width * 4)
    } else {
        (
            downscale_bgra(
                &bgra,
                grabber.width,
                grabber.height,
                grabber.width * 4,
                width,
                height,
            ),
            width * 4,
        )
    };
    match pipeline {
        Some(pipeline) => pipeline.submit(OwnedRawFrame {
            bgra: Arc::from(bgra),
            width,
            height,
            stride,
            capture_us,
        }),
        None => {
            let packed = if stride == width * 4 {
                bgra
            } else {
                let mut packed = Vec::with_capacity((width * height * 4) as usize);
                for row in bgra.chunks(stride as usize).take(height as usize) {
                    packed.extend_from_slice(&row[..(width * 4) as usize]);
                }
                packed
            };
            sink.on_event(CaptureEvent::Frame(OwnedFrame {
                bytes: Arc::from(packed),
                format: PixelFormat::Bgra8,
                width_px: width,
                height_px: height,
                bytes_per_row: Some(width * 4),
                capture_timestamp_us: capture_us,
                encode_duration_us: None,
                codec_epoch: 1,
                keyframe: true,
            }));
        }
    }
    let _ = SurfaceGeometry {
        width_px: width,
        height_px: height,
        scale_factor: 1.0,
    };
}

/// One-shot grab for `ComputerService.Screenshot`: tightly packed BGRA of a
/// display region or a window (Composite-backed when available).
pub(crate) fn grab_once(source: StreamSource) -> Result<(Vec<u8>, u32, u32), ProviderError> {
    let mut grabber = Grabber::new(source)?;
    let bgra = grabber.grab()?;
    Ok((bgra, grabber.width, grabber.height))
}
