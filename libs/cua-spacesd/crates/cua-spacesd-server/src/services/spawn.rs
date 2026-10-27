// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Files of the spawn channel (`TunnelService.StartSpawn`): where the guest
//! keeps the Space's spawn capability and the listener's address for
//! clients in the Space (the `cua` CLI, SDKs, the agent's tools).
//!
//! The files live in a directory only the user cua-spacesd runs as can
//! read: `/run/cua` when it may write there (a root driver), else a private
//! directory in the user's runtime dir or a RAM-backed temp dir. They are
//! never the proof of who calls: the host knows the Space by the WebSocket
//! it opened, and a capability copied into another Space (a snapshot, a
//! fork) is refused there.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// The capability's file name (`/run/cua/spawn-token` for a root driver).
pub const TOKEN_FILE: &str = cua_proto::metadata::SPAWN_TOKEN_FILE;
/// The listener address's file name (one line, `http://127.0.0.1:<port>`).
pub const ADDRESS_FILE: &str = cua_proto::metadata::SPAWN_ADDRESS_FILE;

/// Environment of new processes: the Space's id.
pub const ENV_SPACE: &str = cua_proto::metadata::ENV_SPACE;
/// Environment of new processes: the spawn listener's URL.
pub const ENV_ADDRESS: &str = cua_proto::metadata::ENV_SPAWN;
/// Environment of new processes: the capability's file.
pub const ENV_TOKEN_FILE: &str = cua_proto::metadata::ENV_SPAWN_TOKEN_FILE;

/// Directories tried in order; the first this process can own wins (the
/// same list clients look in, [`cua_proto::metadata::spawn_dirs`]).
pub fn candidate_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions.
    let uid = Some(unsafe { libc::getuid() });
    #[cfg(not(unix))]
    let uid = None;
    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    cua_proto::metadata::spawn_dirs(uid, xdg.as_deref(), &std::env::temp_dir())
}

/// The first usable directory of [`candidate_dirs`], created when missing.
pub fn choose_dir() -> std::io::Result<PathBuf> {
    let mut last = None;
    for dir in candidate_dirs() {
        match usable(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no directory for the spawn files")))
}

/// Creates `dir` (0700) or accepts an existing one this user owns that no
/// one else may write, then proves a file can be made in it.
fn usable(dir: &Path) -> std::io::Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() {
            return Err(std::io::Error::other(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        // Another user's directory, or one others may write into, could
        // swap the files under us.
        if meta.uid() != uid || meta.mode() & 0o022 != 0 {
            return Err(std::io::Error::other(format!(
                "{} is not private to this user",
                dir.display()
            )));
        }
    }
    let probe = dir.join(".spawn-probe");
    std::fs::write(&probe, b"")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Writes `contents` to `path` atomically with `mode` (Unix).
pub fn write_private(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", crate::util::random_id(6)));
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let mut f = options.open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The written files of one channel.
#[derive(Debug, Clone)]
pub struct SpawnFiles {
    /// The capability.
    pub token: PathBuf,
    /// The listener's address.
    pub address: PathBuf,
}

impl SpawnFiles {
    /// Writes the capability (0600) and the address (0600) into `dir`.
    pub fn write(dir: &Path, capability: &str, address: &str) -> std::io::Result<Self> {
        let files = Self {
            token: dir.join(TOKEN_FILE),
            address: dir.join(ADDRESS_FILE),
        };
        write_private(&files.token, capability.as_bytes(), 0o600)?;
        write_private(&files.address, format!("{address}\n").as_bytes(), 0o600)?;
        Ok(files)
    }

    /// Removes both files (missing is fine).
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.token);
        let _ = std::fs::remove_file(&self.address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_private_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let files = SpawnFiles::write(dir.path(), "cap", "http://127.0.0.1:1").unwrap();
        assert_eq!(std::fs::read_to_string(&files.token).unwrap(), "cap");
        assert_eq!(
            std::fs::read_to_string(&files.address).unwrap(),
            "http://127.0.0.1:1\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&files.token)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        files.remove();
        assert!(!files.token.exists() && !files.address.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_others_may_write_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(usable(&shared).is_err());
        let own = dir.path().join("own");
        usable(&own).unwrap();
        let mode = std::fs::metadata(&own).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
