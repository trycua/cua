//! Writes the cross-language transcript conformance fixtures, or with
//! `--check` fails when the checked-in fixtures no longer match the Rust core.
//!
//! The fixtures are the proof. Rust writes them, and Swift and TypeScript
//! compare against them byte for byte through their UniFFI bindings; a
//! divergence in any language is a build failure rather than a footnote.

use cua_spaces_transcript::conformance::{
    INSTANTS, frame_fixture_name, instants_json, jsonui_fixture_name,
};
use cua_spaces_transcript::{render_frame_document, render_jsonui_document};
use std::path::PathBuf;

fn main() {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical libs/cua root")
        .join("spaces-contract/fixtures/transcript");
    let casts = root.join("casts");
    let frames = root.join("frames");
    let jsonui = root.join("jsonui");

    let mut planned: Vec<(PathBuf, String)> = vec![(root.join("instants.json"), instants_json())];
    for (cast, instant, time_ms, _) in INSTANTS {
        let path = casts.join(format!("{cast}.cast"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let document = render_frame_document(&text, cast, instant, *time_ms)
            .unwrap_or_else(|error| panic!("render {cast}@{instant}: {error}"));
        planned.push((frames.join(frame_fixture_name(cast, instant)), document));
        let structured = render_jsonui_document(&text, cast, instant, *time_ms)
            .unwrap_or_else(|error| panic!("parse {cast}@{instant}: {error}"));
        planned.push((jsonui.join(jsonui_fixture_name(cast, instant)), structured));
    }

    if check {
        let mut stale: Vec<String> = Vec::new();
        for (path, contents) in &planned {
            match std::fs::read_to_string(path) {
                Ok(existing) if existing == *contents => {}
                _ => stale.push(path.display().to_string()),
            }
        }
        if !stale.is_empty() {
            eprintln!("stale transcript fixtures:\n  {}", stale.join("\n  "));
            eprintln!("run: cargo run -p cua-spaces-transcript --bin cua-spaces-fixtures");
            std::process::exit(1);
        }
        println!(
            "transcript conformance fixtures are up to date ({}).",
            planned.len()
        );
        return;
    }

    std::fs::create_dir_all(&frames).expect("create frames directory");
    std::fs::create_dir_all(&jsonui).expect("create jsonui directory");
    for (path, contents) in &planned {
        std::fs::write(path, contents)
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
    println!("wrote {} transcript conformance fixtures.", planned.len());
}
