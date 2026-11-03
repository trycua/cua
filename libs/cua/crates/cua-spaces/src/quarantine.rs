//! Marks what a Space hands to this computer as downloaded from elsewhere,
//! so the system asks before it opens: `com.apple.quarantine` on macOS and
//! the `Zone.Identifier` stream (zone 3, the internet) on Windows. Nothing
//! on Linux has a matching notion.

use std::path::Path;

/// The macOS extended attribute Gatekeeper reads.
pub const MACOS_ATTRIBUTE: &str = "com.apple.quarantine";

/// The macOS attribute value: flags, the time in hex Unix seconds, the
/// agent that downloaded it. `0081` is what browsers write for a download
/// the user started.
pub fn macos_value(unix_seconds: u64) -> String {
    format!("0081;{unix_seconds:x};Cua;")
}

/// The Windows `Zone.Identifier` stream content for the internet zone.
pub fn windows_zone_identifier() -> &'static str {
    "[ZoneTransfer]\r\nZoneId=3\r\n"
}

#[cfg(target_os = "macos")]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Marks one downloaded file. Returns whether this platform has a
/// quarantine mark (and it was set); `Ok(false)` where none exists.
pub fn mark(path: &Path) -> std::io::Result<bool> {
    mark_impl(path)
}

#[cfg(target_os = "macos")]
fn mark_impl(path: &Path) -> std::io::Result<bool> {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn setxattr(
            path: *const c_char,
            name: *const c_char,
            value: *const c_void,
            size: usize,
            position: u32,
            options: c_int,
        ) -> c_int;
    }
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let name = CString::new(MACOS_ATTRIBUTE).expect("no NUL");
    let value = macos_value(now());
    // SAFETY: both strings are NUL-terminated and outlive the call, and the
    // value pointer and length describe `value`.
    let rc = unsafe {
        setxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if rc == 0 {
        Ok(true)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn mark_impl(path: &Path) -> std::io::Result<bool> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    std::fs::write(stream, windows_zone_identifier())?;
    Ok(true)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn mark_impl(_: &Path) -> std::io::Result<bool> {
    Ok(false)
}

/// Marks every regular file under `root` (or `root` itself when it is a
/// file). Returns how many files were marked. Failing to mark one does not
/// stop the rest; the first error is returned after all were tried.
pub fn mark_tree(root: &Path) -> std::io::Result<usize> {
    let mut marked = 0;
    let mut first_error = None;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        } else if meta.is_file() {
            match mark(&path) {
                Ok(true) => marked += 1,
                Ok(false) => {}
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(marked),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macos_value_has_flags_time_and_agent() {
        assert_eq!(macos_value(0x6500_0000), "0081;65000000;Cua;");
        assert!(windows_zone_identifier().contains("ZoneId=3"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_marked_file_carries_the_quarantine_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("b.txt"), b"y").unwrap();
        assert_eq!(mark_tree(dir.path()).unwrap(), 2);
        for f in [file, sub.join("b.txt")] {
            let out = std::process::Command::new("xattr")
                .args(["-p", MACOS_ATTRIBUTE])
                .arg(&f)
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.starts_with("0081;"), "{f:?}: {text}");
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn other_platforms_mark_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(!mark(&file).unwrap());
        assert_eq!(mark_tree(dir.path()).unwrap(), 0);
    }
}
