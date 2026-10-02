//! UEFI firmware discovery.
//!
//! Code and variable-store images must come from the *same* candidate entry:
//! the halves are sized for each other, and a 4 MB OVMF paired with a 2 MB
//! varstore leaves the guest unbootable (lesson carried over from
//! `cua_sandbox/runtime/qemu.py`).

use std::path::{Path, PathBuf};

use crate::types::Arch;

/// UEFI firmware for one guest architecture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uefi {
    /// Read-only firmware code.
    pub code: PathBuf,
    /// Variable-store template, copied per VM. `None` means boot with
    /// `-bios <code>` (volatile variables).
    pub vars_template: Option<PathBuf>,
}

/// `(code, vars)` candidates. Relative paths are resolved against the QEMU
/// install prefix (`<bin>/..`), which is where Homebrew and the upstream
/// tarballs put edk2 (`share/qemu/edk2-*.fd`).
fn candidates(arch: Arch) -> &'static [(&'static str, &'static str)] {
    match arch {
        Arch::Aarch64 => &[
            (
                "share/qemu/edk2-aarch64-code.fd",
                "share/qemu/edk2-arm-vars.fd",
            ),
            (
                "/usr/share/AAVMF/AAVMF_CODE.fd",
                "/usr/share/AAVMF/AAVMF_VARS.fd",
            ),
            (
                "/usr/share/qemu/edk2-aarch64-code.fd",
                "/usr/share/qemu/edk2-arm-vars.fd",
            ),
            (
                "/usr/share/edk2/aarch64/QEMU_EFI-pflash.raw",
                "/usr/share/edk2/aarch64/vars-template-pflash.raw",
            ),
            ("/usr/share/qemu-efi-aarch64/QEMU_EFI.fd", ""),
        ],
        Arch::X86_64 => &[
            (
                "share/qemu/edk2-x86_64-code.fd",
                "share/qemu/edk2-i386-vars.fd",
            ),
            // Bundled Windows/portable layout (`<dir>/share/...` next to the exe).
            ("share/edk2-x86_64-code.fd", "share/edk2-i386-vars.fd"),
            (
                "/usr/share/OVMF/OVMF_CODE_4M.fd",
                "/usr/share/OVMF/OVMF_VARS_4M.fd",
            ),
            (
                "/usr/share/OVMF/OVMF_CODE.fd",
                "/usr/share/OVMF/OVMF_VARS.fd",
            ),
            (
                "/usr/share/edk2/ovmf/OVMF_CODE.fd",
                "/usr/share/edk2/ovmf/OVMF_VARS.fd",
            ),
            (
                "/usr/share/qemu/edk2-x86_64-code.fd",
                "/usr/share/qemu/edk2-i386-vars.fd",
            ),
        ],
    }
}

/// Find UEFI firmware for `arch`, given the path of the `qemu-system-*`
/// binary. `exists` is injectable for tests.
pub fn find_uefi_with(arch: Arch, qemu_bin: &Path, exists: impl Fn(&Path) -> bool) -> Option<Uefi> {
    // bin/qemu-system-x → prefix = bin/..   (also try the bin dir itself for
    // portable layouts where share/ sits beside the exe).
    let bin_dir = qemu_bin.parent().unwrap_or(Path::new("."));
    let prefixes = [
        bin_dir.parent().unwrap_or(bin_dir).to_path_buf(),
        bin_dir.to_path_buf(),
    ];
    for (code, vars) in candidates(arch) {
        let resolve = |p: &str| -> Vec<PathBuf> {
            if p.starts_with('/') {
                vec![PathBuf::from(p)]
            } else {
                prefixes.iter().map(|pre| pre.join(p)).collect()
            }
        };
        for code_path in resolve(code) {
            if !exists(&code_path) {
                continue;
            }
            let vars_path = if vars.is_empty() {
                None
            } else {
                // Prefer the vars file in the same prefix as the code file.
                let same_prefix = if vars.starts_with('/') {
                    PathBuf::from(vars)
                } else {
                    let depth = Path::new(code).components().count();
                    let mut prefix = code_path.clone();
                    for _ in 0..depth {
                        prefix.pop();
                    }
                    prefix.join(vars)
                };
                exists(&same_prefix).then_some(same_prefix)
            };
            return Some(Uefi {
                code: code_path,
                vars_template: vars_path,
            });
        }
    }
    None
}

/// Find UEFI firmware on the real filesystem.
pub fn find_uefi(arch: Arch, qemu_bin: &Path) -> Option<Uefi> {
    // Resolve symlinks: Homebrew's /opt/homebrew/bin/qemu-system-* links into
    // the Cellar, whose share/qemu holds the firmware.
    let real = std::fs::canonicalize(qemu_bin).unwrap_or_else(|_| qemu_bin.to_path_buf());
    find_uefi_with(arch, &real, |p| p.is_file())
        .or_else(|| find_uefi_with(arch, qemu_bin, |p| p.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn fs(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    #[test]
    fn homebrew_layout_is_found_relative_to_binary() {
        let bin = Path::new("/opt/homebrew/Cellar/qemu/11.0.0/bin/qemu-system-aarch64");
        let found = find_uefi_with(
            Arch::Aarch64,
            bin,
            fs(&[
                "/opt/homebrew/Cellar/qemu/11.0.0/share/qemu/edk2-aarch64-code.fd",
                "/opt/homebrew/Cellar/qemu/11.0.0/share/qemu/edk2-arm-vars.fd",
            ]),
        )
        .unwrap();
        assert_eq!(
            found.code,
            Path::new("/opt/homebrew/Cellar/qemu/11.0.0/share/qemu/edk2-aarch64-code.fd")
        );
        assert_eq!(
            found.vars_template.as_deref(),
            Some(Path::new(
                "/opt/homebrew/Cellar/qemu/11.0.0/share/qemu/edk2-arm-vars.fd"
            ))
        );
    }

    #[test]
    fn debian_ovmf_pairs_code_with_matching_vars() {
        let bin = Path::new("/usr/bin/qemu-system-x86_64");
        let found = find_uefi_with(
            Arch::X86_64,
            bin,
            fs(&[
                "/usr/share/OVMF/OVMF_CODE_4M.fd",
                "/usr/share/OVMF/OVMF_VARS_4M.fd",
                "/usr/share/OVMF/OVMF_VARS.fd",
            ]),
        )
        .unwrap();
        assert_eq!(found.code, Path::new("/usr/share/OVMF/OVMF_CODE_4M.fd"));
        assert_eq!(
            found.vars_template.as_deref(),
            Some(Path::new("/usr/share/OVMF/OVMF_VARS_4M.fd"))
        );
    }

    #[test]
    fn code_without_vars_falls_back_to_bios_mode() {
        let bin = Path::new("/usr/bin/qemu-system-aarch64");
        let found = find_uefi_with(
            Arch::Aarch64,
            bin,
            fs(&["/usr/share/qemu-efi-aarch64/QEMU_EFI.fd"]),
        )
        .unwrap();
        assert!(found.vars_template.is_none());
        // A code file whose paired vars are missing yields no vars either.
        let found =
            find_uefi_with(Arch::X86_64, bin, fs(&["/usr/share/OVMF/OVMF_CODE.fd"])).unwrap();
        assert!(found.vars_template.is_none());
    }

    #[test]
    fn nothing_found_is_none() {
        assert!(find_uefi_with(Arch::X86_64, Path::new("/x/bin/q"), |_| false).is_none());
    }

    #[test]
    fn portable_windows_layout() {
        let bin = Path::new("C:/qemu/qemu-system-x86_64.exe");
        let found = find_uefi_with(
            Arch::X86_64,
            bin,
            fs(&[
                "C:/qemu/share/edk2-x86_64-code.fd",
                "C:/qemu/share/edk2-i386-vars.fd",
            ]),
        )
        .unwrap();
        assert_eq!(found.code, Path::new("C:/qemu/share/edk2-x86_64-code.fd"));
        assert!(found.vars_template.is_some());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn real_host_has_firmware_when_qemu_installed() {
        if let Some(bin) = crate::host::which("qemu-system-aarch64") {
            assert!(
                find_uefi(Arch::Aarch64, &bin).is_some(),
                "edk2 not found next to {bin:?}"
            );
        }
    }
}
