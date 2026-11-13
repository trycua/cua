// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One ScreenCaptureKit user per unbundled executable.
//!
//! `replayd` names a client by its bundle identifier, and one that has none
//! by its executable path. When a second process started from that path
//! connects, `replayd` cancels the first one's connection; ReplayKit in the
//! first reconnects at once, which cancels the second one's, and so on for as
//! long as both live.
//!
//! Measured on macOS 26.7 (M3) with two daemons from one binary: about 1,000
//! reconnects a second, `replayd` at 60 to 77% of a core, each daemon at 10 to
//! 14% and growing 0.5 to 0.9 MB a second while idle, and every
//! ScreenCaptureKit call of both timing out. Two processes whose executable
//! is an app's `Contents/MacOS` one do not collide; a binary elsewhere inside
//! an app (`Contents/Resources`, `Contents/Helpers`) does.
//!
//! So such a process claims its executable path before its first
//! ScreenCaptureKit call and keeps the claim for life, as ReplayKit keeps its
//! connection. A process that finds the path claimed never calls
//! ScreenCaptureKit; its captures go through `screencapture`.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// `Ok` when this process may call ScreenCaptureKit. The error names the
/// process that holds the executable's claim.
pub fn claim() -> anyhow::Result<()> {
    static LOCK_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    static HELD: Mutex<Option<File>> = Mutex::new(None);

    let Some(lock_path) = LOCK_PATH.get_or_init(own_lock_path) else {
        return Ok(());
    };
    let mut held = HELD.lock().unwrap_or_else(|poison| poison.into_inner());
    if held.is_some() {
        return Ok(());
    }
    match try_claim(lock_path) {
        Ok(file) => {
            *held = Some(file);
            Ok(())
        }
        Err(Refusal::Held { pid }) => {
            let holder =
                pid.map_or_else(|| "another process".to_owned(), |pid| format!("pid {pid}"));
            anyhow::bail!(
                "ScreenCaptureKit is in use by {holder} started from this executable; macOS \
                 serves it to one process per executable outside an app bundle"
            )
        }
        Err(Refusal::Unavailable(error)) => {
            tracing::debug!(
                "ScreenCaptureKit claim unavailable at {}: {error}",
                lock_path.display()
            );
            Ok(())
        }
    }
}

#[derive(Debug)]
enum Refusal {
    Held { pid: Option<u32> },
    Unavailable(std::io::Error),
}

fn try_claim(lock_path: &Path) -> Result<File, Refusal> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(Refusal::Unavailable)?;
    match file.try_lock() {
        Ok(()) => {
            let _ = file.set_len(0);
            let _ = file.seek(SeekFrom::Start(0));
            let _ = write!(file, "{}", std::process::id());
            Ok(file)
        }
        Err(TryLockError::WouldBlock) => {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            Err(Refusal::Held {
                pid: text.trim().parse().ok(),
            })
        }
        Err(TryLockError::Error(error)) => Err(Refusal::Unavailable(error)),
    }
}

/// Where this process claims its executable; `None` inside an app bundle or
/// when either path cannot be resolved.
fn own_lock_path() -> Option<PathBuf> {
    let executable = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    if crate::capture_consent::app_bundle_of(&executable).is_some() {
        return None;
    }
    Some(user_cache_dir()?.join(lock_file_name(&executable)))
}

fn lock_file_name(executable: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    // FNV-1a: the same name in every build that runs from this path.
    let hash = executable
        .as_os_str()
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!("cua-driver-screencapturekit-{hash:016x}.lock")
}

/// The per-user cache directory macOS assigns, the same for every process of
/// the user whatever its environment.
fn user_cache_dir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt as _;
    let mut buffer = vec![0_u8; libc::PATH_MAX as usize];
    // SAFETY: `buffer` is writable for the length passed.
    let written = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_CACHE_DIR,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    if written == 0 || written > buffer.len() {
        return None;
    }
    buffer.truncate(written - 1);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claimed_executable_refuses_a_second_claim_until_the_first_is_released() {
        let directory = std::env::temp_dir().join(format!("cua-claim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let lock_path = directory.join(lock_file_name(Path::new("/opt/tool/bin/cua-driver")));

        let first = try_claim(&lock_path).expect("first claim");
        assert!(matches!(
            try_claim(&lock_path),
            Err(Refusal::Held { pid: Some(pid) }) if pid == std::process::id()
        ));
        drop(first);
        assert!(try_claim(&lock_path).is_ok());

        let _ = std::fs::remove_dir_all(&directory);
    }
}
