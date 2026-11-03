//! Reading what a Linux executable asks of its host.

/// The program interpreter (`PT_INTERP`) of a 64-bit little-endian ELF
/// executable: `Some(None)` for a static one, `Some(Some(path))` for a
/// dynamically linked one, `None` when `elf` is not such an ELF.
pub fn interpreter(elf: &[u8]) -> Option<Option<String>> {
    if elf.len() < 64 || &elf[..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return None;
    }
    let u16_at = |o: usize| Some(u16::from_le_bytes(elf.get(o..o + 2)?.try_into().ok()?));
    let u32_at = |o: usize| Some(u32::from_le_bytes(elf.get(o..o + 4)?.try_into().ok()?));
    let u64_at = |o: usize| Some(u64::from_le_bytes(elf.get(o..o + 8)?.try_into().ok()?));
    let phoff = u64_at(32)? as usize;
    let phentsize = u16_at(54)? as usize;
    let phnum = u16_at(56)? as usize;
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        // PT_INTERP
        if u32_at(ph)? == 3 {
            let off = u64_at(ph + 8)? as usize;
            let size = u64_at(ph + 32)? as usize;
            let raw = elf.get(off..off + size)?;
            let s = String::from_utf8_lossy(raw)
                .trim_end_matches('\0')
                .to_string();
            return Some(Some(s));
        }
    }
    Some(None)
}
