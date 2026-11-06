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
use cua_vmm::{Arch, GuestOs, ImageSource, Probe, Runtime, StartSpec};

use super::build::{InstallPlan, Installer};
use super::table::WindowsArch;
use super::unattend::SPACESD_PORT;
use crate::error::{ImageError, Result};

/// The guest architecture as the VMM names it.
pub fn vmm_arch(a: WindowsArch) -> Arch {
    match a {
        WindowsArch::Amd64 => Arch::X86_64,
        WindowsArch::Arm64 => Arch::Aarch64,
    }
}

/// Errors when `guest` could only run under software emulation on `host`.
pub fn refuse_software_emulation(guest: WindowsArch, host: WindowsArch) -> Result<()> {
    if guest == host {
        return Ok(());
    }
    Err(ImageError::Build {
        step: "windows".into(),
        detail: format!(
            "this {} computer would run {} Windows under software emulation, which takes hours; cua does not do that. \
             Use {} Windows here, or build on a {} computer",
            host.as_str(),
            guest.as_str(),
            host.as_str(),
            guest.as_str()
        ),
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
        .probe(Probe::tcp(SPACESD_PORT))
        .ready_timeout(plan.timeout);
    spec.disk_size_gb = Some(plan.disk_gb);
    spec.extra_args = extra_drives(plan);
    spec
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
        // "Press any key to boot from CD" stays up a few seconds; missing it
        // leaves the firmware in the EFI shell. Press Enter every half second
        // for a minute from power-on (the installer's own window takes far
        // longer than that to appear, so nothing here reaches its buttons).
        let keys = {
            let rt = rt.clone();
            let name = name.clone();
            tokio::spawn(async move {
                for _ in 0..120 {
                    if let Ok(mut q) = rt.qmp(&name).await {
                        let _ = q.send_keys(&["ret"]).await;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            })
        };
        let started = rt.start(&start_spec(plan)).await;
        keys.abort();
        started?;
        // cua-spacesd answers: the install finished. Shut down cleanly so the
        // disk is consistent, then keep the disk.
        rt.stop(&name).await?;
        let disk = rt.load(&name)?.disk;
        let out = plan.work_dir.join("installed.qcow2");
        if out.exists() {
            std::fs::remove_file(&out)?;
        }
        if std::fs::rename(&disk, &out).is_err() {
            std::fs::copy(&disk, &out)?;
        }
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
    fn the_spec_boots_the_iso_on_a_blank_disk_and_waits_for_spacesd() {
        let s = start_spec(&plan(WindowsArch::Amd64));
        assert_eq!(s.os, GuestOs::Windows);
        assert_eq!(s.disk_size_gb, Some(64));
        assert!(matches!(&s.image, ImageSource::Disk { path } if path.ends_with("w.iso")));
        assert!(s.probes.contains(&Probe::tcp(3211)));
        assert!(s.ports.contains(&3211));
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
            e.contains("software emulation") && e.contains("hours"),
            "{e}"
        );
    }
}
