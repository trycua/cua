//! `qemu-img` wrapper: overlays, chains, conversion.
//!
//! The QEMU backend keeps disks as qcow2 chains:
//!
//! ```text
//!   image (containerDisk cache / user file, never written)
//!     └─ frozen layers (_layers/*.qcow2: checkpoints, fork points)
//!          └─ instance disk (<state>/<name>/disk.qcow2, the only writable file)
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Result, VmmError};
use crate::host;

/// Locate `qemu-img` (PATH, Homebrew, next to qemu-system-*).
pub fn qemu_img() -> Result<PathBuf> {
    host::which("qemu-img").ok_or_else(|| {
        VmmError::missing(
            "qemu-img",
            "install QEMU (macOS: `brew install qemu`; Debian/Ubuntu: `apt install qemu-utils`)",
        )
    })
}

/// Subset of `qemu-img info --output=json`.
#[derive(Clone, Debug, Deserialize)]
pub struct ImageInfo {
    pub filename: String,
    pub format: String,
    #[serde(rename = "virtual-size")]
    pub virtual_size: u64,
    #[serde(rename = "actual-size", default)]
    pub actual_size: u64,
    #[serde(rename = "backing-filename", default)]
    pub backing_filename: Option<String>,
    #[serde(rename = "full-backing-filename", default)]
    pub full_backing_filename: Option<String>,
    #[serde(rename = "backing-filename-format", default)]
    pub backing_format: Option<String>,
    #[serde(default)]
    pub snapshots: Vec<SnapshotInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SnapshotInfo {
    pub name: String,
    #[serde(rename = "vm-state-size", default)]
    pub vm_state_size: u64,
}

/// Parse `qemu-img info --backing-chain --output=json` (an array, top first).
pub fn parse_chain(json: &str) -> Result<Vec<ImageInfo>> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let arr = if v.is_array() {
        v
    } else {
        serde_json::Value::Array(vec![v])
    };
    Ok(serde_json::from_value(arr)?)
}

/// `qemu-img info` for one file. `force_share` (`-U`) allows inspecting a disk
/// a running QEMU holds open.
pub async fn info(path: &Path, force_share: bool) -> Result<ImageInfo> {
    let p = path.display().to_string();
    let mut args = vec!["info", "--output=json"];
    if force_share {
        args.push("-U");
    }
    args.push(&p);
    let out = host::run(qemu_img()?, &args).await?;
    Ok(serde_json::from_str(&out)?)
}

/// Full backing chain, top (the file itself) first.
pub async fn chain(path: &Path) -> Result<Vec<ImageInfo>> {
    let p = path.display().to_string();
    let out = host::run(
        qemu_img()?,
        &["info", "-U", "--backing-chain", "--output=json", &p],
    )
    .await?;
    parse_chain(&out)
}

/// Create a qcow2 overlay on `backing`. The backing format is detected so raw
/// cloud images and qcow2 containerDisks both work.
pub async fn create_overlay(backing: &Path, overlay: &Path) -> Result<()> {
    if let Some(parent) = overlay.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let backing = std::fs::canonicalize(backing).map_err(|e| {
        VmmError::invalid(format!(
            "backing disk {} is not readable: {e}",
            backing.display()
        ))
    })?;
    let fmt = info(&backing, true).await?.format;
    let b = backing.display().to_string();
    let o = overlay.display().to_string();
    host::run(
        qemu_img()?,
        &["create", "-q", "-f", "qcow2", "-b", &b, "-F", &fmt, &o],
    )
    .await?;
    Ok(())
}

/// Create an empty qcow2 of `size_gb` GiB.
pub async fn create_blank(path: &Path, size_gb: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let p = path.display().to_string();
    host::run(
        qemu_img()?,
        &["create", "-q", "-f", "qcow2", &p, &format!("{size_gb}G")],
    )
    .await?;
    Ok(())
}

/// Grow a disk to at least `size_gb` GiB (never shrinks).
pub async fn grow_to(path: &Path, size_gb: u32) -> Result<()> {
    let want = u64::from(size_gb) * 1024 * 1024 * 1024;
    if info(path, false).await?.virtual_size >= want {
        return Ok(());
    }
    let p = path.display().to_string();
    host::run(qemu_img()?, &["resize", "-q", &p, &format!("{size_gb}G")]).await?;
    Ok(())
}

/// Convert (flatten) `src` into a standalone qcow2, optionally compressed and
/// optionally from an internal snapshot.
pub async fn convert(src: &Path, dst: &Path, compress: bool, snapshot: Option<&str>) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = src.display().to_string();
    let d = dst.display().to_string();
    let snap;
    let mut args = vec!["convert", "-q", "-O", "qcow2"];
    if compress {
        args.push("-c");
    }
    if let Some(name) = snapshot {
        snap = format!("snapshot.name={name}");
        args.push("-l");
        args.push(&snap);
    }
    args.push(&s);
    args.push(&d);
    host::run(qemu_img()?, &args).await?;
    Ok(())
}

/// Convert a raw firmware varstore into qcow2 so it can hold `savevm` state.
pub async fn raw_to_qcow2(src: &Path, dst: &Path) -> Result<()> {
    let s = src.display().to_string();
    let d = dst.display().to_string();
    host::run(
        qemu_img()?,
        &["convert", "-q", "-f", "raw", "-O", "qcow2", &s, &d],
    )
    .await?;
    Ok(())
}

/// Every file in the backing chain of `path` (including `path`).
pub async fn chain_files(path: &Path) -> Result<Vec<PathBuf>> {
    Ok(chain(path)
        .await?
        .into_iter()
        .map(|i| PathBuf::from(i.filename))
        .collect())
}

/// What the partition table says about how a disk boots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartitionLayout {
    /// The disk has a GPT.
    pub gpt: bool,
    /// An EFI System Partition is present.
    pub esp: bool,
    /// A BIOS boot partition (GRUB `bios_grub`) is present.
    pub bios_boot: bool,
    /// A Microsoft reserved / basic data partition is present (Windows).
    pub windows: bool,
}

impl PartitionLayout {
    /// GPT with an ESP and no BIOS boot partition: boots under UEFI only
    /// (e.g. the dockur-built Windows containerDisks). Hybrid cloud images
    /// (ESP + `bios_grub`) boot either way; KubeVirt uses BIOS for them.
    pub fn efi_only(&self) -> bool {
        self.gpt && self.esp && !self.bios_boot
    }
}

const GUID_ESP: [u8; 16] = guid(
    0xC12A7328,
    0xF81F,
    0x11D2,
    [0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B],
);
const GUID_BIOS_BOOT: [u8; 16] = guid(
    0x21686148,
    0x6449,
    0x6E6F,
    [0x74, 0x4E, 0x65, 0x65, 0x64, 0x45, 0x46, 0x49],
);
const GUID_MS_RESERVED: [u8; 16] = guid(
    0xE3C9E316,
    0x0B5C,
    0x4DB8,
    [0x81, 0x7D, 0xF9, 0x2D, 0xF0, 0x02, 0x15, 0xAE],
);
const GUID_MS_BASIC_DATA: [u8; 16] = guid(
    0xEBD0A0A2,
    0xB9E5,
    0x4433,
    [0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7],
);
const GUID_MS_RECOVERY: [u8; 16] = guid(
    0xDE94BBA4,
    0x06D1,
    0x4D40,
    [0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xD6, 0xAC],
);

/// A GPT type GUID in its on-disk (mixed-endian) byte order.
const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

/// Sectors read to see the GPT header and the first 128 entries.
const GPT_SECTORS: usize = 34;

/// Parse the partition layout from the first [`GPT_SECTORS`] 512-byte
/// sectors of a disk. `None` when the bytes are too short.
pub fn parse_partition_layout(head: &[u8]) -> Option<PartitionLayout> {
    if head.len() < 1024 {
        return None;
    }
    let mut out = PartitionLayout::default();
    let hdr = &head[512..1024];
    if &hdr[..8] != b"EFI PART" {
        return Some(out); // MBR or unpartitioned: BIOS.
    }
    out.gpt = true;
    let u32_at = |o: usize| u32::from_le_bytes(hdr[o..o + 4].try_into().unwrap());
    let entries_lba = u64::from_le_bytes(hdr[72..80].try_into().unwrap()) as usize;
    let count = u32_at(80) as usize;
    let size = u32_at(84) as usize;
    if size < 16 {
        return Some(out);
    }
    for i in 0..count.min(128) {
        let off = entries_lba * 512 + i * size;
        let Some(ty) = head.get(off..off + 16) else {
            break;
        };
        if ty == GUID_ESP {
            out.esp = true;
        } else if ty == GUID_BIOS_BOOT {
            out.bios_boot = true;
        } else if ty == GUID_MS_RESERVED || ty == GUID_MS_RECOVERY {
            out.windows = true;
        } else if ty == GUID_MS_BASIC_DATA && out.esp {
            // Basic data after an ESP without Linux partitions is Windows'
            // C: drive; a lone basic-data partition may just be FAT data.
            out.windows = true;
        }
    }
    Some(out)
}

/// Read a disk's partition layout (any format `qemu-img` reads; the
/// backing chain is followed). `Ok(None)` when it cannot be read.
pub async fn partition_layout(disk: &Path) -> Result<Option<PartitionLayout>> {
    let fmt = info(disk, true).await?.format;
    let tmp = std::env::temp_dir().join(format!(
        "cua-gpt-{}-{}.raw",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let res = host::run(
        qemu_img()?,
        &[
            "dd",
            "-U",
            "-f",
            &fmt,
            "-O",
            "raw",
            "bs=512",
            &format!("count={GPT_SECTORS}"),
            &format!("if={}", disk.display()),
            &format!("of={}", tmp.display()),
        ],
    )
    .await;
    let head = std::fs::read(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    res?;
    Ok(head.and_then(|h| parse_partition_layout(&h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAIN: &str = r#"[
      {"virtual-size": 2147483648, "filename": "/s/vm/disk.qcow2", "format": "qcow2",
       "actual-size": 200704, "backing-filename": "/s/_layers/a.qcow2",
       "full-backing-filename": "/s/_layers/a.qcow2", "backing-filename-format": "qcow2",
       "snapshots": [{"id": "1", "name": "ck1", "vm-state-size": 12345, "date-sec": 1, "date-nsec": 0, "vm-clock-sec": 0, "vm-clock-nsec": 0, "icount": 0}]},
      {"virtual-size": 2147483648, "filename": "/s/_layers/a.qcow2", "format": "qcow2",
       "backing-filename": "/cache/debian.qcow2", "backing-filename-format": "qcow2"},
      {"virtual-size": 2147483648, "filename": "/cache/debian.qcow2", "format": "qcow2"}
    ]"#;

    #[test]
    fn parses_backing_chain_top_first() {
        let c = parse_chain(CHAIN).unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].backing_filename.as_deref(), Some("/s/_layers/a.qcow2"));
        assert_eq!(c[0].snapshots[0].name, "ck1");
        assert_eq!(c[2].backing_filename, None);
    }

    #[test]
    fn gpt_layouts_are_classified() {
        fn disk(types: &[[u8; 16]]) -> Vec<u8> {
            let mut d = vec![0u8; 34 * 512];
            d[512..520].copy_from_slice(b"EFI PART");
            d[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
            d[512 + 80..512 + 84].copy_from_slice(&128u32.to_le_bytes());
            d[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
            for (i, t) in types.iter().enumerate() {
                let o = 1024 + i * 128;
                d[o..o + 16].copy_from_slice(t);
            }
            d
        }
        let linux = [0xAFu8; 16];
        // Ubuntu/Arch cloud images: bios_grub + ESP + root -> BIOS is fine.
        let hybrid = parse_partition_layout(&disk(&[GUID_BIOS_BOOT, GUID_ESP, linux])).unwrap();
        assert!(hybrid.gpt && hybrid.esp && hybrid.bios_boot && !hybrid.efi_only());
        assert!(!hybrid.windows);
        // dockur Windows: ESP + MSR + basic data (+ recovery) -> UEFI only.
        let win = parse_partition_layout(&disk(&[
            GUID_ESP,
            GUID_MS_RESERVED,
            GUID_MS_BASIC_DATA,
            GUID_MS_RECOVERY,
        ]))
        .unwrap();
        assert!(win.efi_only() && win.windows);
        // MBR / unpartitioned.
        let mbr = parse_partition_layout(&vec![0u8; 34 * 512]).unwrap();
        assert!(!mbr.gpt && !mbr.efi_only());
        assert!(parse_partition_layout(&[0u8; 100]).is_none());
        // Real mixed-endian encoding of the ESP GUID.
        assert_eq!(
            GUID_ESP,
            [
                0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
                0xC9, 0x3B
            ]
        );
    }

    #[test]
    fn single_object_is_accepted_as_chain() {
        let c = parse_chain(r#"{"virtual-size": 1, "filename": "x", "format": "raw"}"#).unwrap();
        assert_eq!(c.len(), 1);
    }

    /// Real qemu-img: base → overlay → overlay chain, grow, flatten.
    #[tokio::test]
    async fn real_qcow2_chain_round_trip() {
        if qemu_img().is_err() {
            eprintln!("qemu-img not installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.qcow2");
        let mid = dir.path().join("mid.qcow2");
        let top = dir.path().join("top.qcow2");
        create_blank(&base, 1).await.unwrap();
        create_overlay(&base, &mid).await.unwrap();
        create_overlay(&mid, &top).await.unwrap();
        let files = chain_files(&top).await.unwrap();
        assert_eq!(files.len(), 3);
        assert!(files[2].ends_with("base.qcow2"));
        grow_to(&top, 2).await.unwrap();
        assert_eq!(info(&top, false).await.unwrap().virtual_size, 2 << 30);
        grow_to(&top, 1).await.unwrap(); // never shrinks
        let flat = dir.path().join("flat.qcow2");
        convert(&top, &flat, true, None).await.unwrap();
        assert!(info(&flat, false).await.unwrap().backing_filename.is_none());
        // Raw backing files are detected as raw.
        let raw = dir.path().join("raw.img");
        std::fs::write(&raw, vec![0u8; 1 << 20]).unwrap();
        let ov = dir.path().join("ov.qcow2");
        create_overlay(&raw, &ov).await.unwrap();
        assert_eq!(
            info(&ov, false).await.unwrap().backing_format.as_deref(),
            Some("raw")
        );
    }
}
