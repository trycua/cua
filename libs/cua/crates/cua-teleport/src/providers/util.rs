// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Small filesystem helpers shared by the export providers, so capture
//! plumbing stays identical across them. Reads only: everything they touch is
//! under a directory the provider resolved from its injected host's home.

use std::io::Write;
use std::path::Path;

use crate::Result;
use crate::bundle::BundleWriter;

/// Add one on-disk file to the bundle, skipping it (not failing) when it can't
/// be read — a locked database or an absent optional file never aborts a
/// transfer. Mode defaults to 0600 since these are all profile/credential data.
pub fn add_file_best_effort<W: Write>(
    writer: &mut BundleWriter<W>,
    disk: &Path,
    rel: &str,
) -> Result<()> {
    match std::fs::read(disk) {
        Ok(bytes) => writer.add_bytes(rel, 0o600, &bytes),
        Err(_) => Ok(()),
    }
}

/// Recursively add a directory to the bundle under `bundle_prefix`. Unreadable
/// entries are skipped best-effort. Symlinks are not followed (only regular
/// files and directories are packed).
pub fn add_dir_recursive<W: Write>(
    writer: &mut BundleWriter<W>,
    disk: &Path,
    bundle_prefix: &str,
) -> Result<()> {
    let entries = match std::fs::read_dir(disk) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_prefix = format!("{bundle_prefix}/{name}");
        // `is_dir`/`is_file` follow symlinks; `symlink_metadata` lets us skip
        // links so a profile symlink can't redirect the walk out of the tree.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            add_dir_recursive(writer, &path, &child_prefix)?;
        } else if meta.is_file() {
            add_file_best_effort(writer, &path, &child_prefix)?;
        }
    }
    Ok(())
}

/// Total size of a directory's regular files (best-effort, follows nothing).
pub fn dir_len(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => total += dir_len(&path),
                Ok(meta) if meta.is_file() => total += meta.len(),
                _ => {}
            }
        }
    }
    total
}

/// Size of one file, or 0 when it can't be stat'd.
pub fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}
