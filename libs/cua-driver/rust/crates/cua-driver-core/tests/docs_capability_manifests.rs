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

fn is_manifest(body: &str) -> bool {
    let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str(body) else {
        return false;
    };
    map.keys()
        .filter_map(serde_yaml_ng::Value::as_str)
        .any(|key| MANIFEST_KEYS.contains(&key))
}

/// YAML fences that are capability manifests, as (line, body).
fn manifest_fences(text: &str) -> Vec<(usize, String)> {
    let mut fences = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        match open.as_mut() {
            None if trimmed.starts_with("```yaml") => open = Some((index + 1, String::new())),
            Some(_) if trimmed.starts_with("```") => {
                let (start, body) = open.take().unwrap();
                if is_manifest(&body) {
                    fences.push((start, body));
                }
            }
            Some((_, body)) => {
                body.push_str(line);
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
