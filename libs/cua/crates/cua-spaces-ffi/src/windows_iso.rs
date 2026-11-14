// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Choose Windows ISO…": the Windows ISO a person picked, remembered for the
//! image that is built from it (what `cua sb create windows:11 --iso <path>`
//! does), so the create that follows installs from that file.

use std::path::Path;

/// What [`windows_use_iso`] answers.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WindowsIsoChoice {
    /// The file is usable and remembered.
    pub ok: bool,
    /// The file's name when `ok`.
    pub name: String,
    /// Why the file cannot be used, in plain words; empty when `ok`.
    pub problem: String,
}

/// Checks the ISO at `path` for the Windows image `image` and remembers it
/// for that image's next build.
#[uniffi::export]
pub fn windows_use_iso(image: String, path: String) -> WindowsIsoChoice {
    use cua_image::windows::build::{Layout, use_user_iso};
    match use_user_iso(&Layout::default_root(), &image, Path::new(&path)) {
        Ok(name) => WindowsIsoChoice {
            ok: true,
            name,
            problem: String::new(),
        },
        Err(problem) => WindowsIsoChoice {
            ok: false,
            name: String::new(),
            problem,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_is_not_an_iso_is_refused_in_plain_words() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("notes.txt");
        std::fs::write(&f, b"hello").unwrap();
        let r = windows_use_iso(
            "ghcr.io/trycua/windows:11".into(),
            f.to_string_lossy().into_owned(),
        );
        assert!(!r.ok && r.name.is_empty());
        assert!(
            r.problem.contains("not a Windows disc image"),
            "{}",
            r.problem
        );
        let r = windows_use_iso("ghcr.io/trycua/linux:24.04".into(), "x.iso".into());
        assert_eq!(r.problem, "This image does not install from a Windows ISO.");
    }
}
