//! Writes the cross-language control-plane conformance fixture, or with
//! `--check` fails when the checked-in fixture no longer matches the core.
//!
//! Same shape as `cua-spaces-fixtures` for the transcript slice: Rust writes
//! the expected document, and Swift and TypeScript compare against it byte for
//! byte through their UniFFI bindings. A divergence in any language is a build
//! failure rather than a footnote.

use std::path::PathBuf;

fn main() {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical libs/cua root")
        .join("spaces-contract/fixtures/control");

    let script = cua_spaces::client::conformance::session_script();
    let document = cua_spaces::client::conformance::run_session(&script)
        .unwrap_or_else(|error| panic!("run the conformance session: {error}"));
    let path = root.join("session.json");

    if check {
        match std::fs::read_to_string(&path) {
            Ok(existing) if existing == document => {
                println!("control-plane conformance fixture is up to date.");
            }
            _ => {
                eprintln!("stale control-plane fixture: {}", path.display());
                eprintln!("run: cargo run -p cua-spaces --bin cua-spaces-control-fixtures");
                std::process::exit(1);
            }
        }
        return;
    }

    std::fs::create_dir_all(&root).expect("create the control fixture directory");
    std::fs::write(&path, &document)
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    println!("wrote {}.", path.display());
}
