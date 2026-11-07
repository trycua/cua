//! The install on a host QEMU can accelerate itself: KVM on Linux, HVF on a
//! Mac (the ARM64 guest on Apple silicon), through `cua-vmm`'s QEMU runtime,
//! which already boots a Windows ISO on a blank disk (AHCI, UEFI, and the TPM
//! and Secure Boot firmware an ARM64 guest needs). This adds the answer-file
//! disc and the virtio disc as drives, presses Enter through the "Press any key
//! to boot from CD" prompt, and waits for cua-spacesd.
//!
//! Never software emulation: [`refuse_software_emulation`] stops a build whose
//! guest would run under TCG (an x64 guest on an ARM Mac, say) before it starts,
//! because that install takes hours.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cua_vmm::qemu::{QemuConfig, QemuRuntime};
use cua_vmm::{Arch, GuestOs, ImageSource, Probe, Runtime, StartSpec, Status};

use super::build::{InstallPlan, Installer};
use super::table::WindowsArch;
use crate::error::{ImageError, Result};

/// The guest architecture as the VMM names it.
pub fn vmm_arch(a: WindowsArch) -> Arch {
    match a {
        WindowsArch::Amd64 => Arch::X86_64,
        WindowsArch::Arm64 => Arch::Aarch64,
    }
}

/// Errors when `guest` could only run under software emulation on `host`,
/// unless the person accepted that (`CUA_ALLOW_SOFTWARE_EMULATION=1`).
pub fn refuse_software_emulation(guest: WindowsArch, host: WindowsArch) -> Result<()> {
    if guest == host || cua_vmm::qemu::software_emulation_allowed() {
        return Ok(());
    }
    Err(ImageError::Build {
        step: "windows".into(),
        detail: cua_vmm::qemu::windows_emulation_message(vmm_arch(guest), vmm_arch(host)),
    })
}

/// The drives that carry the answer-file disc and the virtio disc, as QEMU
/// arguments. x64 hangs them on the AHCI controller the runtime already has
/// for the system disk (`ahci.0`); ARM64 on its xHCI controller as USB
/// storage.
pub fn extra_drives(plan: &InstallPlan) -> Vec<String> {
    let mut a = Vec::new();
    for (i, iso) in [&plan.data_iso, &plan.virtio_iso].into_iter().enumerate() {
        let id = format!("cuadisc{i}");
        a.push("-drive".into());
        a.push(format!(
            "file={},if=none,id={id},format=raw,readonly=on,media=cdrom",
            iso.display()
        ));
        a.push("-device".into());
        a.push(match plan.arch {
            WindowsArch::Amd64 => format!("ide-cd,drive={id},bus=ahci.{}", i + 1),
            WindowsArch::Arm64 => format!("usb-storage,drive={id},bus=xhci.0"),
        });
    }
    a
}

/// The start spec of the install VM.
pub fn start_spec(plan: &InstallPlan) -> StartSpec {
    let mut spec = StartSpec::new(&plan.name, ImageSource::disk(&plan.windows_iso))
        .os(GuestOs::Windows)
        .arch(vmm_arch(plan.arch))
        .cpus(plan.cpus)
        .memory_mb(plan.memory_mb)
        // No probe: the guest installs cua-spacesd, seals itself and powers
        // off; the installer waits for that power-off.
        .probe(Probe::None)
        .ready_timeout(plan.timeout);
    spec.disk_size_gb = Some(plan.disk_gb);
    spec.extra_args = extra_drives(plan);
    spec
}

/// What the VM's screen shows, as far as the boot prompt is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Nothing drawn yet.
    Black,
    /// "Press any key to boot from CD or DVD...": text in the top rows of an
    /// otherwise black screen.
    BootPrompt,
    /// Anything else (firmware logo, shell, Windows).
    Other,
}

/// Classifies a binary PPM (P6) frame.
pub(crate) fn classify_screen(ppm: &[u8]) -> Screen {
    // "P6", width, height, maxval, one whitespace byte, then RGB bytes.
    let mut pos = 0;
    let mut token = || -> Option<&[u8]> {
        while ppm.get(pos).is_some_and(|b| b.is_ascii_whitespace()) {
            pos += 1;
        }
        let start = pos;
        while ppm.get(pos).is_some_and(|b| !b.is_ascii_whitespace()) {
            pos += 1;
        }
        (pos > start).then(|| &ppm[start..pos])
    };
    let num = |t: Option<&[u8]>| -> Option<usize> { std::str::from_utf8(t?).ok()?.parse().ok() };
    if token() != Some(b"P6") {
        return Screen::Other;
    }
    let (Some(w), Some(h), Some(_max)) = (num(token()), num(token()), num(token())) else {
        return Screen::Other;
    };
    let data = &ppm[(pos + 1).min(ppm.len())..];
    if w == 0 || h == 0 || data.len() < w * h * 3 {
        return Screen::Other;
    }
    let (mut lit, mut lit_below) = (0usize, 0usize);
    for y in 0..h {
        let row = &data[y * w * 3..(y + 1) * w * 3];
        let n = row
            .chunks_exact(3)
            .filter(|p| p[0] > 64 || p[1] > 64 || p[2] > 64)
            .count();
        lit += n;
        if y >= 32 {
            lit_below += n;
        }
    }
    match (lit, lit_below) {
        (0..=8, _) => Screen::Black,
        (_, 0) => Screen::BootPrompt,
        _ => Screen::Other,
    }
}

/// Stops a background task when dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Ends the install VM when the build is dropped half-way (a cancelled
/// create): QEMU runs detached from this process, so without this it would go
/// on installing Windows with nobody waiting for it.
struct VmGuard {
    rt: Arc<QemuRuntime>,
    name: String,
    armed: bool,
}

impl Drop for VmGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (rt, name) = (self.rt.clone(), self.name.clone());
        if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                let _ = rt.stop(&name).await;
                let _ = rt.delete(&name).await;
            });
        }
    }
}

/// Runs the install under cua-vmm's QEMU runtime.
pub struct QemuInstaller {
    runtime: Arc<QemuRuntime>,
}

impl QemuInstaller {
    /// A runtime of its own under `$CUA_HOME/build/windows-qemu`, so the build
    /// VM never shows up among Spaces.
    pub fn new() -> Self {
        let cfg = QemuConfig {
            root: cua_vmm::host::cua_home().join("build").join("windows-qemu"),
            allow_install: true,
            install_swtpm: true,
            // Windows shuts down slowly; a forced quit loses the last writes.
            graceful_stop: Duration::from_secs(300),
            warm: false,
            ..QemuConfig::default()
        };
        Self {
            runtime: Arc::new(QemuRuntime::new(cfg)),
        }
    }
}

impl Default for QemuInstaller {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Installer for QemuInstaller {
    async fn install(&self, plan: &InstallPlan) -> Result<PathBuf> {
        refuse_software_emulation(plan.arch, WindowsArch::host())?;
        let rt = self.runtime.clone();
        let name = plan.name.clone();
        // Start from nothing: a half-built VM of an earlier attempt is not reused.
        let _ = rt.delete(&name).await;
        // "Press any key to boot from CD" shows a few seconds after power-on
        // and times out; missing it leaves the firmware in the EFI shell. An
        // Enter after that reaches Setup's own window, whose focused button is
        // Cancel. So Enter is pressed only while the screen shows the prompt
        // (black with text in the top rows), and never once it has gone.
        let keys = {
            let rt = rt.clone();
            let name = name.clone();
            let frame = plan.work_dir.join("screen.ppm");
            AbortOnDrop(tokio::spawn(async move {
                let started = std::time::Instant::now();
                let (mut pressed, mut seen) = (0u32, false);
                while started.elapsed() < Duration::from_secs(150) {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let Ok(mut q) = rt.qmp(&name).await else {
                        continue;
                    };
                    if q.screendump_ppm(&frame).await.is_err() {
                        continue;
                    }
                    let Ok(bytes) = tokio::fs::read(&frame).await else {
                        continue;
                    };
                    match classify_screen(&bytes) {
                        Screen::BootPrompt => {
                            seen = true;
                            if q.send_keys(&["ret"]).await.is_ok() {
                                pressed += 1;
                            }
                        }
                        // The prompt was there and is gone: the CD is booting.
                        Screen::Other if seen => break,
                        Screen::Other | Screen::Black => {}
                    }
                }
                let _ = tokio::fs::remove_file(&frame).await;
                tracing::info!(pressed, seen, "boot prompt handled");
            }))
        };
        let mut vm = VmGuard {
            rt: rt.clone(),
            name: name.clone(),
            armed: true,
        };
        // The install VM is the build's, not a Space being created: its own
        // boot reports (Booting, waiting for services) must not reach the
        // create's progress, where they would push the bar past the install.
        let quiet: cua_vmm::progress::Sink = Arc::new(|_: &cua_vmm::progress::Progress| {});
        let started = cua_vmm::progress::scope(quiet, rt.start(&start_spec(plan))).await;
        // The presses go on after the VM is up: the prompt shows a few
        // seconds after power-on, and a start with no probe returns at once.
        if let Err(e) = started {
            keys.0.abort();
            return Err(e.into());
        }
        // The guest installs Windows, installs cua-spacesd, seals itself and
        // powers off. Wait for the power-off (QEMU exits), then keep the disk.
        let deadline = std::time::Instant::now() + plan.timeout;
        let mut last_report = std::time::Instant::now();
        loop {
            match rt.status(&name).await {
                Ok(Status::Stopped) => break,
                Ok(_) => {}
                // The VM is gone: it powered itself off.
                Err(_) => break,
            }
            if std::time::Instant::now() > deadline {
                keys.0.abort();
                let _ = rt.stop(&name).await;
                return Err(ImageError::Build {
                    step: "windows install".into(),
                    detail: format!(
                        "Windows did not finish installing in {} minutes (the install VM was stopped; its files are in {})",
                        plan.timeout.as_secs() / 60,
                        plan.work_dir.display()
                    ),
                });
            }
            if last_report.elapsed() >= Duration::from_secs(20) {
                last_report = std::time::Instant::now();
                if let Ok(st) = rt.load(&name) {
                    super::build::report_install_progress(&st.disk, plan.disk_gb);
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        keys.0.abort();
        let _ = rt.stop(&name).await;
        let disk = rt.load(&name)?.disk;
        let out = plan.work_dir.join("installed.qcow2");
        if out.exists() {
            std::fs::remove_file(&out)?;
        }
        if std::fs::rename(&disk, &out).is_err() {
            std::fs::copy(&disk, &out)?;
        }
        vm.armed = false;
        let _ = rt.delete(&name).await;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(arch: WindowsArch) -> InstallPlan {
        InstallPlan {
            name: "cua-build-windows-11".into(),
            arch,
            windows_iso: "/i/w.iso".into(),
            data_iso: "/w/unattend.iso".into(),
            virtio_iso: "/i/virtio-win.iso".into(),
            disk_gb: 64,
            cpus: 4,
            memory_mb: 8192,
            work_dir: "/w".into(),
            timeout: Duration::from_secs(60),
        }
    }

    /// A P6 frame of `w` x `h` black pixels with `lit(x, y)` pixels white.
    fn frame(w: usize, h: usize, lit: impl Fn(usize, usize) -> bool) -> Vec<u8> {
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for y in 0..h {
            for x in 0..w {
                let v = if lit(x, y) { 255 } else { 0 };
                out.extend_from_slice(&[v, v, v]);
            }
        }
        out
    }

    #[test]
    fn the_boot_prompt_is_told_from_black_the_shell_and_setup() {
        // Nothing drawn.
        assert_eq!(classify_screen(&frame(80, 60, |_, _| false)), Screen::Black);
        // Text in the top rows only: the prompt.
        assert_eq!(
            classify_screen(&frame(80, 60, |x, y| y < 12 && x % 3 == 0 && x < 50)),
            Screen::BootPrompt
        );
        // Text all down the screen (the EFI shell, Setup): not the prompt.
        assert_eq!(
            classify_screen(&frame(80, 60, |x, y| y % 7 == 0 && x < 50)),
            Screen::Other
        );
        // A logo in the middle (the firmware splash): not the prompt either.
        assert_eq!(
            classify_screen(&frame(80, 60, |x, y| (30..50).contains(&x)
                && (25..35).contains(&y))),
            Screen::Other
        );
        // Garbage is never the prompt.
        assert_eq!(classify_screen(b"not a ppm"), Screen::Other);
        assert_eq!(classify_screen(b"P6\n80 60\n255\nshort"), Screen::Other);
    }

    #[test]
    fn x64_drives_sit_on_the_ahci_controller() {
        let a = extra_drives(&plan(WindowsArch::Amd64));
        assert_eq!(a.len(), 8);
        assert!(a.contains(&"ide-cd,drive=cuadisc0,bus=ahci.1".to_string()));
        assert!(a.contains(&"ide-cd,drive=cuadisc1,bus=ahci.2".to_string()));
        assert!(
            a.iter()
                .any(|x| x.starts_with("file=/w/unattend.iso,") && x.contains("media=cdrom"))
        );
        assert!(a.iter().filter(|x| x.contains("readonly=on")).count() == 2);
    }

    #[test]
    fn arm64_drives_are_usb_storage_on_xhci() {
        let a = extra_drives(&plan(WindowsArch::Arm64));
        assert!(a.contains(&"usb-storage,drive=cuadisc0,bus=xhci.0".to_string()));
        assert!(a.contains(&"usb-storage,drive=cuadisc1,bus=xhci.0".to_string()));
    }

    #[test]
    fn the_spec_boots_the_iso_on_a_blank_disk_and_waits_for_the_guests_power_off() {
        let s = start_spec(&plan(WindowsArch::Amd64));
        assert_eq!(s.os, GuestOs::Windows);
        assert_eq!(s.disk_size_gb, Some(64));
        assert!(matches!(&s.image, ImageSource::Disk { path } if path.ends_with("w.iso")));
        // No readiness probe: the guest seals itself and powers off.
        assert!(s.probes.iter().all(|p| *p == Probe::None));
        assert_eq!(s.ready_timeout, Duration::from_secs(60));
        assert_eq!(s.effective_arch(), Arch::X86_64);
        assert_eq!(
            start_spec(&plan(WindowsArch::Arm64)).effective_arch(),
            Arch::Aarch64
        );
    }

    #[test]
    fn a_foreign_architecture_is_refused_not_emulated() {
        assert!(refuse_software_emulation(WindowsArch::Amd64, WindowsArch::Amd64).is_ok());
        let e = refuse_software_emulation(WindowsArch::Amd64, WindowsArch::Arm64)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("software emulation") && e.contains("hours") && e.contains("windows:11"),
            "{e}"
        );
    }
}
