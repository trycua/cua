// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Embeds `assets/` (the built web app, committed; `web/` rebuilds it) as a
//! sorted table of `(path, bytes)`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, root, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    assert!(
        files.iter().any(|(p, _)| p == "index.html"),
        "assets/index.html is missing: run `pnpm build` in web/"
    );
    let mut code = String::from(
        "/// Embedded viewer files, sorted by path.\npub static ASSETS: &[(&str, &[u8])] = &[\n",
    );
    for (rel, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        writeln!(
            code,
            "    ({rel:?}, include_bytes!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("assets.rs");
    std::fs::write(out, code).unwrap();
}
