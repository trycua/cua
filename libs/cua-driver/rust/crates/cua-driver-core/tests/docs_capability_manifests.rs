// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every capability manifest the user docs show loads through the driver's
//! own loader and starts a bounded daemon, so a docs example cannot drift
//! from the manifest format the driver accepts.

use std::path::{Path, PathBuf};

use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::session_manifest::load_manifest;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(5)
        .expect("repository root")
        .to_path_buf()
}

fn collect_mdx(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_mdx(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "mdx") {
            files.push(path);
        }
    }
}

/// Top-level keys only a capability manifest has. Policy files use just
/// `allow` and `deny`.
const MANIFEST_KEYS: &[&str] = &[
    "version",
    "mode",
    "expires_after",
    "idle_timeout",
    "resources",
];

/// Read from the unindented key lines rather than a parse, so an example
/// that does not parse still reaches the loader and fails there.
fn is_manifest(body: &str) -> bool {
    body.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#', '-']))
        .filter_map(|line| line.split_once(':'))
        .any(|(key, _)| MANIFEST_KEYS.contains(&key.trim().trim_matches(['"', '\''])))
}

/// YAML fences that are capability manifests, as (line, body). A fence
/// indented under a list item has that indent removed from its body.
fn manifest_fences(text: &str) -> Vec<(usize, String)> {
    let mut fences = Vec::new();
    let mut open: Option<(usize, usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        match open.as_mut() {
            None if trimmed.starts_with("```yaml") => {
                open = Some((index + 1, line.len() - trimmed.len(), String::new()))
            }
            Some(_) if trimmed.starts_with("```") => {
                let (start, _, body) = open.take().unwrap();
                if is_manifest(&body) {
                    fences.push((start, body));
                }
            }
            Some((_, indent, body)) => {
                let strip = line.len() - line.trim_start().len();
                body.push_str(&line[strip.min(*indent)..]);
                body.push('\n');
            }
            None => {}
        }
    }
    fences
}

#[test]
fn documented_capability_manifests_load_in_bounded_mode() {
    let docs = repo_root().join("docs/content/docs/cua-driver");
    // Nix tests the filtered Rust source, which excludes the docs tree.
    if !docs.is_dir() {
        return;
    }
    let mut files = Vec::new();
    collect_mdx(&docs, &mut files);

    let scratch = std::env::temp_dir().join(format!("cua-docs-manifests-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let mut checked = 0;
    let mut failures = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        for (line, body) in manifest_fences(&text) {
            let file = scratch.join(format!("manifest-{checked}.yaml"));
            std::fs::write(&file, body).unwrap();
            checked += 1;
            let result = load_manifest(&file)
                .and_then(|manifest| manifest.validate_for_mode(PermissionMode::Bounded));
            if let Err(error) = result {
                failures.push(format!("{}:{line}: {error}", path.display()));
            }
        }
    }
    std::fs::remove_dir_all(&scratch).ok();

    assert!(
        checked > 0,
        "no capability manifest examples under {}",
        docs.display()
    );
    assert!(
        failures.is_empty(),
        "documented capability manifests the driver rejects:\n{}",
        failures.join("\n")
    );
}

#[test]
fn selector_finds_nested_quoted_and_unparseable_manifests() {
    let page = concat!(
        "```yaml\nallow:\n  tools: [click]\n```\n",
        "1. Write the manifest:\n\n   ```yaml\n   version: 4\n   allow:\n     tools: [click]\n   ```\n",
        "```yaml\n\"version\": 3\n```\n",
        "```yaml\nversion: 3\nversion: 3\n```\n",
    );
    let found: Vec<_> = manifest_fences(page)
        .into_iter()
        .map(|(line, body)| (line, body.lines().next().unwrap().to_owned()))
        .collect();
    assert_eq!(
        found,
        [
            (7, "version: 4".to_owned()),
            (12, "\"version\": 3".to_owned()),
            (15, "version: 3".to_owned()),
        ]
    );
}
