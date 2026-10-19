//! Guest architecture of a local VM disk, so `vm:<disk>` boots an amd64
//! disk under `qemu-system-x86_64` (emulated on an arm64 host) instead of the
//! host's architecture.
//!
//! Evidence, first match wins:
//!
//! 1. The removable-media UEFI loader on the disk's ESP: `EFI/BOOT/BOOTX64.EFI`
//!    (x86_64) or `BOOTAA64.EFI` (aarch64). Every libs/images disk and most
//!    distribution images carry one; its FAT short name is found in the first
//!    [`SCAN_BYTES`] of the disk (qcow2 is read through `qemu-img dd`).
//! 2. The directory the disk sits in, as `cua images build` and
//!    `libs/images/build.sh` write them: `<out>/<name>/<amd64|arm64>/disk.img`.
//!
//! With neither, the caller falls back to the host architecture.

use std::path::Path;

use crate::types::Arch;

/// How much of the disk is scanned: the libs/images layout puts the ESP at
/// 2..66 MiB; distribution images keep it in the first ~600 MiB.
pub const SCAN_BYTES: u64 = 128 * 1024 * 1024;

/// FAT 8.3 short names of the removable-media loaders (name padded to 8,
/// extension 3, no dot), as they appear in a FAT directory entry.
const X86_64_LOADER: &[u8; 11] = b"BOOTX64 EFI";
const AARCH64_LOADER: &[u8; 11] = b"BOOTAA64EFI";

/// The guest architecture `path` was built for, when the disk says so.
pub fn detect(path: &Path) -> Option<Arch> {
    read_head(path, SCAN_BYTES)
        .and_then(|head| from_bytes(&head))
        .or_else(|| from_dir(path))
}

/// The architecture of the only UEFI loader in `head` (none or both: unknown).
pub fn from_bytes(head: &[u8]) -> Option<Arch> {
    let has = |needle: &[u8; 11]| head.windows(needle.len()).any(|w| w == needle);
    match (has(X86_64_LOADER), has(AARCH64_LOADER)) {
        (true, false) => Some(Arch::X86_64),
        (false, true) => Some(Arch::Aarch64),
        _ => None,
    }
}

/// The architecture named by the disk's directory (`.../amd64/disk.img`).
pub fn from_dir(path: &Path) -> Option<Arch> {
    let dir = path.parent()?.file_name()?.to_str()?.to_ascii_lowercase();
    match dir.as_str() {
        "amd64" | "x86_64" | "x64" => Some(Arch::X86_64),
        "arm64" | "aarch64" => Some(Arch::Aarch64),
        _ => None,
    }
}

/// The first `limit` bytes of the disk's guest-visible contents: raw files
/// directly, qcow2 (magic `QFI\xfb`) through `qemu-img dd`.
fn read_head(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return None;
    }
    if magic != *b"QFI\xfb" {
        let mut head = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(limit)
            .read_to_end(&mut head)
            .ok()?;
        return Some(head);
    }
    let qemu_img = super::img::qemu_img().ok()?;
    let tmp = tempfile::tempdir().ok()?;
    let out = tmp.path().join("head.raw");
    let mib = limit.div_ceil(1024 * 1024);
    let status = std::process::Command::new(qemu_img)
        .args(["dd", "-f", "qcow2", "-O", "raw", "bs=1M"])
        .arg(format!("count={mib}"))
        .arg(format!("if={}", path.display()))
        .arg(format!("of={}", out.display()))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::read(&out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk_with(dir: &Path, name: &str, loader: Option<&[u8; 11]>) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut bytes = vec![0u8; 3 * 1024 * 1024];
        if let Some(loader) = loader {
            // A FAT directory entry inside the (fake) ESP at 2 MiB.
            let at = 2 * 1024 * 1024 + 4096;
            bytes[at..at + 11].copy_from_slice(loader);
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn loader_names_decide_the_architecture() {
        assert_eq!(from_bytes(b"....BOOTX64 EFI...."), Some(Arch::X86_64));
        assert_eq!(from_bytes(b"....BOOTAA64EFI...."), Some(Arch::Aarch64));
        assert_eq!(from_bytes(b"BOOTX64 EFI BOOTAA64EFI"), None);
        assert_eq!(from_bytes(b"no loader"), None);
    }

    #[test]
    fn build_output_directories_name_the_architecture() {
        assert_eq!(
            from_dir(Path::new("/o/omarchy/amd64/disk.img")),
            Some(Arch::X86_64)
        );
        assert_eq!(
            from_dir(Path::new("/o/linux/arm64/disk.img")),
            Some(Arch::Aarch64)
        );
        assert_eq!(from_dir(Path::new("/o/disk.img")), None);
    }

    #[test]
    fn raw_disks_are_scanned_before_their_directory() {
        let tmp = tempfile::tempdir().unwrap();
        // The loader wins over a misleading directory name.
        let arm = tmp.path().join("amd64");
        std::fs::create_dir(&arm).unwrap();
        let disk = disk_with(&arm, "disk.img", Some(AARCH64_LOADER));
        assert_eq!(detect(&disk), Some(Arch::Aarch64));
        // No loader: the directory decides; neither: unknown.
        let bare = disk_with(&arm, "bare.img", None);
        assert_eq!(detect(&bare), Some(Arch::X86_64));
        assert_eq!(detect(&disk_with(tmp.path(), "x.img", None)), None);
    }

    /// The Omarchy e2e's case: a compressed qcow2 amd64 disk on any host.
    #[test]
    fn qcow2_disks_are_read_through_qemu_img() {
        let Ok(qemu_img) = super::super::img::qemu_img() else {
            eprintln!("skipped: qemu-img not installed");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let raw = disk_with(tmp.path(), "disk.raw", Some(X86_64_LOADER));
        let qcow2 = tmp.path().join("disk.img");
        let ok = std::process::Command::new(qemu_img)
            .args(["convert", "-c", "-f", "raw", "-O", "qcow2"])
            .arg(&raw)
            .arg(&qcow2)
            .status()
            .unwrap()
            .success();
        assert!(ok, "qemu-img convert");
        assert_eq!(detect(&qcow2), Some(Arch::X86_64));
    }
}
