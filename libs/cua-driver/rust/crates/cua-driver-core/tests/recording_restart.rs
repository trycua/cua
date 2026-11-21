// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A start over a live recording, and the idle stop, through the public
//! recorder API. `.cursor/skills/formally-verify-cua-driver/models/recording.tla`
//! checks the same rules.

use std::path::{Path, PathBuf};

use cua_driver_core::recording::{RecordingSession, StopBy};
use cua_driver_core::video::{
    set_video_backend_factory, VideoBackend, VideoBackendFactory, VideoMetadata,
};
use serde_json::Value;

struct FakeFactory;
/// Writes its file only when stopped, so a missing file means still running.
struct FakeVideo(PathBuf);

impl VideoBackendFactory for FakeFactory {
    fn start(&self, output_path: &Path) -> anyhow::Result<Box<dyn VideoBackend>> {
        Ok(Box::new(FakeVideo(output_path.to_path_buf())))
    }
}

impl VideoBackend for FakeVideo {
    fn stop(self: Box<Self>) -> anyhow::Result<VideoMetadata> {
        std::fs::write(&self.0, b"fixture video")?;
        Ok(VideoMetadata {
            path: self.0,
            duration_ms: 1,
            finalized: true,
        })
    }
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("session.json")).unwrap()).unwrap()
}

fn utf8(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn restart_finalizes_the_previous_recording() {
    set_video_backend_factory(Box::new(FakeFactory));
    let root = tempfile::tempdir().unwrap();
    let (first, second) = (root.path().join("first"), root.path().join("second"));
    let session = RecordingSession::new();
    session.start(utf8(&first), true, Some("a")).unwrap();
    session.start(utf8(&second), true, Some("b")).unwrap();

    let old = manifest(&first);
    assert_eq!(old["video"]["finalized"], true, "{old}");
    assert_eq!(
        old["video"]["absolute_path"],
        utf8(&first.join("recording.mp4"))
    );
    assert!(first.join("recording.mp4").exists());

    let state = session.current_state();
    assert!(state.enabled && state.video_active);
    assert_eq!(state.owner.as_deref(), Some("b"));
    assert_eq!(state.output_dir.as_deref(), Some(utf8(&second)));
    session.stop(StopBy::Anyone).unwrap();
    assert_eq!(manifest(&second)["video"]["finalized"], true);
}

#[test]
fn restart_into_an_unusable_directory_keeps_the_live_recording() {
    set_video_backend_factory(Box::new(FakeFactory));
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let blocker = root.path().join("file");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let session = RecordingSession::new();
    session.start(utf8(&live), true, None).unwrap();

    assert!(session
        .start(utf8(&blocker.join("child")), true, None)
        .is_err());

    assert!(!live.join("recording.mp4").exists(), "video kept running");
    let state = session.current_state();
    assert!(state.enabled && state.video_active);
    assert_eq!(state.output_dir.as_deref(), Some(utf8(&live)));
    session.stop(StopBy::Anyone).unwrap();
    assert_eq!(manifest(&live)["video"]["finalized"], true);
}

#[test]
fn a_stop_for_an_earlier_generation_leaves_the_newer_recording_running() {
    let root = tempfile::tempdir().unwrap();
    let session = RecordingSession::new();
    session
        .start(utf8(&root.path().join("first")), false, None)
        .unwrap();
    let read = session.current_state().generation.unwrap();
    session
        .start(utf8(&root.path().join("second")), false, None)
        .unwrap();

    session.stop(StopBy::Generation(read)).unwrap();
    assert!(session.current_state().enabled, "newer recording survives");

    let now = session.current_state().generation.unwrap();
    session.stop(StopBy::Generation(now)).unwrap();
    let state = session.current_state();
    assert!(!state.enabled);
    assert_eq!(state.generation, None);
}
