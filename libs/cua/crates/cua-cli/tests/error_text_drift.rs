//! Drift guard: user-facing error text must not carry the source tree or
//! Rust debug structs. A released binary has no `libs/cua/crates` to point
//! at, and a `{ source: ... }` dump says nothing a person can act on.
//!
//! The runtime half is `cua_sdk::report` (it scrubs what slips through);
//! this half fails the build when a string literal in shipped code names a
//! source path, so the message is fixed where it is written.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name()
                .is_some_and(|n| n == "tests" || n == "target" || n == "node_modules")
            {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Source-tree paths and struct-dump markers that must not be inside a
/// string literal of shipped code.
const FORBIDDEN: &[&str] = &[
    "libs/cua/crates",
    "/crates/cua-",
    "DisplayErrorContext(&e).to_string()",
];

#[test]
fn no_string_literal_in_shipped_code_names_the_source_tree() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap().flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    assert!(files.len() > 100, "found {} files", files.len());
    let mut bad = Vec::new();
    for f in files {
        // The report module and this test list the patterns on purpose.
        if f.ends_with("cua-sdk/src/report.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        // Everything after the first `#[cfg(test)]` is test code.
        let shipped = text.split("#[cfg(test)]").next().unwrap();
        for (n, line) in shipped.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.contains('"')
                && FORBIDDEN
                    .iter()
                    .any(|p| t.contains(p) && *p != "DisplayErrorContext(&e).to_string()")
            {
                bad.push(format!("{}:{}: {}", f.display(), n + 1, t));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "user-facing text names the source tree:\n{}",
        bad.join("\n")
    );
}
