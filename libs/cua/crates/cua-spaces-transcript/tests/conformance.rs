//! The Rust third of the cross-language proof.
//!
//! Rust writes the fixtures, so this suite alone would be circular. It is here
//! because the Swift and TypeScript suites compare against *files*, and a file
//! is only evidence if something re-derives it: this test is what fails when
//! someone edits a fixture by hand instead of changing the emulator.

use cua_spaces_transcript::conformance::{INSTANTS, frame_fixture_name, instants_json};
use cua_spaces_transcript::{CastPlayer, FRAME_SCHEMA, render_frame_document};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../spaces-contract/fixtures/transcript")
        .canonicalize()
        .expect("conformance fixtures are checked in")
}

#[test]
fn every_fixture_is_reproduced_byte_for_byte() {
    let root = fixtures();
    assert_eq!(INSTANTS.len(), 11);
    for (cast, instant, time_ms, purpose) in INSTANTS {
        let text =
            std::fs::read_to_string(root.join(format!("casts/{cast}.cast"))).expect("cast fixture");
        let actual = render_frame_document(&text, cast, instant, *time_ms).expect("render");
        let expected =
            std::fs::read_to_string(root.join("frames").join(frame_fixture_name(cast, instant)))
                .expect("frame fixture");
        assert_eq!(actual, expected, "{cast}@{instant} ({purpose})");
    }
}

#[test]
fn the_instant_index_is_the_generated_one() {
    let expected = std::fs::read_to_string(fixtures().join("instants.json")).expect("index");
    assert_eq!(instants_json(), expected);
    assert!(expected.contains(FRAME_SCHEMA));
}

/// Rendering is a pure function of the bytes: the same cast rendered twice is
/// the same document, or a golden diff would be noise rather than signal.
#[test]
fn rendering_is_stable() {
    let root = fixtures();
    let text = std::fs::read_to_string(root.join("casts/claude-basic-turn.cast")).unwrap();
    let first = render_frame_document(&text, "claude-basic-turn", "complete", 13500).unwrap();
    let second = render_frame_document(&text, "claude-basic-turn", "complete", 13500).unwrap();
    assert_eq!(first, second);
}

/// A frame is the state *before* the chunk that straddles the requested time.
/// A half-applied chunk is a state the terminal never actually had.
#[test]
fn effective_time_never_exceeds_the_requested_time() {
    let root = fixtures();
    for (cast, _, time_ms, _) in INSTANTS {
        let text = std::fs::read_to_string(root.join(format!("casts/{cast}.cast"))).unwrap();
        let player = CastPlayer::parse(&text).unwrap();
        let frame = player.frame(*time_ms as f64 / 1000.0);
        assert!(frame.effective_time <= frame.requested_time, "{cast}");
    }
}

#[test]
fn a_malformed_cast_is_an_error() {
    assert!(CastPlayer::parse("not a cast\n").is_err());
    assert!(CastPlayer::parse("").is_err());
    assert!(CastPlayer::parse("{\"version\":1,\"width\":80,\"height\":24}\n").is_err());
}
