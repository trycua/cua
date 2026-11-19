// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Turn any BGRA capture backend into an H.264 one.
//!
//! Backends that only produce CPU BGRA frames (the Hyprland/Wayland window
//! grabber) start behind an [`EncodingPipeline`]: raw frames go to the
//! encoder worker (`cua-media-codec`'s fastest working encoder), everything
//! else (geometry, title, suspend/resume, close) passes straight through.
//! Keyframe requests re-encode the last frame, so a static window still
//! answers them.

use std::sync::Arc;

use cua_spacesd_provider_api::{
    CaptureConfig, CaptureEvent, CaptureLease, CaptureSink, PixelFormat, ProviderError,
};
use cua_spacesd_session::media::encoder::{EncodingPipeline, FrameEncoderFactory, OwnedRawFrame};

struct PipelineSink {
    pipeline: Arc<EncodingPipeline>,
    downstream: Arc<dyn CaptureSink>,
}

impl CaptureSink for PipelineSink {
    fn on_event(&self, event: CaptureEvent) {
        match event {
            CaptureEvent::Frame(frame) if frame.format == PixelFormat::Bgra8 => {
                self.pipeline.submit(OwnedRawFrame {
                    stride: frame.bytes_per_row.unwrap_or(frame.width_px * 4),
                    bgra: frame.bytes,
                    width: frame.width_px,
                    height: frame.height_px,
                    capture_us: frame.capture_timestamp_us,
                });
            }
            other => self.downstream.on_event(other),
        }
    }
}

struct EncodedLease {
    capture: Arc<dyn CaptureLease>,
    pipeline: Arc<EncodingPipeline>,
}

impl CaptureLease for EncodedLease {
    fn request_keyframe(&self) {
        self.pipeline.request_keyframe();
    }

    fn stop(&self) {
        self.capture.stop();
        self.pipeline.stop();
    }

    fn set_target_bitrate_kbps(&self, kbps: u32) {
        self.pipeline.set_bitrate_kbps(kbps);
    }

    fn set_max_fps(&self, fps: u16) {
        self.capture.set_max_fps(fps);
        self.pipeline.set_max_fps(fps);
    }

    fn set_paused(&self, paused: bool) {
        self.capture.set_paused(paused);
    }

    fn encoder_name(&self) -> Option<String> {
        Some(self.pipeline.encoder_name().to_owned())
    }
}

/// Start `start_bgra` (a BGRA-only backend) behind an encoder so the sink
/// receives H.264 access units.
pub(crate) fn start_encoded(
    factory: Arc<dyn FrameEncoderFactory>,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    start_bgra: impl FnOnce(
        &CaptureConfig,
        Arc<dyn CaptureSink>,
    ) -> Result<Arc<dyn CaptureLease>, ProviderError>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let pipeline = Arc::new(EncodingPipeline::start(
        factory,
        config.max_fps.max(1),
        config.target_bitrate_kbps.unwrap_or(4_000),
        sink.clone(),
    ));
    let bgra_config = CaptureConfig {
        accepted_formats: vec![PixelFormat::Bgra8],
        ..config.clone()
    };
    let adapter = Arc::new(PipelineSink {
        pipeline: pipeline.clone(),
        downstream: sink,
    });
    match start_bgra(&bgra_config, adapter) {
        Ok(capture) => Ok(Arc::new(EncodedLease { capture, pipeline })),
        Err(error) => {
            pipeline.stop();
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spacesd_provider_api::OwnedFrame;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Collect(Mutex<Vec<CaptureEvent>>);

    impl CaptureSink for Collect {
        fn on_event(&self, event: CaptureEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    struct NoopLease;

    impl CaptureLease for NoopLease {
        fn stop(&self) {}
    }

    #[test]
    fn bgra_backend_frames_come_out_as_h264_and_other_events_pass_through() {
        let collect = Arc::new(Collect::default());
        let backend_sink: Arc<Mutex<Option<Arc<dyn CaptureSink>>>> = Arc::default();
        let captured = backend_sink.clone();
        let lease = start_encoded(
            Arc::new(crate::codec_encoder::CodecEncoderFactory::default()),
            &CaptureConfig {
                max_fps: 30,
                max_dimension: 0,
                target_bitrate_kbps: Some(1_000),
                accepted_formats: vec![PixelFormat::H264AnnexB],
            },
            collect.clone(),
            move |config, sink| {
                assert_eq!(config.accepted_formats, vec![PixelFormat::Bgra8]);
                *captured.lock().unwrap() = Some(sink);
                Ok(Arc::new(NoopLease))
            },
        )
        .unwrap();
        let sink = backend_sink.lock().unwrap().clone().unwrap();
        sink.on_event(CaptureEvent::TitleChanged("t".into()));
        sink.on_event(CaptureEvent::Frame(OwnedFrame {
            bytes: Arc::from([30u8, 60, 90, 255].repeat(64 * 48)),
            format: PixelFormat::Bgra8,
            width_px: 64,
            height_px: 48,
            bytes_per_row: Some(256),
            capture_timestamp_us: 7,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        }));
        let deadline = Instant::now() + Duration::from_secs(10);
        let encoded = loop {
            let found = collect
                .0
                .lock()
                .unwrap()
                .iter()
                .find_map(|event| match event {
                    CaptureEvent::Frame(frame) => Some(frame.clone()),
                    _ => None,
                });
            if let Some(frame) = found {
                break frame;
            }
            assert!(Instant::now() < deadline, "no encoded frame");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(encoded.format, PixelFormat::H264AnnexB);
        assert!(encoded.keyframe);
        assert_eq!(encoded.capture_timestamp_us, 7);
        assert!(matches!(
            collect.0.lock().unwrap()[0],
            CaptureEvent::TitleChanged(_)
        ));
        assert!(lease.encoder_name().is_some());
        lease.stop();
    }
}
