// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Extended attributes as string metadata on files.
//!
//! A write stores `metadata` as extended attributes whose names are the
//! request's `xattr_namespace` (default [`DEFAULT_NAMESPACE`]) followed by
//! the key; reads report the attributes under that prefix with the prefix
//! removed. Limits (enforced here, documented in `filesystem.proto`):
//! [`MAX_KEYS`] keys, keys of 1 to [`MAX_KEY_BYTES`] bytes of ASCII letters,
//! digits and ``._:/@+-``, values of at most [`MAX_VALUE_BYTES`] bytes, and
//! [`MAX_TOTAL_BYTES`] for all keys and values together. The namespace must
//! start with `user.`, end with `.` and be at most [`MAX_NAMESPACE_BYTES`]
//! bytes of letters, digits and ``._-``.
//!
//! Linux and macOS: elsewhere a write with metadata is refused as
//! unsupported and reads report none.

use std::collections::HashMap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::Path;

use cua_proto::env::v1::ErrorReason;
use tonic::{Code, Status};

use crate::error::StatusBuilder;

/// Namespace used when a request names none.
pub const DEFAULT_NAMESPACE: &str = "user.cua.meta.";
/// Most keys on one file.
pub const MAX_KEYS: usize = 32;
/// Longest key.
pub const MAX_KEY_BYTES: usize = 128;
/// Longest value.
pub const MAX_VALUE_BYTES: usize = 1024;
/// Keys plus values of one write.
pub const MAX_TOTAL_BYTES: usize = 3072;
/// Longest namespace prefix.
pub const MAX_NAMESPACE_BYTES: usize = 64;
/// Values larger than this are never read back (a foreign attribute).
#[cfg(any(target_os = "linux", target_os = "macos"))]
const MAX_READ_BYTES: usize = 64 * 1024;

fn invalid(message: String) -> Status {
    StatusBuilder::new(Code::InvalidArgument, ErrorReason::Unspecified, message).build()
}

/// The namespace to use for a request value (`""` is the default).
pub fn namespace(requested: &str) -> Result<String, Status> {
    if requested.is_empty() {
        return Ok(DEFAULT_NAMESPACE.to_owned());
    }
    let ok = requested.len() <= MAX_NAMESPACE_BYTES
        && requested.starts_with("user.")
        && requested.ends_with('.')
        && requested
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(requested.to_owned())
    } else {
        Err(invalid(format!(
            "xattr_namespace {requested:?} must start with \"user.\", end with \".\" and use at most \
             {MAX_NAMESPACE_BYTES} bytes of letters, digits and \"._-\""
        )))
    }
}

/// Checks `metadata` against the documented limits.
pub fn validate(metadata: &HashMap<String, String>) -> Result<(), Status> {
    if metadata.len() > MAX_KEYS {
        return Err(invalid(format!(
            "metadata has {} keys; at most {MAX_KEYS}",
            metadata.len()
        )));
    }
    let mut total = 0usize;
    for (key, value) in metadata {
        let key_ok = !key.is_empty()
            && key.len() <= MAX_KEY_BYTES
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:/@+-".contains(&b));
        if !key_ok {
            return Err(invalid(format!(
                "metadata key {key:?} must be 1 to {MAX_KEY_BYTES} bytes of letters, digits and \"._:/@+-\""
            )));
        }
        if value.len() > MAX_VALUE_BYTES {
            return Err(invalid(format!(
                "metadata value of {key:?} is {} bytes; at most {MAX_VALUE_BYTES}",
                value.len()
            )));
        }
        total += key.len() + value.len();
    }
    if total > MAX_TOTAL_BYTES {
        return Err(invalid(format!(
            "metadata is {total} bytes; at most {MAX_TOTAL_BYTES}"
        )));
    }
    Ok(())
}

/// What a write stores: the namespace and validated attributes.
#[derive(Debug, Clone, Default)]
pub struct Attributes {
    /// Full prefix (for example `user.cua.meta.`).
    pub namespace: String,
    /// Key to value, without the prefix.
    pub values: HashMap<String, String>,
}

impl Attributes {
    /// Validates a request's `xattr_namespace` and `metadata`.
    pub fn from_request(
        xattr_namespace: &str,
        metadata: &HashMap<String, String>,
    ) -> Result<Self, Status> {
        validate(metadata)?;
        Ok(Self {
            namespace: namespace(xattr_namespace)?,
            values: metadata.clone(),
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn c_name(full: &str) -> std::io::Result<std::ffi::CString> {
    std::ffi::CString::new(full).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn c_path(path: &Path) -> std::io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod sys {
    use std::ffi::CStr;
    use std::io;
    use std::os::fd::RawFd;

    fn check(rc: isize) -> io::Result<usize> {
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(rc as usize)
        }
    }

    fn out_ptr(buf: &mut [u8]) -> *mut libc::c_void {
        if buf.is_empty() {
            std::ptr::null_mut()
        } else {
            buf.as_mut_ptr().cast()
        }
    }

    #[cfg(target_os = "linux")]
    pub fn fset(fd: RawFd, name: &CStr, value: &[u8]) -> io::Result<()> {
        // SAFETY: valid fd, NUL-terminated name, value pointer and length.
        let rc =
            unsafe { libc::fsetxattr(fd, name.as_ptr(), value.as_ptr().cast(), value.len(), 0) };
        check(rc as isize).map(|_| ())
    }

    #[cfg(target_os = "macos")]
    pub fn fset(fd: RawFd, name: &CStr, value: &[u8]) -> io::Result<()> {
        // SAFETY: valid fd, NUL-terminated name, value pointer and length.
        let rc =
            unsafe { libc::fsetxattr(fd, name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0) };
        check(rc as isize).map(|_| ())
    }

    #[cfg(target_os = "linux")]
    pub fn list(path: &CStr, follow: bool, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len();
        let ptr = out_ptr(buf).cast();
        // SAFETY: `ptr` is null or valid for `len` bytes.
        let rc = unsafe {
            if follow {
                libc::listxattr(path.as_ptr(), ptr, len)
            } else {
                libc::llistxattr(path.as_ptr(), ptr, len)
            }
        };
        check(rc as isize)
    }

    #[cfg(target_os = "macos")]
    pub fn list(path: &CStr, follow: bool, buf: &mut [u8]) -> io::Result<usize> {
        let options = if follow { 0 } else { libc::XATTR_NOFOLLOW };
        let len = buf.len();
        let ptr = out_ptr(buf).cast();
        // SAFETY: `ptr` is null or valid for `len` bytes.
        let rc = unsafe { libc::listxattr(path.as_ptr(), ptr, len, options) };
        check(rc as isize)
    }

    #[cfg(target_os = "linux")]
    pub fn get(path: &CStr, follow: bool, name: &CStr, buf: &mut [u8]) -> io::Result<usize> {
        let len = buf.len();
        let ptr = out_ptr(buf);
        // SAFETY: `ptr` is null or valid for `len` bytes.
        let rc = unsafe {
            if follow {
                libc::getxattr(path.as_ptr(), name.as_ptr(), ptr, len)
            } else {
                libc::lgetxattr(path.as_ptr(), name.as_ptr(), ptr, len)
            }
        };
        check(rc as isize)
    }

    #[cfg(target_os = "macos")]
    pub fn get(path: &CStr, follow: bool, name: &CStr, buf: &mut [u8]) -> io::Result<usize> {
        let options = if follow { 0 } else { libc::XATTR_NOFOLLOW };
        let len = buf.len();
        let ptr = out_ptr(buf);
        // SAFETY: `ptr` is null or valid for `len` bytes.
        let rc = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), ptr, len, 0, options) };
        check(rc as isize)
    }
}

/// Stores `attributes` on the open file `fd`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn set_on_fd(fd: std::os::fd::RawFd, attributes: &Attributes) -> Result<(), Status> {
    for (key, value) in &attributes.values {
        let full = format!("{}{key}", attributes.namespace);
        let name = c_name(&full).map_err(|e| invalid(format!("metadata key {key:?}: {e}")))?;
        if let Err(error) = sys::fset(fd, &name, value.as_bytes()) {
            return Err(set_error(&error, &full));
        }
    }
    Ok(())
}

/// Stores `attributes` on the open file `fd`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn set_on_fd(_fd: i32, attributes: &Attributes) -> Result<(), Status> {
    if attributes.values.is_empty() {
        return Ok(());
    }
    Err(crate::error::unsupported(
        "xattr",
        "this platform has no extended attributes",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn set_error(error: &std::io::Error, name: &str) -> Status {
    match error.raw_os_error() {
        Some(code) if code == libc::ENOTSUP => StatusBuilder::new(
            Code::FailedPrecondition,
            ErrorReason::Unspecified,
            format!("the file system does not support extended attributes ({name})"),
        )
        .build(),
        Some(code) if code == libc::EPERM || code == libc::EACCES => StatusBuilder::new(
            Code::PermissionDenied,
            ErrorReason::PermissionDenied,
            format!("cannot set extended attribute {name}: {error}"),
        )
        .build(),
        _ => StatusBuilder::new(
            Code::Internal,
            ErrorReason::Unspecified,
            format!("cannot set extended attribute {name}: {error}"),
        )
        .build(),
    }
}

/// Reads the attributes of `path` under `namespace`, prefix removed.
/// Anything unreadable or unsupported reads as absent.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn read(path: &Path, namespace: &str, follow: bool) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(cpath) = c_path(path) else {
        return out;
    };
    let Ok(size) = sys::list(&cpath, follow, &mut []) else {
        return out;
    };
    if size == 0 {
        return out;
    }
    let mut names = vec![0u8; size];
    let Ok(size) = sys::list(&cpath, follow, &mut names) else {
        return out;
    };
    names.truncate(size);
    for raw in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let Ok(full) = std::str::from_utf8(raw) else {
            continue;
        };
        let Some(key) = full.strip_prefix(namespace).filter(|k| !k.is_empty()) else {
            continue;
        };
        let Ok(name) = c_name(full) else { continue };
        let Ok(len) = sys::get(&cpath, follow, &name, &mut []) else {
            continue;
        };
        if len > MAX_READ_BYTES {
            continue;
        }
        let mut value = vec![0u8; len];
        let Ok(len) = sys::get(&cpath, follow, &name, &mut value) else {
            continue;
        };
        value.truncate(len);
        out.insert(key.to_owned(), String::from_utf8_lossy(&value).into_owned());
    }
    out
}

/// Reads the attributes of `path` under `namespace`, prefix removed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read(_path: &std::path::Path, _namespace: &str, _follow: bool) -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn namespace_defaults_and_validates() {
        assert_eq!(namespace("").unwrap(), DEFAULT_NAMESPACE);
        assert_eq!(namespace("user.e2b.").unwrap(), "user.e2b.");
        for bad in [
            "e2b.",
            "user.e2b",
            "trusted.x.",
            "user.a b.",
            "user.\u{e9}.",
        ] {
            assert_eq!(
                namespace(bad).unwrap_err().code(),
                Code::InvalidArgument,
                "{bad}"
            );
        }
        let long = format!("user.{}.", "a".repeat(MAX_NAMESPACE_BYTES));
        assert!(namespace(&long).is_err());
    }

    #[test]
    fn limits_are_enforced() {
        assert!(validate(&map(&[("a.b-c_d:e/f@g+h", "v")])).is_ok());
        assert!(validate(&map(&[("", "v")])).is_err());
        assert!(validate(&map(&[("a=b", "v")])).is_err());
        assert!(validate(&map(&[("a b", "v")])).is_err());
        assert!(validate(&map(&[(&"k".repeat(MAX_KEY_BYTES + 1), "v")])).is_err());
        assert!(validate(&map(&[("k", &"v".repeat(MAX_VALUE_BYTES))])).is_ok());
        assert!(validate(&map(&[("k", &"v".repeat(MAX_VALUE_BYTES + 1))])).is_err());
        let many: HashMap<String, String> = (0..=MAX_KEYS)
            .map(|i| (format!("k{i}"), String::new()))
            .collect();
        assert!(validate(&many).is_err());
        let heavy: HashMap<String, String> = (0..4)
            .map(|i| (format!("k{i}"), "v".repeat(MAX_VALUE_BYTES)))
            .collect();
        assert!(validate(&heavy).is_err(), "total over the cap");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn round_trips_on_a_file_system_that_supports_it() {
        use std::os::fd::AsRawFd as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let file = std::fs::File::create(&path).unwrap();
        let attrs =
            Attributes::from_request("user.e2b.", &map(&[("owner", "\u{fc}n\u{ef}"), ("k2", "")]))
                .unwrap();
        match set_on_fd(file.as_raw_fd(), &attrs) {
            Ok(()) => {}
            // A tmpfs without user.* support: nothing to verify here.
            Err(status) if status.code() == Code::FailedPrecondition => {
                eprintln!("skipped: {}", status.message());
                return;
            }
            Err(status) => panic!("{status}"),
        }
        let got = read(&path, "user.e2b.", true);
        assert_eq!(got, map(&[("owner", "\u{fc}n\u{ef}"), ("k2", "")]));
        assert!(read(&path, "user.other.", true).is_empty());
        assert!(read(&path, DEFAULT_NAMESPACE, true).is_empty());
    }
}
