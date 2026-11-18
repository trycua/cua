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
                "qemu/usr/share/AAVMF/AAVMF_CODE.fd",
                "qemu/usr/share/AAVMF/AAVMF_VARS.fd",
            ),
            ("qemu/usr/share/qemu-efi-aarch64/QEMU_EFI.fd", ""),
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
                "qemu/usr/share/OVMF/OVMF_CODE_4M.fd",
                "qemu/usr/share/OVMF/OVMF_VARS_4M.fd",
            ),
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

/// Environment variable naming the only directory searched for the Windows
/// on ARM Secure Boot firmware pair.
pub const AAVMF_DIR_ENV: &str = "CUA_QEMU_AAVMF_DIR";

/// Directories searched for the Secure Boot pair, best first. A set
/// override is exclusive.
pub fn windows_arm_dirs(
    override_dir: Option<&Path>,
    cua_dir: &Path,
    bin: &Path,
    real_bin: &Path,
) -> Vec<PathBuf> {
    if let Some(d) = override_dir {
        return vec![d.to_path_buf()];
    }
    let mut dirs = vec![cua_dir.to_path_buf()];
    for b in [real_bin, bin] {
        if let Some(prefix) = b.parent().and_then(Path::parent) {
            let d = prefix.join("share/AAVMF");
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    dirs.push(PathBuf::from("/usr/share/AAVMF"));
    dirs
}

/// The first directory holding both `AAVMF_CODE.secboot.fd` and
/// `AAVMF_VARS.ms.fd`.
pub fn find_windows_arm(dirs: &[PathBuf], exists: impl Fn(&Path) -> bool) -> Option<Uefi> {
    dirs.iter().find_map(|dir| {
        let code = dir.join("AAVMF_CODE.secboot.fd");
        let vars = dir.join("AAVMF_VARS.ms.fd");
        (exists(&code) && exists(&vars)).then_some(Uefi {
            code,
            vars_template: Some(vars),
        })
    })
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

    #[test]
    fn local_wrapper_firmware() {
        let code = "/local/qemu/usr/share/qemu-efi-aarch64/QEMU_EFI.fd";
        let found = find_uefi_with(
            Arch::Aarch64,
            Path::new("/local/bin/qemu-system-aarch64"),
            fs(&[code]),
        )
        .unwrap();
        assert_eq!(found.code, Path::new(code));
        assert!(found.vars_template.is_none());
        for (arch, code, vars) in [
            (Arch::X86_64, "OVMF/OVMF_CODE_4M.fd", "OVMF/OVMF_VARS_4M.fd"),
            (Arch::Aarch64, "AAVMF/AAVMF_CODE.fd", "AAVMF/AAVMF_VARS.fd"),
        ] {
            let code = format!("/local/qemu/usr/share/{code}");
            let vars = format!("/local/qemu/usr/share/{vars}");
            let found = find_uefi_with(
                arch,
                Path::new("/local/bin/qemu-system-test"),
                fs(&[&code, &vars]),
            )
            .unwrap();
            assert_eq!(found.code, Path::new(&code));
            assert_eq!(found.vars_template.as_deref(), Some(Path::new(&vars)));
        }
    }

    fn fs(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    }

    #[test]
    fn windows_arm_lookup_order() {
        let d = |s: &str| PathBuf::from(s);
        let bin = Path::new("/opt/homebrew/bin/qemu-system-aarch64");
        let real = Path::new("/opt/homebrew/Cellar/qemu/11/bin/qemu-system-aarch64");
        let cua = Path::new("/h/.cua/firmware/aavmf");
        let pair = |dir: &str| {
            [
                format!("{dir}/AAVMF_CODE.secboot.fd"),
                format!("{dir}/AAVMF_VARS.ms.fd"),
            ]
        };
        let find = |over: Option<&Path>, dirs: &[&str]| {
            let files: Vec<String> = dirs.iter().flat_map(|x| pair(x)).collect();
            let refs: Vec<&str> = files.iter().map(String::as_str).collect();
            find_windows_arm(&windows_arm_dirs(over, cua, bin, real), fs(&refs))
                .map(|u| u.code.parent().unwrap().to_path_buf())
        };
        // An override is exclusive.
        let over = Path::new("/o");
        assert_eq!(windows_arm_dirs(Some(over), cua, bin, real), vec![d("/o")]);
        assert_eq!(find(Some(over), &["/h/.cua/firmware/aavmf"]), None);
        assert_eq!(find(Some(over), &["/o"]), Some(d("/o")));
        // The cua directory beats the prefixes and the system one.
        assert_eq!(
            find(
                None,
                &[
                    "/usr/share/AAVMF",
                    "/opt/homebrew/share/AAVMF",
                    "/h/.cua/firmware/aavmf"
                ]
            ),
            Some(d("/h/.cua/firmware/aavmf"))
        );
        // Homebrew's as-invoked prefix, when the Cellar lacks the pair.
        assert_eq!(
            find(None, &["/opt/homebrew/share/AAVMF"]),
            Some(d("/opt/homebrew/share/AAVMF"))
        );
        assert_eq!(
            find(
                None,
                &[
                    "/opt/homebrew/Cellar/qemu/11/share/AAVMF",
                    "/usr/share/AAVMF"
                ]
            ),
            Some(d("/opt/homebrew/Cellar/qemu/11/share/AAVMF"))
        );
        // Halves from different directories never mix.
        let split = fs(&[
            "/h/.cua/firmware/aavmf/AAVMF_CODE.secboot.fd",
            "/usr/share/AAVMF/AAVMF_VARS.ms.fd",
        ]);
        assert!(find_windows_arm(&windows_arm_dirs(None, cua, bin, real), split).is_none());
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
