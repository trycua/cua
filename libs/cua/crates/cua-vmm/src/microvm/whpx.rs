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

use std::path::{Path, PathBuf};

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

/// The error for a PC without QEMU: Cua installs it on the first create.
pub fn qemu_missing() -> VmmError {
    VmmError::missing(
        "QEMU",
        "microVM Spaces on Windows run in QEMU 9 or later: creating a microVM installs it (one administrator prompt), or `cua runtime setup microvm` does it now",
    )
}

/// The QEMU release Cua installs on Windows (the QEMU project's own build).
pub const QEMU_VERSION: &str = "11.1.0";

/// File name of the pinned installer.
pub const QEMU_INSTALLER: &str = "qemu-w64-setup-20260811.exe";

/// Disk the installed QEMU takes (about 1.2 GB), with room to spare.
pub const QEMU_INSTALLED_BYTES: u64 = 2 << 30;

/// The pinned QEMU for Windows installer (NSIS; `/S` installs silently into
/// `C:\Program Files\qemu`, which [`qemu_bin`] looks in).
pub fn qemu_pin() -> crate::managed::Pin {
    crate::managed::Pin {
        url: "https://qemu.weilnetz.de/w64/qemu-w64-setup-20260811.exe",
        sha256: "f98a8aeb5f7faea9765b6dee28316c266cd179d80354a2fed8e50176f9a2e59f",
        size: 206_615_928,
    }
}

/// The winget package of the same QEMU, the fallback when the download fails.
pub const QEMU_WINGET_ID: &str = "SoftwareFreedomConservancy.QEMU";

/// Where the QEMU a PC has stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QemuState {
    Missing,
    /// Older than [`MIN_QEMU_MAJOR`].
    Old,
    Ok,
}

/// `found`: whether a QEMU was found; `major`: its version's major, when it
/// could be read (an unreadable version is taken as good).
pub fn qemu_state(found: bool, major: Option<u32>) -> QemuState {
    match (found, major) {
        (false, _) => QemuState::Missing,
        (true, Some(m)) if m < MIN_QEMU_MAJOR => QemuState::Old,
        _ => QemuState::Ok,
    }
}

/// What a microVM create has to set up on this PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// The Windows feature Windows Hypervisor Platform is off.
    pub feature: bool,
    /// QEMU is missing or too old.
    pub qemu: bool,
}

impl Needs {
    pub fn any(self) -> bool {
        self.feature || self.qemu
    }

    /// The sentence the progress line and the prompt use.
    pub fn describe(self) -> String {
        match (self.feature, self.qemu) {
            (true, true) => {
                format!("turn on Windows Hypervisor Platform and install QEMU {QEMU_VERSION}")
            }
            (true, false) => "turn on Windows Hypervisor Platform".into(),
            (false, true) => format!("install QEMU {QEMU_VERSION}"),
            (false, false) => "nothing to set up".into(),
        }
    }
}

/// What is missing: the feature when WHP says it is off, QEMU when missing or old.
pub fn needs(w: &Whpx, qemu: QemuState) -> Needs {
    Needs {
        feature: *w == Whpx::FeatureOff,
        qemu: qemu != QemuState::Ok,
    }
}

/// QEMU on this PC: found, and its version.
pub async fn qemu_state_now() -> QemuState {
    let Some(q) = qemu_bin() else {
        return QemuState::Missing;
    };
    let major = host::run(&q, &["--version"])
        .await
        .ok()
        .and_then(|s| s.lines().next().map(qemu_major))
        .flatten();
    qemu_state(true, major)
}

/// Quotes `s` as a PowerShell single-quoted string.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The script the one administrator prompt runs: turns the feature on when
/// `feature`, runs the QEMU installer silently when `installer` is given, and
/// appends `dism=<exit code>` and `qemu=<exit code>` lines to `result` (an
/// elevated process's output cannot be read, a file can).
pub fn elevated_script(feature: bool, installer: Option<&Path>, result: &Path) -> String {
    let out = ps_quote(&result.to_string_lossy());
    let mut s = String::new();
    s.push_str("$ErrorActionPreference = 'Continue'\r\n");
    s.push_str(&format!("$out = {out}\r\n"));
    if feature {
        s.push_str(
            "try { $p = Start-Process -FilePath dism.exe -ArgumentList \
             '/online','/enable-feature','/featurename:HypervisorPlatform','/all','/norestart' \
             -Wait -PassThru; Add-Content -Path $out -Value (\"dism=\" + $p.ExitCode) } \
             catch { Add-Content -Path $out -Value (\"dism-error=\" + $_.Exception.Message) }\r\n",
        );
    }
    if let Some(i) = installer {
        s.push_str(&format!(
            "try {{ $p = Start-Process -FilePath {} -ArgumentList '/S' -Wait -PassThru; \
             Add-Content -Path $out -Value (\"qemu=\" + $p.ExitCode) }} \
             catch {{ Add-Content -Path $out -Value (\"qemu-error=\" + $_.Exception.Message) }}\r\n",
            ps_quote(&i.to_string_lossy())
        ));
    }
    s
}

/// The PowerShell that runs the script file `script` elevated (one UAC prompt)
/// and prints the elevated PowerShell's exit code.
pub fn elevate_command(script: &Path) -> String {
    // Start-Process does not quote array elements: the path carries its own quotes.
    let file = ps_quote(&format!("\"{}\"", script.to_string_lossy()));
    format!(
        "$p = Start-Process -FilePath powershell.exe -ArgumentList '-NoProfile','-NonInteractive',\
         '-ExecutionPolicy','Bypass','-File',{file} -Verb RunAs -Wait -PassThru; \
         [Console]::Out.Write($p.ExitCode)"
    )
}

/// Reads the result lines [`elevated_script`] wrote: `feature` and `qemu` say
/// what was asked. A step without a line did not run to its end.
pub fn setup_outcome(
    text: &str,
    feature: bool,
    qemu: bool,
) -> std::result::Result<Enabled, String> {
    let find = |key: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(key).map(|v| v.trim().to_string()))
    };
    let mut enabled = Enabled::Ready;
    if feature {
        if let Some(e) = find("dism-error=") {
            return Err(format!(
                "turning on Windows Hypervisor Platform failed: {e}"
            ));
        }
        let code = find("dism=").ok_or("turning on Windows Hypervisor Platform did not finish")?;
        let code: i32 = code.parse().map_err(|_| format!("dism printed {code:?}"))?;
        enabled = enable_outcome(code)
            .map_err(|e| format!("turning on Windows Hypervisor Platform failed: {e}"))?;
    }
    if qemu {
        if let Some(e) = find("qemu-error=") {
            return Err(format!("the QEMU installer failed: {e}"));
        }
        let code = find("qemu=").ok_or("the QEMU installer did not finish")?;
        if code != "0" {
            return Err(format!("the QEMU installer exited with code {code}"));
        }
    }
    Ok(enabled)
}

/// What Windows needs after [`provision`] when the feature is only now on.
pub fn restart_message(n: Needs) -> String {
    let did = if n.qemu {
        format!("QEMU {QEMU_VERSION} is installed and Windows Hypervisor Platform is on")
    } else {
        "Windows Hypervisor Platform is on".to_string()
    };
    format!(
        "{did}, but Windows needs a restart before microVMs can run: restart this PC, then \
         create the microVM again"
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

/// Sets up what `n` says is missing, with progress as the other first-use
/// downloads report it: downloads the pinned QEMU installer (SHA-256 checked),
/// then ONE administrator prompt turns the Windows feature on and runs the
/// installer silently. When the download fails, the feature (if needed) is
/// still turned on and QEMU is installed with winget instead. A refused prompt
/// or a failed install is an error that says so; [`Enabled::RestartNeeded`]
/// means the feature is on but Windows must restart before microVMs run.
pub async fn provision(n: Needs) -> Result<Enabled> {
    #[cfg(windows)]
    {
        use crate::progress::{Phase, Progress, report};
        let say = |d: String| report(Progress::phase(Phase::Preparing).detail(d));
        let dir = host::cua_home().join("downloads");
        std::fs::create_dir_all(&dir)?;
        let pin = qemu_pin();
        let mut installer = None;
        let mut download_error = None;
        if n.qemu {
            match download_installer(&dir, pin).await {
                Ok(p) => installer = Some(p),
                Err(e) => {
                    tracing::warn!("QEMU download failed, falling back to winget: {e}");
                    download_error = Some(e.to_string());
                }
            }
        }
        let mut enabled = Enabled::Ready;
        if n.feature || installer.is_some() {
            let what = Needs {
                feature: n.feature,
                qemu: installer.is_some(),
            };
            say(format!(
                "Setting up microVMs: approve the administrator prompt to {}",
                what.describe()
            ));
            let result = dir.join("microvm-setup.result");
            let script = dir.join("microvm-setup.ps1");
            let _ = std::fs::remove_file(&result);
            // A BOM: PowerShell 5 reads a BOM-less file as ANSI.
            let body = format!(
                "\u{feff}{}",
                elevated_script(n.feature, installer.as_deref(), &result)
            );
            std::fs::write(&script, body)?;
            let out = tokio::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &elevate_command(&script),
                ])
                .creation_flags(0x0800_0000)
                .stdin(std::process::Stdio::null())
                .output()
                .await?;
            let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
            let text = std::fs::read_to_string(&result).unwrap_or_default();
            let _ = std::fs::remove_file(&script);
            let _ = std::fs::remove_file(&result);
            if text.trim().is_empty() {
                let declined = err.contains("canceled") || err.contains("cancelled");
                return Err(VmmError::other(if declined {
                    format!(
                        "the administrator prompt was declined: could not {}. Create the \
                         microVM again and accept it",
                        what.describe()
                    )
                } else {
                    format!("could not {}: {}", what.describe(), err.trim())
                }));
            }
            enabled = setup_outcome(&text, what.feature, what.qemu).map_err(VmmError::other)?;
            if let Some(p) = &installer {
                let _ = std::fs::remove_file(p);
            }
        }
        if n.qemu && installer.is_none() {
            say(format!(
                "Setting up microVMs: installing QEMU {QEMU_VERSION} with winget"
            ));
            let winget = host::which("winget").ok_or_else(|| {
                VmmError::other(format!(
                    "could not install QEMU: the download failed ({}) and winget is not \
                     installed; install QEMU {QEMU_VERSION} from https://qemu.weilnetz.de/ and \
                     create the microVM again",
                    download_error.clone().unwrap_or_default()
                ))
            })?;
            host::run(
                &winget,
                &[
                    "install",
                    "-e",
                    "--id",
                    QEMU_WINGET_ID,
                    "--silent",
                    "--accept-source-agreements",
                    "--accept-package-agreements",
                ],
            )
            .await
            .map_err(|e| {
                VmmError::other(format!(
                    "could not install QEMU: the download failed ({}) and winget failed too: {e}",
                    download_error.unwrap_or_default()
                ))
            })?;
        }
        if n.qemu {
            match qemu_state_now().await {
                QemuState::Ok => {}
                QemuState::Missing => {
                    return Err(VmmError::other(
                        "QEMU installed but qemu-system-x86_64.exe is not in C:\\Program Files\\qemu",
                    ));
                }
                QemuState::Old => {
                    return Err(VmmError::other(format!(
                        "the QEMU on this PC is older than {MIN_QEMU_MAJOR} and the new one did \
                         not take its place: uninstall it, then create the microVM again"
                    )));
                }
            }
        }
        Ok(enabled)
    }
    #[cfg(not(windows))]
    {
        let _ = n;
        Err(VmmError::other("QEMU microVMs run on Windows"))
    }
}

/// Downloads the pinned installer into `dir` (resuming, retrying, SHA-256
/// checked) and returns its path; a copy already there that matches is reused.
#[cfg(windows)]
async fn download_installer(dir: &Path, pin: crate::managed::Pin) -> Result<PathBuf> {
    use crate::progress::{Meter, Phase, Progress, report};
    crate::disk::ensure_space(dir, pin.size + QEMU_INSTALLED_BYTES, "install QEMU")?;
    let to = dir.join(QEMU_INSTALLER);
    if to.is_file() && sha256_file(&to).is_ok_and(|h| h == pin.sha256) {
        return Ok(to);
    }
    let (mut done, mut meter) = (0u64, Meter::new());
    crate::fetch::fetch(
        &crate::fetch::Fetch {
            url: pin.url,
            bearer: None,
            size: Some(pin.size),
            sha256: pin.sha256,
        },
        &to,
        &crate::fetch::Policy::default(),
        &mut |n| {
            done += n;
            if let Some(t) = meter.sample_at(std::time::Instant::now(), done, pin.size) {
                report(
                    Progress::phase(Phase::Preparing)
                        .detail(format!(
                            "Setting up microVMs: downloading QEMU {QEMU_VERSION}"
                        ))
                        .bytes(t)
                        .fraction(done as f64 / pin.size.max(1) as f64),
                );
            }
        },
    )
    .await?;
    Ok(to)
}

#[cfg(windows)]
fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
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

    #[test]
    fn the_qemu_pin_is_the_official_installer() {
        let p = qemu_pin();
        assert!(p.url.starts_with("https://qemu.weilnetz.de/w64/"));
        assert!(p.url.ends_with(QEMU_INSTALLER));
        assert_eq!(p.size, 206_615_928);
        assert_eq!(p.sha256.len(), 64);
        assert!(
            p.sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
        );
        assert_eq!(
            p.sha256,
            "f98a8aeb5f7faea9765b6dee28316c266cd179d80354a2fed8e50176f9a2e59f"
        );
    }

    #[test]
    fn what_is_missing_decides_what_the_create_sets_up() {
        let n = needs(&Whpx::Ready, QemuState::Ok);
        assert!(!n.any());
        assert_eq!(
            needs(&Whpx::Ready, QemuState::Missing),
            Needs {
                feature: false,
                qemu: true
            }
        );
        assert_eq!(
            needs(&Whpx::Ready, QemuState::Old),
            Needs {
                feature: false,
                qemu: true
            }
        );
        assert_eq!(
            needs(&Whpx::FeatureOff, QemuState::Missing),
            Needs {
                feature: true,
                qemu: true
            }
        );
        assert_eq!(
            needs(&Whpx::FeatureOff, QemuState::Ok),
            Needs {
                feature: true,
                qemu: false
            }
        );
        // Firmware or restart problems are not something an install fixes.
        assert!(!needs(&Whpx::NoHypervisor, QemuState::Ok).any());
        assert_eq!(qemu_state(false, None), QemuState::Missing);
        assert_eq!(qemu_state(true, Some(8)), QemuState::Old);
        assert_eq!(qemu_state(true, Some(9)), QemuState::Ok);
        assert_eq!(qemu_state(true, None), QemuState::Ok);
    }

    #[test]
    fn one_elevated_script_enables_the_feature_and_installs_qemu() {
        let inst = Path::new(r"C:\Users\a b\.cua\downloads\qemu-w64-setup-20260811.exe");
        let res = Path::new(r"C:\Users\it's\r.result");
        let s = elevated_script(true, Some(inst), res);
        assert!(s.contains("dism.exe") && s.contains("/featurename:HypervisorPlatform"));
        assert!(s.contains("/norestart") && s.contains("'/S'"));
        assert!(s.contains(&format!("'{}'", inst.display())));
        // A quote in a path is doubled, never ends the string.
        assert!(s.contains(r"'C:\Users\it''s\r.result'"));
        let only_qemu = elevated_script(false, Some(inst), res);
        assert!(!only_qemu.contains("dism.exe") && only_qemu.contains("'/S'"));
        let only_feature = elevated_script(true, None, res);
        assert!(only_feature.contains("dism.exe") && !only_feature.contains("'/S'"));
    }

    #[test]
    fn the_prompt_runs_the_script_file_elevated_with_its_path_quoted() {
        let c = elevate_command(Path::new(r"C:\Users\a b\s.ps1"));
        assert!(c.contains("-Verb RunAs") && c.contains("-Wait") && c.contains("-File"));
        assert!(c.contains(r#"'"C:\Users\a b\s.ps1"'"#), "{c}");
    }

    #[test]
    fn setup_results_are_read_per_step() {
        assert_eq!(
            setup_outcome("dism=0\nqemu=0\n", true, true),
            Ok(Enabled::Ready)
        );
        assert_eq!(
            setup_outcome("dism=3010\nqemu=0\n", true, true),
            Ok(Enabled::RestartNeeded)
        );
        assert_eq!(setup_outcome("qemu=0", false, true), Ok(Enabled::Ready));
        assert!(
            setup_outcome("dism=0\n", true, true)
                .unwrap_err()
                .contains("installer did not finish")
        );
        assert!(
            setup_outcome("", true, false)
                .unwrap_err()
                .contains("did not finish")
        );
        assert!(
            setup_outcome("dism=0\nqemu=1603", true, true)
                .unwrap_err()
                .contains("1603")
        );
        assert!(
            setup_outcome("qemu-error=boom", false, true)
                .unwrap_err()
                .contains("boom")
        );
        assert!(setup_outcome("dism=5", true, false).is_err());
    }

    #[test]
    fn the_restart_message_says_what_is_done_and_not_ready() {
        let m = restart_message(Needs {
            feature: true,
            qemu: true,
        });
        assert!(m.contains("QEMU 11.1.0 is installed") && m.contains("restart this PC"));
        assert!(
            !restart_message(Needs {
                feature: true,
                qemu: false
            })
            .contains("QEMU")
        );
        assert!(!qemu_missing().to_string().contains("winget"));
    }

    /// The real call, on this PC (Windows only): it answers without elevation.
    #[cfg(windows)]
    #[test]
    fn whp_answers_without_elevation() {
        assert_ne!(status(), Whpx::NotWindows);
    }
}
