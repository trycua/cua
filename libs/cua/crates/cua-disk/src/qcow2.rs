//! qcow2 backing chains, read from the image headers (no `qemu-img`
//! needed), so garbage collection knows which cached disks an overlay still
//! depends on.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"QFI\xfb";
/// Longest chain followed (a loop or a corrupt header stops here).
const MAX_DEPTH: usize = 64;

/// The backing file named in `path`'s qcow2 header (relative names resolve
/// against `path`'s directory). `None` for a non-qcow2 file or a qcow2
/// without a backing file.
pub fn backing_file(path: &Path) -> Option<PathBuf> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut hdr = [0u8; 20];
    f.read_exact(&mut hdr).ok()?;
    if &hdr[..4] != MAGIC {
        return None;
    }
    let off = u64::from_be_bytes(hdr[8..16].try_into().ok()?);
    let len = u32::from_be_bytes(hdr[16..20].try_into().ok()?) as usize;
    if off == 0 || len == 0 || len > 4096 {
        return None;
    }
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut name = vec![0u8; len];
    f.read_exact(&mut name).ok()?;
    let name = PathBuf::from(String::from_utf8(name).ok()?);
    Some(if name.is_absolute() {
        name
    } else {
        path.parent().unwrap_or(Path::new(".")).join(name)
    })
}

/// `path` and every file below it in its backing chain.
pub fn chain(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    let mut cur = path.to_path_buf();
    while out.len() < MAX_DEPTH {
        match backing_file(&cur) {
            Some(next) if !out.contains(&next) => {
                out.push(next.clone());
                cur = next;
            }
            _ => break,
        }
    }
    out
}

/// A minimal qcow2 header naming `backing` (tests and fixtures).
pub fn write_header(path: &Path, backing: Option<&Path>) -> std::io::Result<()> {
    let mut buf = vec![0u8; 512];
    buf[..4].copy_from_slice(MAGIC);
    buf[4..8].copy_from_slice(&3u32.to_be_bytes());
    if let Some(b) = backing {
        let name = b.to_string_lossy().into_owned().into_bytes();
        buf[8..16].copy_from_slice(&112u64.to_be_bytes());
        buf[16..20].copy_from_slice(&(name.len() as u32).to_be_bytes());
        buf.resize(112 + name.len().max(400), 0);
        buf[112..112 + name.len()].copy_from_slice(&name);
    }
    std::fs::write(path, buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_absolute_and_relative_backing_names() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("base.qcow2");
        let mid = d.path().join("mid.qcow2");
        let top = d.path().join("top.qcow2");
        write_header(&base, None).unwrap();
        write_header(&mid, Some(&base)).unwrap();
        write_header(&top, Some(Path::new("mid.qcow2"))).unwrap();
        assert_eq!(chain(&top), vec![top.clone(), mid.clone(), base.clone()]);
        assert_eq!(backing_file(&base), None);
        std::fs::write(d.path().join("raw.img"), b"not qcow2 at all.....").unwrap();
        assert_eq!(chain(&d.path().join("raw.img")).len(), 1);
    }
}
