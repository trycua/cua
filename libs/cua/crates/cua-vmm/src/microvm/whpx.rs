//! The Windows half of the microVM backend: QEMU on the Windows Hypervisor
//! Platform (WHPX). Hardware acceleration only: a PC without WHPX has no
//! microVMs (QEMU's software emulation is never used).
//!
//! Everything above the monitor is the Linux backend's: the same guest kernel
//! and init, the image as a read-only squashfs (written in process,
//! [`super::rootfs::build_squashfs_native`]) under an overlay whose upper layer
//! is the VM's own sparse ext4 scratch disk. What differs:
//!
//! * **The monitor** is `qemu-system-x86_64` (`-M q35 -accel whpx`), one
//!   process per VM. `-M microvm` hangs under WHPX in QEMU 11, so the guest
//!   gets virtio over PCI.
//! * **The host link** is a loopback TCP port the VM's user-mode network
//!   forwards to the init (`cua.agent=tcp:` on the kernel command line, a key in
//!   every connection's first line): Windows QEMU has no vsock.
//! * **The network** is QEMU's user-mode stack. It enforces no rules, so under
//!   the secure default a VM has no network at all (as on a Mac) unless the user
//!   chose No protection; published ports are `hostfwd` rules on loopback.
//! * **Disk-only snapshots**: WHPX blocks QEMU migration, so there is no saved
//!   memory. A checkpoint is the scratch disk (qcow2 over a shared template),
//!   a fork copies it and boots cold, and a pool member is booted and paused.
//!
//! [`status`] answers whether WHPX is there without elevation (a call to
//! `WHvGetCapability`); [`enable_elevated`] turns the Windows feature on through
//! one UAC prompt.

use std::path::PathBuf;

use crate::error::{Result, VmmError};
use crate::host;

/// The oldest QEMU whose WHPX accelerator the backend uses.
pub const MIN_QEMU_MAJOR: u32 = 9;

/// What the Windows Hypervisor Platform says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Whpx {
    /// The hypervisor runs and WHP answers.
    Ready,
    /// `WinHvPlatform.dll` is not on this PC: the Windows feature
    /// "Windows Hypervisor Platform" is off.
    FeatureOff,
    /// The feature is on but no hypervisor runs: virtualization is off in the
    /// firmware, or a restart is pending after turning the feature on.
    NoHypervisor,
    /// Not Windows.
    NotWindows,
}

/// Asks the Windows Hypervisor Platform, without elevation.
pub fn status() -> Whpx {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let name: Vec<u16> = "WinHvPlatform.dll\0".encode_utf16().collect();
        let dll = unsafe { LoadLibraryW(name.as_ptr()) };
        if dll.is_null() {
            return Whpx::FeatureOff;
        }
        type GetCapability =
            unsafe extern "system" fn(u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;
        let f = unsafe { GetProcAddress(dll, c"WHvGetCapability".as_ptr().cast()) };
        let Some(f) = f else {
            return Whpx::FeatureOff;
        };
        let f: GetCapability = unsafe { std::mem::transmute(f) };
        // WHvCapabilityCodeHypervisorPresent = 0: a BOOL.
        let mut present: u32 = 0;
        let mut written: u32 = 0;
        let hr = unsafe { f(0, (&mut present as *mut u32).cast(), 4, &mut written) };
        if hr >= 0 && present != 0 {
            Whpx::Ready
        } else {
            Whpx::NoHypervisor
        }
    }
    #[cfg(not(windows))]
    {
        Whpx::NotWindows
    }
}

/// Why this PC cannot run WHPX microVMs, as the doctor and New Space say it
/// (`None` when it can).
pub fn unsupported_reason(w: &Whpx) -> Option<String> {
    match w {
        Whpx::Ready => None,
        Whpx::FeatureOff => Some(
            "microVMs need the Windows feature Windows Hypervisor Platform: Cua Spaces can turn \
             it on (one administrator prompt), then restart this PC"
                .into(),
        ),
        Whpx::NoHypervisor => Some(
            "microVMs need the Windows hypervisor, which is not running: restart this PC if you \
             just turned on Windows Hypervisor Platform, else turn on virtualization (Intel VT-x \
             or AMD SVM) in the PC's firmware settings"
                .into(),
        ),
        Whpx::NotWindows => Some("WHPX microVMs run on Windows".into()),
    }
}

/// `$CUA_QEMU`, then QEMU in `C:\Program Files\qemu` (the installer's folder),
/// then `~/.cua/bin`, then `PATH`.
pub fn qemu_bin() -> Option<PathBuf> {
    const NAME: &str = "qemu-system-x86_64.exe";
    if let Some(p) = std::env::var_os("CUA_QEMU").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    for var in ["ProgramFiles", "ProgramW6432"] {
        if let Some(dir) = std::env::var_os(var) {
            let p = PathBuf::from(dir).join("qemu").join(NAME);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let own = host::cua_home().join("bin").join(NAME);
    if own.is_file() {
        return Some(own);
    }
    host::which(NAME).or_else(|| host::which("qemu-system-x86_64"))
}

/// `qemu-img` next to the QEMU in use (the installer ships both): it makes and
/// copies the VMs' qcow2 scratch disks.
pub fn qemu_img_bin() -> Option<PathBuf> {
    let dir = qemu_bin()?.parent()?.to_path_buf();
    let p = dir.join("qemu-img.exe");
    p.is_file().then_some(p).or_else(|| host::which("qemu-img"))
}

/// `qemu-system-x86_64 --version`'s first line's major version.
pub fn qemu_major(version_line: &str) -> Option<u32> {
    version_line
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)
        .and_then(|v| v.split('.').next())
        .and_then(|m| m.parse().ok())
}

/// The error for a PC without QEMU: how to get it.
pub fn qemu_missing() -> VmmError {
    VmmError::missing(
        "QEMU",
        "microVM Spaces on Windows run in QEMU: `cua runtime setup microvm` installs it with \
         winget (winget install SoftwareFreedomConservancy.QEMU), then create the Space again",
    )
}

/// The command that turns the Windows Hypervisor Platform feature on.
pub const ENABLE_COMMAND: &str =
    "dism.exe /online /enable-feature /featurename:HypervisorPlatform /all /norestart";

/// What turning the feature on came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Enabled {
    /// On, and the hypervisor answers.
    Ready,
    /// On; Windows needs a restart before it runs.
    RestartNeeded,
}

/// How a `dism` exit code reads: 0 done, 3010 done after a restart.
pub fn enable_outcome(exit_code: i32) -> std::result::Result<Enabled, String> {
    match exit_code {
        0 => Ok(Enabled::Ready),
        3010 => Ok(Enabled::RestartNeeded),
        n => Err(format!("dism exited with code {n}")),
    }
}

/// The PowerShell that runs [`ENABLE_COMMAND`] elevated (one UAC prompt) and
/// prints its exit code.
pub fn enable_script() -> String {
    "$p = Start-Process -FilePath dism.exe -ArgumentList '/online','/enable-feature',\
     '/featurename:HypervisorPlatform','/all','/norestart' -Verb RunAs -Wait -PassThru; \
     [Console]::Out.Write($p.ExitCode)"
        .into()
}

/// Turns the Windows Hypervisor Platform on through one UAC prompt. A refused
/// prompt is an error that says so; a feature that needs a restart is
/// [`Enabled::RestartNeeded`] (the caller keeps that state until a restart).
pub async fn enable_elevated() -> Result<Enabled> {
    #[cfg(windows)]
    {
        let out = tokio::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &enable_script(),
            ])
            .creation_flags(0x0800_0000)
            .stdin(std::process::Stdio::null())
            .output()
            .await?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let err = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() || text.is_empty() {
            let refused =
                err.to_lowercase().contains("canceled") || err.to_lowercase().contains("cancelled");
            return Err(VmmError::other(if refused {
                "the administrator prompt was declined: Windows Hypervisor Platform stays off. \
                 Run `cua runtime setup microvm` again and accept it"
                    .to_string()
            } else {
                format!(
                    "turning on Windows Hypervisor Platform failed: {}",
                    err.trim()
                )
            }));
        }
        let code: i32 = text.parse().map_err(|_| {
            VmmError::other(format!("dism printed {text:?} instead of an exit code"))
        })?;
        enable_outcome(code).map_err(VmmError::other)
    }
    #[cfg(not(windows))]
    {
        Err(VmmError::other(
            "Windows Hypervisor Platform is a Windows feature",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_versions_are_read_from_its_banner() {
        assert_eq!(
            qemu_major("QEMU emulator version 11.1.0 (v11.1.0-12130-ge470268ff4)"),
            Some(11)
        );
        assert_eq!(qemu_major("QEMU emulator version 8.2.2"), Some(8));
        assert_eq!(qemu_major("not qemu"), None);
    }

    #[test]
    fn every_state_has_a_reason_that_says_what_to_do() {
        assert_eq!(unsupported_reason(&Whpx::Ready), None);
        let off = unsupported_reason(&Whpx::FeatureOff).unwrap();
        assert!(off.contains("Windows Hypervisor Platform") && off.contains("administrator"));
        let hv = unsupported_reason(&Whpx::NoHypervisor).unwrap();
        assert!(hv.contains("firmware") && hv.contains("restart"));
    }

    #[test]
    fn dism_exit_codes() {
        assert_eq!(enable_outcome(0), Ok(Enabled::Ready));
        assert_eq!(enable_outcome(3010), Ok(Enabled::RestartNeeded));
        assert!(enable_outcome(5).is_err());
    }

    #[test]
    fn the_elevation_runs_dism_on_the_right_feature_with_a_prompt() {
        let s = enable_script();
        assert!(s.contains("-Verb RunAs") && s.contains("HypervisorPlatform"));
        assert!(s.contains("/norestart"));
        assert!(ENABLE_COMMAND.contains("/featurename:HypervisorPlatform"));
    }

    /// The real call, on this PC (Windows only): it answers without elevation.
    #[cfg(windows)]
    #[test]
    fn whp_answers_without_elevation() {
        assert_ne!(status(), Whpx::NotWindows);
    }
}
