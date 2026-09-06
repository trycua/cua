//! Published bounded-manifest examples must stay loadable by the runtime parser.
//!
//! Three published examples once drifted from the schema and were refused by
//! the daemon at startup (#2847). Validating every full manifest example in
//! the docs through the real `load_manifest` path makes that class of drift
//! fail in CI instead of in a user's terminal (#2572).
#![cfg(feature = "yaml")]

use std::io::Write;
use std::path::{Path, PathBuf};

use cua_driver_core::session_manifest::load_manifest;

/// Every docs page that publishes at least one complete session manifest.
/// A page that moves or stops carrying a full example should update this
/// list in the same change.
const MANIFEST_DOC_PAGES: &[&str] = &[
    "docs/content/docs/how-to-guides/driver/drive-a-web-page.mdx",
    "docs/content/docs/how-to-guides/driver/write-a-bounded-manifest.mdx",
    "docs/content/docs/reference/cua-driver/permission-modes.mdx",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("crate directory should sit five levels below the repo root")
        .to_path_buf()
}

/// Fenced ```yaml blocks that declare a complete manifest: partial snippets
/// (a lone `resources:` section, for example) document one field and are not
/// expected to load on their own.
fn full_manifest_blocks(source: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in source.lines() {
        match current.as_mut() {
            None => {
                let fence = line.trim_end();
                if fence == "```yaml" || fence == "```yml" {
                    current = Some(Vec::new());
                }
            }
            Some(lines) => {
                if line.trim_end() == "```" {
                    let block = lines.join("\n");
                    let declares_version = lines.iter().any(|l| l.starts_with("version:"));
                    let declares_mode = lines.iter().any(|l| l.starts_with("mode:"));
                    if declares_version && declares_mode {
                        blocks.push(block);
                    }
                    current = None;
                } else {
                    lines.push(line);
                }
            }
        }
    }
    blocks
}

// Ignored by default because the docs tree is outside the Nix build sandbox
// (flake.nix's rustTestSrc roots at libs/cua-driver); ci-rust-linux.yml runs
// it explicitly from the full checkout, like the other `-- --ignored` suites.
#[test]
#[ignore = "requires a full repo checkout (reads docs/); run with -- --ignored"]
fn published_manifest_examples_load() {
    let root = repo_root();
    let mut failures = Vec::new();
    for page in MANIFEST_DOC_PAGES {
        let path = root.join(page);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {page}: {error}"));
        let manifests = full_manifest_blocks(&source);
        assert!(
            !manifests.is_empty(),
            "{page} no longer contains a complete manifest example; \
             update MANIFEST_DOC_PAGES if it moved"
        );
        for (index, manifest) in manifests.iter().enumerate() {
            let mut file = tempfile::NamedTempFile::new().expect("create temp manifest");
            file.write_all(manifest.as_bytes())
                .expect("write temp manifest");
            if let Err(error) = load_manifest(file.path()) {
                failures.push(format!("{page} (manifest example {index}): {error}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "published manifest examples no longer load:\n{}",
        failures.join("\n")
    );
}
