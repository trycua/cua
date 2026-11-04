//! The bulk lane's disk on the host side: a sparse raw file next to a Linux
//! VM's other files, attached to the VM as a second virtio-blk disk. The
//! guest's cua-spacesd and the host's client move file content through it
//! (see `cua_proto::bulk` for the layout), so a transfer between the two
//! never crosses the user-mode network, the slowest part of a local VM.

use std::path::{Path, PathBuf};

use crate::error::Result;

/// Serial of the virtio-blk device. Equal to `cua_proto::bulk::SERIAL`
/// (a test in `cua-daemon` holds them together).
pub const SERIAL: &str = "cuabulk";
/// Size of the disk file. Equal to `cua_proto::bulk::DISK_BYTES`.
pub const DISK_BYTES: u64 = (1 << 20) + 4 * (16 << 20);
/// File name inside a VM's directory.
pub const FILE: &str = "bulk.img";

/// Whether new VMs get the lane. `CUA_BULK_LANE=off` turns it off.
pub fn enabled() -> bool {
    !matches!(
        std::env::var("CUA_BULK_LANE").ok().as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

/// Creates (or resets) the disk file for the VM whose files live in `dir`:
/// private to the user, zeroed, so the guest writes a fresh header.
pub fn prepare(dir: &Path) -> Result<PathBuf> {
    let path = dir.join(FILE);
    let _ = std::fs::remove_file(&path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let f = o.open(&path)?;
    f.set_len(DISK_BYTES)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_makes_a_private_sparse_file() {
        let d = tempfile::tempdir().unwrap();
        let p = prepare(d.path()).unwrap();
        let m = std::fs::metadata(&p).unwrap();
        assert_eq!(m.len(), DISK_BYTES);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(m.permissions().mode() & 0o077, 0);
        }
        // Preparing again resets it.
        std::fs::write(&p, b"stale").unwrap();
        let p = prepare(d.path()).unwrap();
        assert_eq!(std::fs::metadata(p).unwrap().len(), DISK_BYTES);
    }
}
