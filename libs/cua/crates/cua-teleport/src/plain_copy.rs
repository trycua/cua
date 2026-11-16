// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Data-only file copies for browser databases.
//!
//! `std::fs::copy` on macOS clones or `fcopyfile`s the source, which carries
//! its extended attributes (`com.apple.provenance`, `com.apple.quarantine`),
//! ACLs and flags to the destination. Newer macOS refuses that metadata write
//! with EPERM for files a browser owns, even when a plain read succeeds. The
//! export only needs the bytes, so these copies open, read and write.

use std::fs::File;
use std::io;
use std::path::Path;

use crate::TeleportError;

/// Copies the bytes of `src` to a new file at `dst`, carrying no xattrs, ACLs
/// or flags.
pub(crate) fn copy_data_only(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut from = File::open(src)?;
    let mut to = File::create(dst)?;
    let n = io::copy(&mut from, &mut to)?;
    to.sync_all()?;
    Ok(n)
}

/// [`copy_data_only`] with an error that names the file and, for a denied
/// read, the permission that usually fixes it.
pub(crate) fn copy_named(what: &str, src: &Path, dst: &Path) -> Result<u64, TeleportError> {
    copy_data_only(src, dst).map_err(|e| {
        let hint = if e.kind() == io::ErrorKind::PermissionDenied {
            ". Cua could not read it: grant Full Disk Access to cua-spacesd \
             (System Settings > Privacy & Security), then retry"
        } else {
            ""
        };
        TeleportError::Provider(format!(
            "copying {what} failed: {e} ({}){hint}",
            src.display()
        ))
    })
}
