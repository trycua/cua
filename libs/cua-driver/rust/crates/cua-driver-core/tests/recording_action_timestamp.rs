use cua_driver_core::recording::{
    now_ms, set_ax_snapshot_fn, set_element_bounds_fn, set_screenshot_fn, RecordingCaller,
    RecordingSession,
};
use serde_json::{json, Value};
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn install_capture_fixtures() {
    let png = cua_driver_core::image_utils::encode_rgba_to_png(&[255; 16], 2, 2).unwrap();

    set_screenshot_fn(move |_, _| Some(png.clone()));
    set_ax_snapshot_fn(|_, _| Some(br#"{"fixture":"action-timestamp"}"#.to_vec()));
    set_element_bounds_fn(|_, _, _| None);
}

#[test]
fn action_json_timestamp_is_the_documented_iso_8601_wall_clock_time() {
    install_capture_fixtures();
    let directory = tempfile::tempdir().unwrap();
    let recording = RecordingSession::new();

    // The recorder keeps milliseconds, so compare against a millisecond floor.
    let earliest = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    recording
        .start(directory.path().to_str().unwrap(), false, None)
        .unwrap();

    // Keep session start, turn start, and turn end in distinct milliseconds.
    std::thread::sleep(Duration::from_millis(5));
    let start_ms = now_ms();
    let pending = recording
        .begin_turn(
            "press_key",
            &json!({"pid": 1, "key": "a"}),
            start_ms,
            RecordingCaller::default(),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    recording.finish_turn_with_outcome(pending, "Pressed a", None, false);
    recording.stop_owner(None).unwrap();
    let latest = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;

    let recorded: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("turn-00001").join("action.json")).unwrap(),
    )
    .unwrap();
    let timestamp = recorded["timestamp"].as_str().unwrap();
    let parsed = OffsetDateTime::parse(timestamp, &Rfc3339)
        .unwrap_or_else(|error| panic!("timestamp {timestamp:?} is not ISO-8601: {error}"));
    let parsed_ms = parsed.unix_timestamp_nanos() / 1_000_000;

    assert!(
        (earliest..=latest).contains(&parsed_ms),
        "timestamp {timestamp:?} must be the turn's wall-clock time, between {earliest} and {latest} ms"
    );

    // Fixed shape: UTC with millisecond precision, e.g. 2026-01-01T00:00:00.000Z.
    assert_eq!(timestamp.len(), 24, "unexpected shape: {timestamp:?}");
    assert!(timestamp.ends_with('Z'), "unexpected shape: {timestamp:?}");

    let end = recorded["t_ms_from_session_start"].as_u64().unwrap();
    let start = recorded["t_start_ms_from_session_start"].as_u64().unwrap();
    assert!(0 < start && start < end, "start {start} ms, end {end} ms");

    // Wall-clock and relative timestamps must share the session anchor.
    assert_eq!(
        parsed_ms - i128::from(end),
        i128::from(start_ms) - i128::from(start),
        "timestamp {timestamp:?} and the relative fields must share one session anchor"
    );

    let anchor = parsed_ms - i128::from(end);
    assert!(
        (earliest..=i128::from(start_ms)).contains(&anchor),
        "session anchor {anchor} must lie between {earliest} and the turn start {start_ms}"
    );
}

// Recording starts a live cursor sampler on macOS/Windows; Linux's sampler is
// a no-op, so this regression can run without reading desktop state.
#[cfg(target_os = "linux")]
mod video_start {
    use super::*;
    use cua_driver_core::video::{
        set_video_backend_factory, VideoBackend, VideoBackendFactory, VideoMetadata,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const STARTUP_MS: u64 = 300;
    static FACTORY_ENTERED_MS: AtomicU64 = AtomicU64::new(0);

    struct SlowStartFactory;
    struct FakeVideoBackend(PathBuf);

    impl VideoBackendFactory for SlowStartFactory {
        fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
            FACTORY_ENTERED_MS.store(now_ms(), Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(STARTUP_MS));

            Ok(Box::new(FakeVideoBackend(output_path.to_path_buf())))
        }
    }

    impl VideoBackend for FakeVideoBackend {
        fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
            std::fs::write(&self.0, b"fixture video")?;

            Ok(VideoMetadata {
                path: self.0,
                duration_ms: STARTUP_MS,
                finalized: true,
            })
        }
    }

    #[test]
    fn turn_times_count_from_before_the_video_backend_starts() {
        install_capture_fixtures();
        set_video_backend_factory(Box::new(SlowStartFactory));
        let directory = tempfile::tempdir().unwrap();
        let recording = RecordingSession::new();

        recording
            .start(directory.path().to_str().unwrap(), true, None)
            .unwrap();

        let pending = recording
            .begin_turn(
                "press_key",
                &json!({"pid": 1, "key": "a"}),
                now_ms(),
                RecordingCaller::default(),
            )
            .unwrap();
        recording.finish_turn_with_outcome(pending, "Pressed a", None, false);
        recording.stop_owner(None).unwrap();

        let action: Value = serde_json::from_slice(
            &std::fs::read(directory.path().join("turn-00001").join("action.json")).unwrap(),
        )
        .unwrap();
        let session: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("session.json")).unwrap())
                .unwrap();

        // Allow wall-clock rounding below the simulated startup duration.
        let start_ms = action["t_start_ms_from_session_start"].as_u64().unwrap();
        assert!(start_ms >= STARTUP_MS - 50, "turn start {start_ms} ms");
        assert_eq!(session["video"]["present"], true);
        assert!(
            session["started_at_monotonic_ms"].as_u64().unwrap()
                <= FACTORY_ENTERED_MS.load(Ordering::SeqCst)
        );
    }
}
