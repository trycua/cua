
//! This machine's memory and disk in whole GB, to show next to the limits a
//! host sets for the Spaces others create on it. 0 when the system does not
//! say.

use std::path::Path;

const GB: u64 = 1024 * 1024 * 1024;

/// Total physical memory in GB (rounded to the nearest).
pub fn memory() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: sysconf only reads system constants.
        let (pages, size) = unsafe {
            (
                libc::sysconf(libc::_SC_PHYS_PAGES),
                libc::sysconf(libc::_SC_PAGESIZE),
            )
        };
        if pages > 0 && size > 0 {
            let bytes = pages as u64 * size as u64;
            return ((bytes + GB / 2) / GB) as u32;
        }
    }
    0
}

/// Total size of the disk `path` is on, in GB.
pub fn disk(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        if let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) {
            // SAFETY: `c` is a valid C string and `s` is written by statvfs.
            let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0 {
                let bytes = s.f_blocks as u64 * s.f_frsize as u64;
                return (bytes / GB) as u32;
            }
        }
    }
    let _ = path;
    0
}
