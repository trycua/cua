// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Temporary private ScreenCaptureKit backend for the RCDP CUA adapter.
//!
//! This implementation follows the same owned-frame contract as the general
//! `cua-driver-core` capture seam. It can be replaced by that public seam once
//! a released CUA revision exposes the macOS provider.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cua_media_protocol::SurfaceGeometry;
use cua_spacesd_provider_api::{
    CaptureConfig, CaptureEvent, CaptureLease, CaptureSink, OwnedFrame, PixelFormat, ProviderError,
    ProviderErrorCode,
};
use screencapturekit::cm::{CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::error::{SCError, SCStreamErrorCode};
use screencapturekit::prelude::{
    PixelFormat as ScPixelFormat, SCContentFilter, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamOutputType,
};
use screencapturekit::stream::delegate_trait::SCStreamDelegateTrait;

use super::{macos_h264::MacosH264Encoder, NativeTarget};

const WINDOW_POLL_INTERVAL: Duration = Duration::from_millis(200);
const MISSING_POLLS_BEFORE_CLOSE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
struct CaptureGeometry {
    width_px: u32,
    height_px: u32,
    scale_factor: f64,
    native_width_px: u32,
    native_height_px: u32,
}

struct StreamDelegate {
    sink: Arc<dyn CaptureSink>,
    retired: Arc<AtomicBool>,
    callback_gate: Arc<Mutex<()>>,
}

impl SCStreamDelegateTrait for StreamDelegate {
    fn stream_did_become_inactive(&self) {
        let _callback = self
            .callback_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.retired.load(Ordering::Acquire) {
            self.sink
                .on_event(CaptureEvent::Suspended("window became inactive".into()));
        }
    }

    fn did_stop_with_error(&self, error: SCError) {
        let _callback = self
            .callback_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.retired.load(Ordering::Acquire) {
            self.sink.on_event(CaptureEvent::Suspended(
                screen_capture_error_reason(&error).into(),
            ));
        }
    }
}

struct MacosCaptureLease {
    stream: Arc<SCStream>,
    retired: Arc<AtomicBool>,
    callback_gate: Arc<Mutex<()>>,
    monitor: Mutex<Option<JoinHandle<()>>>,
}

impl CaptureLease for MacosCaptureLease {
    fn stop(&self) {
        {
            let _callback = self
                .callback_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.retired.swap(true, Ordering::AcqRel) {
                return;
            }
        }
        if let Some(monitor) = self
            .monitor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = monitor.join();
        }
        let _ = self.stream.stop_capture();
    }
}

impl Drop for MacosCaptureLease {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a stream captures: one window, or a whole display (by its
/// CGDirectDisplayID, the id `CuaDisplayProvider` reports).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaptureSource {
    Window(NativeTarget),
    Display(u32),
}

pub(super) fn start(
    source: CaptureSource,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    start_inner(source, config, sink, None, native_geometry)
}

pub(super) fn start_h264(
    source: CaptureSource,
    config: &CaptureConfig,
    encoder: Arc<MacosH264Encoder>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    start_inner(
        source,
        config,
        encoder.clone(),
        Some(encoder),
        native_geometry,
    )
}

/// The shareable content behind a source, as the stream and its monitor
/// need it.
struct Located {
    filter: SCContentFilter,
    width_points: f64,
    height_points: f64,
    title: String,
    on_screen: bool,
}

fn locate(content: &SCShareableContent, source: CaptureSource) -> Option<Located> {
    match source {
        CaptureSource::Window(target) => {
            let window = content.windows().into_iter().find(|window| {
                window.window_id() == target.window_id as u32
                    && window
                        .owning_application()
                        .is_some_and(|app| i64::from(app.process_id()) == target.pid)
            })?;
            let frame = window.frame();
            Some(Located {
                filter: SCContentFilter::create().with_window(&window).build(),
                width_points: frame.size.width,
                height_points: frame.size.height,
                title: window.title().unwrap_or_default(),
                on_screen: window.is_on_screen(),
            })
        }
        CaptureSource::Display(display_id) => {
            let display = content
                .displays()
                .into_iter()
                .find(|display| display.display_id() == display_id)?;
            let frame = display.frame();
            Some(Located {
                filter: SCContentFilter::create()
                    .with_display(&display)
                    .with_excluding_windows(&[])
                    .build(),
                width_points: frame.size.width,
                height_points: frame.size.height,
                title: format!("Display {display_id}"),
                on_screen: true,
            })
        }
    }
}

fn start_inner(
    source: CaptureSource,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    encoder: Option<Arc<MacosH264Encoder>>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let content = SCShareableContent::get().map_err(capture_error)?;
    let located = locate(&content, source).ok_or_else(|| {
        ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "target is no longer shareable",
        )
    })?;

    let filter = &located.filter;
    let native_scale = f64::from(filter.point_pixel_scale()).max(1.0);
    let initial_geometry = capture_geometry(
        located.width_points,
        located.height_points,
        native_scale,
        config.max_dimension,
    );
    native_geometry(
        initial_geometry.native_width_px,
        initial_geometry.native_height_px,
    );
    let frame_interval = screencapturekit::cm::CMTime::new(1, i32::from(config.max_fps));
    let configuration = stream_configuration(initial_geometry, frame_interval);
    let retired = Arc::new(AtomicBool::new(false));
    let callback_gate = Arc::new(Mutex::new(()));
    let expected_geometry = Arc::new(Mutex::new(initial_geometry));
    let last_timestamp_us = Arc::new(AtomicU64::new(0));
    let delegate = StreamDelegate {
        sink: sink.clone(),
        retired: retired.clone(),
        callback_gate: callback_gate.clone(),
    };
    let mut stream = SCStream::new_with_delegate(filter, &configuration, delegate);
    let frame_sink = sink.clone();
    let frame_retired = retired.clone();
    let frame_callback_gate = callback_gate.clone();
    let frame_geometry = expected_geometry.clone();
    let frame_timestamp = last_timestamp_us.clone();
    let frame_encoder = encoder.clone();
    let handler = stream.add_output_handler(
        move |sample: screencapturekit::cm::CMSampleBuffer, output_type| {
            let _callback = frame_callback_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if frame_retired.load(Ordering::Acquire) || output_type != SCStreamOutputType::Screen {
                return;
            }
            let status = sample.frame_status();
            if !frame_status_allows_content(status) {
                if matches!(
                    status,
                    Some(SCFrameStatus::Suspended | SCFrameStatus::Stopped)
                ) {
                    frame_sink.on_event(CaptureEvent::Suspended(format!(
                        "ScreenCaptureKit frame status: {}",
                        status.expect("matched status is present")
                    )));
                }
                return;
            }
            let Some(pixel_buffer) = sample.image_buffer() else {
                return;
            };
            let width = pixel_buffer.width();
            let height = pixel_buffer.height();
            let geometry = *frame_geometry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if width as u32 != geometry.width_px || height as u32 != geometry.height_px {
                return;
            }
            let capture_timestamp_us =
                monotonic_sample_timestamp_us(sample.presentation_timestamp(), &frame_timestamp);
            if let Some(encoder) = &frame_encoder {
                if let Err(error) = encoder.submit_pixel_buffer(pixel_buffer, capture_timestamp_us)
                {
                    frame_sink.on_event(CaptureEvent::Suspended(error.to_string()));
                }
                return;
            }
            let Ok(guard) = pixel_buffer.lock_read_only() else {
                return;
            };
            let source_stride = guard.bytes_per_row();
            let packed_stride = width.saturating_mul(4);
            if width == 0 || height == 0 || source_stride < packed_stride {
                return;
            }
            let mut packed = Vec::with_capacity(packed_stride.saturating_mul(height));
            for row in 0..height {
                let Some(bytes) = guard.row(row) else {
                    return;
                };
                packed.extend_from_slice(&bytes[..packed_stride]);
            }
            drop(guard);
            frame_sink.on_event(CaptureEvent::Frame(OwnedFrame {
                bytes: packed.into(),
                format: PixelFormat::Bgra8,
                width_px: width as u32,
                height_px: height as u32,
                bytes_per_row: Some(packed_stride as u32),
                capture_timestamp_us,
                encode_duration_us: None,
                codec_epoch: 1,
                keyframe: true,
            }));
        },
        SCStreamOutputType::Screen,
    );
    if handler.is_none() {
        return Err(ProviderError::new(
            ProviderErrorCode::CaptureFailed,
            "ScreenCaptureKit refused the frame handler",
        ));
    }
    stream.start_capture().map_err(capture_error)?;

    sink.on_event(CaptureEvent::TitleChanged(located.title.clone()));
    sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
        width_px: initial_geometry.width_px,
        height_px: initial_geometry.height_px,
        scale_factor: initial_geometry.scale_factor,
    }));
    if !located.on_screen {
        sink.on_event(CaptureEvent::Suspended("window became inactive".into()));
    }

    let stream = Arc::new(stream);
    let monitor = spawn_monitor(
        source,
        config.max_dimension,
        frame_interval,
        stream.clone(),
        sink,
        retired.clone(),
        callback_gate.clone(),
        expected_geometry,
        native_geometry,
        located.title,
        located.on_screen,
    )?;
    Ok(Arc::new(MacosCaptureLease {
        stream,
        retired,
        callback_gate,
        monitor: Mutex::new(Some(monitor)),
    }))
}

#[allow(clippy::too_many_arguments)]
fn spawn_monitor(
    source: CaptureSource,
    max_dimension: u32,
    frame_interval: screencapturekit::cm::CMTime,
    stream: Arc<SCStream>,
    sink: Arc<dyn CaptureSink>,
    retired: Arc<AtomicBool>,
    callback_gate: Arc<Mutex<()>>,
    expected_geometry: Arc<Mutex<CaptureGeometry>>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
    initial_title: String,
    initial_on_screen: bool,
) -> Result<JoinHandle<()>, ProviderError> {
    std::thread::Builder::new()
        .name("rcdp-macos-capture-monitor".into())
        .spawn(move || {
            let mut missing_polls = 0u8;
            let mut last_title = initial_title;
            let mut last_on_screen = initial_on_screen;
            while !retired.load(Ordering::Acquire) {
                std::thread::sleep(WINDOW_POLL_INTERVAL);
                if retired.load(Ordering::Acquire) {
                    break;
                }
                let content = match SCShareableContent::get() {
                    Ok(content) => content,
                    Err(error) => {
                        sink.on_event(CaptureEvent::Suspended(
                            screen_capture_error_reason(&error).into(),
                        ));
                        continue;
                    }
                };
                let Some(located) = locate(&content, source) else {
                    missing_polls = missing_polls.saturating_add(1);
                    if missing_polls >= MISSING_POLLS_BEFORE_CLOSE {
                        let _callback = callback_gate
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if !retired.swap(true, Ordering::AcqRel) {
                            sink.on_event(CaptureEvent::Closed);
                        }
                        break;
                    }
                    continue;
                };
                missing_polls = 0;
                let on_screen = located.on_screen;
                if on_screen != last_on_screen {
                    sink.on_event(if on_screen {
                        CaptureEvent::Resumed
                    } else {
                        CaptureEvent::Suspended("window became inactive".into())
                    });
                    last_on_screen = on_screen;
                }
                if located.title != last_title {
                    sink.on_event(CaptureEvent::TitleChanged(located.title.clone()));
                    last_title = located.title;
                }
                let next = capture_geometry(
                    located.width_points,
                    located.height_points,
                    f64::from(located.filter.point_pixel_scale()),
                    max_dimension,
                );
                let current = *expected_geometry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if next == current {
                    continue;
                }
                let _callback = callback_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if retired.load(Ordering::Acquire) {
                    break;
                }
                match stream.update_configuration(&stream_configuration(next, frame_interval)) {
                    Ok(()) => {
                        *expected_geometry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
                        native_geometry(next.native_width_px, next.native_height_px);
                        sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
                            width_px: next.width_px,
                            height_px: next.height_px,
                            scale_factor: next.scale_factor,
                        }));
                    }
                    Err(error) => sink.on_event(CaptureEvent::Suspended(
                        screen_capture_error_reason(&error).into(),
                    )),
                }
            }
        })
        .map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                format!("start macOS capture monitor: {error}"),
            )
        })
}

fn stream_configuration(
    geometry: CaptureGeometry,
    frame_interval: screencapturekit::cm::CMTime,
) -> SCStreamConfiguration {
    SCStreamConfiguration::new()
        .with_width(geometry.width_px)
        .with_height(geometry.height_px)
        .with_pixel_format(ScPixelFormat::BGRA)
        .with_minimum_frame_interval(&frame_interval)
        .with_queue_depth(2)
        .with_shows_cursor(false)
}

fn frame_status_allows_content(status: Option<SCFrameStatus>) -> bool {
    status.is_none_or(SCFrameStatus::has_content)
}

fn capture_geometry(
    width_points: f64,
    height_points: f64,
    native_scale: f64,
    max_dimension: u32,
) -> CaptureGeometry {
    let width_points = width_points.max(1.0);
    let height_points = height_points.max(1.0);
    let native_width = (width_points * native_scale.max(1.0)).round() as u32;
    let native_height = (height_points * native_scale.max(1.0)).round() as u32;
    let (width_px, height_px) = fit_dimensions(native_width, native_height, max_dimension);
    CaptureGeometry {
        width_px,
        height_px,
        scale_factor: width_px as f64 / width_points,
        native_width_px: native_width,
        native_height_px: native_height,
    }
}

/// Fits the long edge within `max_dimension`; 0 means no cap (the
/// `CaptureConfig` convention, e.g. one-frame screenshots at native size).
fn fit_dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    let longest = width.max(height);
    if max_dimension == 0 || longest <= max_dimension {
        return (width, height);
    }
    let ratio = max_dimension as f64 / longest as f64;
    (
        ((width as f64 * ratio).round() as u32).max(1),
        ((height as f64 * ratio).round() as u32).max(1),
    )
}

fn monotonic_sample_timestamp_us(timestamp: screencapturekit::cm::CMTime, last: &AtomicU64) -> u64 {
    static FALLBACK_ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let candidate = if timestamp.is_valid() && timestamp.value >= 0 && timestamp.timescale > 0 {
        i128::from(timestamp.value)
            .saturating_mul(1_000_000)
            .checked_div(i128::from(timestamp.timescale))
            .and_then(|value| u64::try_from(value).ok())
    } else {
        None
    }
    .unwrap_or_else(|| {
        FALLBACK_ORIGIN
            .get_or_init(Instant::now)
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64
    });
    let mut previous = last.load(Ordering::Acquire);
    loop {
        let next = candidate.max(previous.saturating_add(1));
        match last.compare_exchange_weak(previous, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return next,
            Err(actual) => previous = actual,
        }
    }
}

fn capture_error(error: SCError) -> ProviderError {
    ProviderError::new(
        match screen_capture_error_reason(&error) {
            "consent_required" | "consent_revoked" => ProviderErrorCode::ConsentRequired,
            _ => ProviderErrorCode::CaptureFailed,
        },
        format!("ScreenCaptureKit: {error}"),
    )
}

fn screen_capture_error_reason(error: &SCError) -> &'static str {
    match error {
        SCError::PermissionDenied(_) | SCError::NoShareableContent(_) => "consent_required",
        SCError::SCStreamError {
            code: SCStreamErrorCode::UserDeclined | SCStreamErrorCode::MissingEntitlements,
            ..
        } => "consent_required",
        SCError::SCStreamError {
            code: SCStreamErrorCode::UserStopped,
            ..
        } => "consent_revoked",
        _ => "capture_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_preserve_aspect_ratio_and_cap_long_edge() {
        assert_eq!(fit_dimensions(2560, 1440, 1280), (1280, 720));
        assert_eq!(fit_dimensions(800, 1200, 600), (400, 600));
        assert_eq!(fit_dimensions(640, 480, 1280), (640, 480));
        // 0 = uncapped: a screenshot grab keeps the native size.
        assert_eq!(fit_dimensions(2048, 1536, 0), (2048, 1536));
    }

    #[test]
    fn capture_geometry_keeps_native_action_dimensions_when_stream_is_scaled() {
        let geometry = capture_geometry(960.0, 480.0, 2.0, 1280);
        assert_eq!(geometry.width_px, 1280);
        assert_eq!(geometry.height_px, 640);
        assert_eq!(geometry.native_width_px, 1920);
        assert_eq!(geometry.native_height_px, 960);
        assert!((geometry.scale_factor - (4.0 / 3.0)).abs() < f64::EPSILON);
    }
}
