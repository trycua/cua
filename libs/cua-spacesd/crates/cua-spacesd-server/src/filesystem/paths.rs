// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Path resolution and `EntryInfo` construction.

use std::fs::Metadata;
use std::path::{Component, Path, PathBuf};

use cua_proto::env::v1::{EntryInfo, FileType};

use crate::util::timestamp;

/// Expands `~`, resolves relative paths against `workdir` (else `home`) and
/// normalizes `.`/`..` lexically (symlinks are not resolved).
pub fn expand(path: &str, home: &Path, workdir: Option<&Path>) -> PathBuf {
    let raw = if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            workdir.unwrap_or(home).join(p)
        }
    };
    normalize(&raw)
}

/// Lexical normalization: drops `.`, folds `..` (never above the root).
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    None | Some(Component::RootDir) | Some(Component::Prefix(_))
                ) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn file_type(meta: &Metadata) -> FileType {
    let ft = meta.file_type();
    if ft.is_symlink() {
        FileType::Symlink
    } else if ft.is_dir() {
        FileType::Directory
    } else if ft.is_file() {
        FileType::File
    } else {
        FileType::Other
    }
}

/// Builds an `EntryInfo` for `path` from `meta`.
pub fn entry_info(path: &Path, meta: &Metadata) -> EntryInfo {
    let kind = file_type(meta);
    let symlink_target = if kind == FileType::Symlink {
        std::fs::read_link(path)
            .map(|t| t.display().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    #[cfg(unix)]
    let (mode, owner, group) = {
        use std::os::unix::fs::MetadataExt as _;
        (
            meta.mode() & 0o7777,
            user_name(meta.uid()),
            group_name(meta.gid()),
        )
    };
    #[cfg(not(unix))]
    let (mode, owner, group) = {
        let base = if meta.is_dir() { 0o755 } else { 0o644 };
        let mode = if meta.permissions().readonly() {
            base & !0o222
        } else {
            base
        };
        (mode, String::new(), String::new())
    };
    EntryInfo {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        path: path.display().to_string(),
        r#type: kind as i32,
        size: if kind == FileType::Directory {
            0
        } else {
            meta.len()
        },
        mode,
        owner,
        group,
        modified_at: meta.modified().ok().map(timestamp),
        symlink_target,
    }
}

#[cfg(unix)]
fn user_name(uid: u32) -> String {
    use std::ffi::CStr;
    let mut buf = vec![0u8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: valid out-pointers; strings copied before `buf` drops.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return uid.to_string();
    }
    // SAFETY: getpwuid_r succeeded.
    unsafe { CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned() }
}

#[cfg(unix)]
fn group_name(gid: u32) -> String {
    use std::ffi::CStr;
    let mut buf = vec![0u8; 4096];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    // SAFETY: valid out-pointers; strings copied before `buf` drops.
    let rc = unsafe {
        libc::getgrgid_r(
            gid,
            &mut grp,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return gid.to_string();
    }
    // SAFETY: getgrgid_r succeeded.
    unsafe { CStr::from_ptr(grp.gr_name).to_string_lossy().into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_relative_and_dots() {
        let home = Path::new("/home/u");
        assert_eq!(expand("~", home, None), PathBuf::from("/home/u"));
        assert_eq!(expand("~/a/./b", home, None), PathBuf::from("/home/u/a/b"));
        assert_eq!(expand("x/../y", home, None), PathBuf::from("/home/u/y"));
        assert_eq!(
            expand("x", home, Some(Path::new("/work"))),
            PathBuf::from("/work/x")
        );
        assert_eq!(expand("/a/../../b", home, None), PathBuf::from("/b"));
    }
}
