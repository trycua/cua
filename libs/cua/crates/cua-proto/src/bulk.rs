//! The bulk lane's disk layout: a raw file the host shares with its guest as
//! a virtio-blk disk (serial [`SERIAL`]). The guest's cua-spacesd writes a
//! [`Header`] at byte 0 and stages file content in fixed slots; the host-side
//! client reads and writes the same bytes through the file. Shared by the
//! host (`cua-vmm` sizes the file), the guest (`cua-spacesd-server`) and the
//! client (`cua-spacesd-client`).

/// Serial number of the virtio-blk device (`/sys/block/vdX/serial`).
pub const SERIAL: &str = "cuabulk";
/// Bytes the header occupies (one 4 KiB block: direct I/O is block-aligned).
pub const HEADER_BYTES: usize = 4096;
/// Identifies a header.
pub const MAGIC: [u8; 8] = *b"CUABULK1";
/// Offset of slot 0.
pub const DATA_OFFSET: u64 = 1 << 20;
/// Slots on the disk.
pub const SLOTS: u32 = 4;
/// Bytes per slot.
pub const SLOT_BYTES: u64 = 16 << 20;
/// Size of the whole disk file (sparse until used).
pub const DISK_BYTES: u64 = DATA_OFFSET + SLOTS as u64 * SLOT_BYTES;
/// Transfers smaller than this use the network: the lane's round trip costs
/// about as much as the bytes saved.
pub const MIN_BYTES: u64 = 2 << 20;

/// What the guest writes at the start of the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Random identity of this disk as the guest saw it.
    pub id: [u8; 16],
    /// Slots.
    pub slots: u32,
    /// Bytes per slot.
    pub slot_bytes: u64,
    /// Offset of slot 0.
    pub data_offset: u64,
}

impl Header {
    /// A header with the standard geometry.
    pub fn new(id: [u8; 16]) -> Self {
        Self {
            id,
            slots: SLOTS,
            slot_bytes: SLOT_BYTES,
            data_offset: DATA_OFFSET,
        }
    }

    /// The header's block.
    pub fn encode(&self) -> [u8; HEADER_BYTES] {
        let mut b = [0u8; HEADER_BYTES];
        b[..8].copy_from_slice(&MAGIC);
        b[8..24].copy_from_slice(&self.id);
        b[24..28].copy_from_slice(&self.slots.to_le_bytes());
        b[28..36].copy_from_slice(&self.slot_bytes.to_le_bytes());
        b[36..44].copy_from_slice(&self.data_offset.to_le_bytes());
        b
    }

    /// Parses a header block; `None` when it is not one (a fresh disk is
    /// all zeros) or its geometry is nonsense.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < 44 || b[..8] != MAGIC {
            return None;
        }
        let h = Self {
            id: b[8..24].try_into().ok()?,
            slots: u32::from_le_bytes(b[24..28].try_into().ok()?),
            slot_bytes: u64::from_le_bytes(b[28..36].try_into().ok()?),
            data_offset: u64::from_le_bytes(b[36..44].try_into().ok()?),
        };
        let aligned = |v: u64| v.is_multiple_of(HEADER_BYTES as u64);
        (h.slots > 0
            && h.slots <= 64
            && h.slot_bytes > 0
            && aligned(h.slot_bytes)
            && aligned(h.data_offset)
            && h.data_offset >= HEADER_BYTES as u64)
            .then_some(h)
    }

    /// Where slot `i` starts.
    pub fn slot_offset(&self, i: u32) -> u64 {
        self.data_offset + u64::from(i) * self.slot_bytes
    }

    /// Bytes the disk file must have for this geometry.
    pub fn disk_bytes(&self) -> u64 {
        self.slot_offset(self.slots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_round_trips_and_a_blank_disk_is_none() {
        let h = Header::new([7; 16]);
        assert_eq!(Header::decode(&h.encode()), Some(h));
        assert_eq!(Header::decode(&[0u8; HEADER_BYTES]), None);
        assert_eq!(h.disk_bytes(), DISK_BYTES);
        assert_eq!(h.slot_offset(1), DATA_OFFSET + SLOT_BYTES);
    }

    #[test]
    fn nonsense_geometry_is_refused() {
        let mut h = Header::new([1; 16]);
        h.slot_bytes = 1000;
        assert_eq!(Header::decode(&h.encode()), None);
        let mut h = Header::new([1; 16]);
        h.slots = 0;
        assert_eq!(Header::decode(&h.encode()), None);
    }
}
