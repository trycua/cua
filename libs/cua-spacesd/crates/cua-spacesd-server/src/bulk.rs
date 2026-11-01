// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The guest end of the bulk lane: a disk the hosting machine shares with
//! this guest (virtio-blk, serial `cuabulk`) that carries file content for
//! `ReadFileRequest.bulk_slot` and `UploadChunkRequest.bulk_slot`, so a
//! same-machine transfer skips the guest network. The layout is
//! `cua_proto::bulk`. Linux only; elsewhere there is no lane.
//!
//! Everything goes through direct I/O: the guest's page cache never holds
//! the disk, so what the host client wrote is what a read returns, and what
//! the guest wrote is on the host file when the write completes.

use cua_proto::bulk::{Header, HEADER_BYTES};

/// A buffer aligned for direct I/O.
pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the buffer owns its allocation and is only reached through `&self`
// / `&mut self`.
unsafe impl Send for AlignedBuf {}
unsafe impl Sync for AlignedBuf {}

impl AlignedBuf {
    /// A zeroed buffer of `len` bytes (rounded up to a block).
    pub fn zeroed(len: usize) -> Self {
        let len = len.max(1).next_multiple_of(HEADER_BYTES);
        let layout = std::alloc::Layout::from_size_align(len, HEADER_BYTES).expect("layout");
        // SAFETY: non-zero size, valid layout.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null(), "out of memory");
        Self { ptr, len }
    }

    /// The bytes.
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` is valid for `len` bytes for as long as `self` lives.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// The bytes, writable.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: as above, and `&mut self` is exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        let layout = std::alloc::Layout::from_size_align(self.len, HEADER_BYTES).expect("layout");
        // SAFETY: allocated in `zeroed` with this layout.
        unsafe { std::alloc::dealloc(self.ptr, layout) };
    }
}

/// The bulk disk, open for direct I/O, with the header the guest wrote.
pub struct BulkDisk {
    #[cfg_attr(not(unix), allow(dead_code))]
    file: std::fs::File,
    header: Header,
}

/// Sub-chunk of one staging or fetching pass: bounds the buffer and lets
/// hashing overlap the device.
#[cfg(unix)]
const PASS_BYTES: usize = 4 << 20;

impl BulkDisk {
    /// The disk's header (what capabilities advertise).
    pub fn header(&self) -> &Header {
        &self.header
    }
}

#[cfg(not(unix))]
impl BulkDisk {
    /// No bulk lane off Unix.
    pub fn stage(
        &self,
        _: &std::fs::File,
        _: u64,
        _: u64,
        _: u32,
    ) -> std::io::Result<(u64, String)> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// No bulk lane off Unix.
    pub fn fetch(&self, _: u32, _: u64) -> std::io::Result<AlignedBuf> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

#[cfg(unix)]
impl BulkDisk {
    /// Copies `len` bytes of `src` from `offset` into slot `slot` and
    /// returns how many were copied (fewer only at the end of the file) with
    /// their SHA-256 (hex).
    pub fn stage(
        &self,
        src: &std::fs::File,
        offset: u64,
        len: u64,
        slot: u32,
    ) -> std::io::Result<(u64, String)> {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::FileExt;
        let base = self.slot_base(slot, len)?;
        let mut hasher = Sha256::new();
        let mut buf = AlignedBuf::zeroed(PASS_BYTES.min(len as usize));
        let mut done = 0u64;
        while done < len {
            let want = (len - done).min(buf.as_slice().len() as u64) as usize;
            let mut filled = 0;
            while filled < want {
                let n = src.read_at(
                    &mut buf.as_mut_slice()[filled..want],
                    offset + done + filled as u64,
                )?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            if filled == 0 {
                break;
            }
            hasher.update(&buf.as_slice()[..filled]);
            // Pad to the block: direct I/O writes whole blocks.
            let padded = filled.next_multiple_of(HEADER_BYTES);
            buf.as_mut_slice()[filled..padded].fill(0);
            self.file
                .write_all_at(&buf.as_slice()[..padded], base + done)?;
            done += filled as u64;
            if filled < want {
                break;
            }
        }
        Ok((done, hex::encode(hasher.finalize())))
    }

    /// The first `len` bytes of slot `slot`.
    pub fn fetch(&self, slot: u32, len: u64) -> std::io::Result<AlignedBuf> {
        use std::os::unix::fs::FileExt;
        let base = self.slot_base(slot, len)?;
        let mut buf = AlignedBuf::zeroed(len as usize);
        let padded = (len as usize).next_multiple_of(HEADER_BYTES);
        self.file
            .read_exact_at(&mut buf.as_mut_slice()[..padded], base)?;
        Ok(buf)
    }

    fn slot_base(&self, slot: u32, len: u64) -> std::io::Result<u64> {
        if slot >= self.header.slots || len > self.header.slot_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bulk slot or length out of range",
            ));
        }
        Ok(self.header.slot_offset(slot))
    }
}

/// Finds the bulk disk (by the serial of its virtio-blk device), opens it
/// and makes sure it carries a header: a disk that is blank, or was never
/// written by this guest, gets a fresh identity. `None` without a bulk disk
/// (or off Linux).
#[cfg(unix)]
pub fn open() -> Option<BulkDisk> {
    use std::os::unix::fs::FileExt;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::OpenOptionsExt;
    // `CUA_SPACESD_BULK_DEVICE` names the disk (a plain file works) for
    // tests; otherwise it is found by its serial and opened for direct I/O.
    let forced = std::env::var_os("CUA_SPACESD_BULK_DEVICE").map(std::path::PathBuf::from);
    let dev = match &forced {
        Some(p) => p.clone(),
        None => find_device()?,
    };
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true);
    #[cfg(target_os = "linux")]
    if forced.is_none() {
        o.custom_flags(libc::O_DIRECT);
    }
    let file = o
        .open(&dev)
        .map_err(|e| tracing::debug!(error = %e, dev = %dev.display(), "cannot open the bulk disk"))
        .ok()?;
    let mut block = AlignedBuf::zeroed(HEADER_BYTES);
    file.read_exact_at(&mut block.as_mut_slice()[..HEADER_BYTES], 0)
        .ok()?;
    let header = match Header::decode(&block.as_slice()[..HEADER_BYTES]) {
        Some(h) => h,
        None => {
            let h = Header::new(rand::random());
            block.as_mut_slice()[..HEADER_BYTES].copy_from_slice(&h.encode());
            file.write_all_at(&block.as_slice()[..HEADER_BYTES], 0)
                .map_err(|e| tracing::debug!(error = %e, "cannot write the bulk header"))
                .ok()?;
            h
        }
    };
    // A disk smaller than its header claims cannot hold the slots.
    let size = file.metadata().ok()?.len();
    let size = if size == 0 { device_bytes(&dev)? } else { size };
    (size >= header.disk_bytes()).then_some(BulkDisk { file, header })
}

#[cfg(not(unix))]
pub fn open() -> Option<BulkDisk> {
    None
}

#[cfg(unix)]
fn find_device() -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir("/sys/block").ok()?.flatten() {
        let name = entry.file_name();
        let serial = std::fs::read_to_string(entry.path().join("serial")).unwrap_or_default();
        if serial.trim() == cua_proto::bulk::SERIAL {
            return Some(std::path::Path::new("/dev").join(name));
        }
    }
    None
}

#[cfg(unix)]
fn device_bytes(dev: &std::path::Path) -> Option<u64> {
    let name = dev.file_name()?.to_str()?;
    let sectors: u64 = std::fs::read_to_string(format!("/sys/block/{name}/size"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(sectors * 512)
}

static LANE: std::sync::Mutex<Option<std::sync::Arc<BulkDisk>>> = std::sync::Mutex::new(None);

/// Opens the disk afresh (re-reading its header, writing one if it is
/// blank, as after a restore into a new machine's file) and makes it the
/// current lane. Called when capabilities are read.
pub async fn refresh() -> Option<std::sync::Arc<BulkDisk>> {
    let opened = tokio::task::spawn_blocking(open).await.ok().flatten();
    let lane = opened.map(std::sync::Arc::new);
    *LANE.lock().expect("bulk lane") = lane.clone();
    lane
}

/// The current lane, if capabilities were read since it appeared.
pub fn current() -> Option<std::sync::Arc<BulkDisk>> {
    LANE.lock().expect("bulk lane").clone()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A regular file stands in for the device: the same positioned I/O,
    /// without direct I/O.
    fn disk(dir: &std::path::Path) -> BulkDisk {
        let path = dir.join("bulk.img");
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        f.set_len(cua_proto::bulk::DISK_BYTES).unwrap();
        BulkDisk {
            file: f,
            header: Header::new([9; 16]),
        }
    }

    #[test]
    fn stage_then_fetch_round_trips_and_hashes() {
        use sha2::{Digest, Sha256};
        let d = tempfile::tempdir().unwrap();
        let disk = disk(d.path());
        let data: Vec<u8> = (0..(5 << 20) + 5000).map(|i| (i % 251) as u8).collect();
        let src = d.path().join("src");
        std::fs::write(&src, &data).unwrap();
        let src = std::fs::File::open(src).unwrap();
        let (n, sha) = disk.stage(&src, 1000, 5 << 20, 2).unwrap();
        assert_eq!(n, 5 << 20);
        assert_eq!(
            sha,
            hex::encode(Sha256::digest(&data[1000..1000 + (5 << 20)]))
        );
        let back = disk.fetch(2, n).unwrap();
        assert_eq!(
            &back.as_slice()[..n as usize],
            &data[1000..1000 + (5 << 20)]
        );
    }

    #[test]
    fn staging_stops_at_the_end_of_the_file_and_rejects_bad_slots() {
        let d = tempfile::tempdir().unwrap();
        let disk = disk(d.path());
        let src = d.path().join("s");
        std::fs::write(&src, b"hello").unwrap();
        let src = std::fs::File::open(src).unwrap();
        let (n, _) = disk.stage(&src, 0, 1 << 20, 0).unwrap();
        assert_eq!(n, 5);
        assert!(disk.stage(&src, 0, 10, 4).is_err());
        assert!(disk
            .stage(&src, 0, cua_proto::bulk::SLOT_BYTES + 1, 0)
            .is_err());
    }
}
