// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Whether an update replaced this process's executable on disk.
//!
//! An in-place update moves the old `CuaDriver.app` aside (or deletes it) and
//! puts the new bundle at the same path. A `serve` daemon that started from
//! the old bundle keeps running its old image, but macOS judges its TCC
//! grants against an install that is gone: permission probes report
//! Accessibility and Screen Recording off while System Settings shows them
//! on, and the gate never opens. Restarting from the live bundle is the only
//! cure, so the daemon exits once [`was_replaced`] says so (#7461).
//!
//! The running image is identified by the vnode the kernel mapped it from
//! (`PROC_PIDREGIONPATHINFO`), not by a path: `proc_pidpath` still reports
//! the launch path after the file was deleted, and a startup `stat` can race
//! the swap it is meant to detect.

use std::os::unix::fs::MetadataExt as _;

/// `PROC_PIDREGIONPATHINFO` from `<sys/proc_info.h>`.
const PROC_PIDREGIONPATHINFO: libc::c_int = 8;

/// `struct proc_regioninfo` from `<sys/proc_info.h>`.
#[allow(dead_code)]
#[repr(C)]
struct ProcRegionInfo {
    pri_protection: u32,
    pri_max_protection: u32,
    pri_inheritance: u32,
    pri_flags: u32,
    pri_offset: u64,
    pri_behavior: u32,
    pri_user_wired_count: u32,
    pri_user_tag: u32,
    pri_pages_resident: u32,
    pri_pages_shared_now_private: u32,
    pri_pages_swapped_out: u32,
    pri_pages_dirtied: u32,
    pri_ref_count: u32,
    pri_shadow_depth: u32,
    pri_share_mode: u32,
    pri_private_pages_resident: u32,
    pri_shared_pages_resident: u32,
    pri_obj_id: u32,
    pri_depth: u32,
    pri_address: u64,
    pri_size: u64,
}

/// `struct proc_regionwithpathinfo` from `<sys/proc_info.h>`.
#[repr(C)]
struct ProcRegionWithPathInfo {
    prp_prinfo: ProcRegionInfo,
    prp_vip: libc::vnode_info_path,
}

/// The file this process's code is mapped from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageFile {
    device: u32,
    inode: u64,
    links: u16,
}

/// `true` once the file at this process's launch path is no longer the image
/// it runs: the image was deleted, or moved aside and something else (or
/// nothing) now sits at that path. `false` when it is unchanged or cannot be
/// told.
pub fn was_replaced() -> bool {
    let Some(image) = running_image() else {
        return false;
    };
    let Ok(launch_path) = std::env::current_exe() else {
        return false;
    };
    judge(image, std::fs::metadata(launch_path))
}

fn judge(image: ImageFile, at_launch_path: std::io::Result<std::fs::Metadata>) -> bool {
    if image.links == 0 {
        return true;
    }
    match at_launch_path {
        Ok(metadata) => (metadata.dev() as u32, metadata.ino()) != (image.device, image.inode),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn running_image() -> Option<ImageFile> {
    // Any address inside this crate's code: it is linked into the executable.
    let address = was_replaced as usize as u64;
    let mut info = std::mem::MaybeUninit::<ProcRegionWithPathInfo>::zeroed();
    let size = std::mem::size_of::<ProcRegionWithPathInfo>() as libc::c_int;
    // SAFETY: `info` is a writable buffer of exactly `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            std::process::id() as libc::c_int,
            PROC_PIDREGIONPATHINFO,
            address,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if written != size {
        return None;
    }
    // SAFETY: the kernel filled all `size` bytes; every field is plain data.
    let info = unsafe { info.assume_init() };
    let region = &info.prp_prinfo;
    if address < region.pri_address || address - region.pri_address >= region.pri_size {
        return None;
    }
    let stat = &info.prp_vip.vip_vi.vi_stat;
    Some(ImageFile {
        device: stat.vst_dev,
        inode: stat.vst_ino,
        links: stat.vst_nlink,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{Duration, Instant};

    const CHILD_ENV: &str = "CUA_TEST_RUNNING_IMAGE_CHILD_DIR";

    fn image_of(path: &Path) -> ImageFile {
        let metadata = std::fs::metadata(path).unwrap();
        ImageFile {
            device: metadata.dev() as u32,
            inode: metadata.ino(),
            links: metadata.nlink() as u16,
        }
    }

    #[test]
    fn a_file_still_at_its_path_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cua-driver");
        std::fs::write(&path, b"old").unwrap();
        assert!(!judge(image_of(&path), std::fs::metadata(&path)));
    }

    #[test]
    fn a_new_file_swapped_in_over_the_path_is_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cua-driver");
        std::fs::write(&path, b"old").unwrap();
        let old = image_of(&path);
        std::fs::rename(&path, dir.path().join("backup")).unwrap();
        std::fs::write(&path, b"new").unwrap();
        assert!(judge(old, std::fs::metadata(&path)));
    }

    #[test]
    fn a_deleted_image_or_an_empty_path_is_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cua-driver");
        std::fs::write(&path, b"old").unwrap();
        let old = image_of(&path);
        assert!(judge(
            ImageFile { links: 0, ..old },
            std::fs::metadata(&path)
        ));
        std::fs::remove_file(&path).unwrap();
        assert!(judge(old, std::fs::metadata(&path)));
    }

    /// Runs only as the child of `the_running_process_sees_its_executable_swapped`.
    #[test]
    fn running_image_child() {
        let Some(dir) = std::env::var_os(CHILD_ENV) else {
            return;
        };
        let dir = Path::new(&dir);
        let before = was_replaced();
        std::fs::write(dir.join("started"), b"").unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !dir.join("swapped").exists() {
            assert!(Instant::now() < deadline, "parent never swapped the binary");
            std::thread::sleep(Duration::from_millis(20));
        }
        println!("before={before} after={}", was_replaced());
    }

    /// The real check on a real process: a copy of this test binary reports
    /// itself unchanged, then replaced once a new file is moved over its path
    /// the way an installer swaps the app bundle.
    #[test]
    fn the_running_process_sees_its_executable_swapped() {
        assert!(!was_replaced(), "the test binary itself is not replaced");
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("cua-driver");
        let test_binary = std::env::current_exe().unwrap();
        std::fs::copy(&test_binary, &binary).unwrap();
        let child = std::process::Command::new(&binary)
            .args([
                "--exact",
                "running_image::tests::running_image_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, dir.path())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !dir.path().join("started").exists() {
            assert!(Instant::now() < deadline, "child never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        let staged = dir.path().join("cua-driver.new");
        std::fs::copy(&test_binary, &staged).unwrap();
        std::fs::rename(&binary, dir.path().join("cua-driver.install-backup")).unwrap();
        std::fs::rename(&staged, &binary).unwrap();
        std::fs::write(dir.path().join("swapped"), b"").unwrap();
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("before=false after=true"), "{stdout}");
    }
}
