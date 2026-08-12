// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use cua_driver_contract::manifest;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("cua-contract-gen: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|arg| arg == "--check");
    let target = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("all");
    if !matches!(target, "all" | "manifest") {
        return Err("usage: cua-contract-gen [all|manifest] [--check]".into());
    }

    let mut contents = serde_json::to_string_pretty(&manifest()).map_err(|e| e.to_string())?;
    contents.push('\n');
    write_or_check(
        &driver_root()?.join("contract/manifest.json"),
        &contents,
        check,
    )
}

fn driver_root() -> Result<PathBuf, String> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "could not resolve libs/cua-driver root".into())
}

fn write_or_check(path: &Path, expected: &str, check: bool) -> Result<(), String> {
    if check {
        let actual = fs::read_to_string(path)
            .map_err(|error| format!("{} is missing: {error}", path.display()))?;
        if actual != expected {
            return Err(format!(
                "{} is stale; run `cargo run -p cua-driver-contract --bin cua-contract-gen -- all`",
                path.display()
            ));
        }
        println!("checked {}", path.display());
        return Ok(());
    }

    if path.is_symlink() {
        return Err(format!(
            "refusing to overwrite generated symlink {}",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::write(&temporary, expected).map_err(|error| error.to_string())?;

    if path.exists() {
        let backup = parent.join(format!(
            ".{}.bak.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        fs::rename(path, &backup).map_err(|error| error.to_string())?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::rename(&backup, path);
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        fs::remove_file(backup).map_err(|error| error.to_string())?;
    } else {
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    }
    println!("generated {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_output_is_pretty_and_newline_terminated() {
        let mut output = serde_json::to_string_pretty(&manifest()).expect("serialize manifest");
        output.push('\n');
        assert!(output.starts_with("{\n"));
        assert!(output.ends_with("\n"));
    }
}
