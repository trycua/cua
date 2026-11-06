//! The install on a Windows PC: QEMU with KVM inside WSL2.
//!
//! Hardware acceleration only, and never software emulation. WSL2 with
//! `/dev/kvm` (nested virtualization, on by default on Windows 11) gives KVM:
//! 15 to 40 minutes. QEMU for Windows under WHPX hangs in OVMF's flash on
//! current QEMU builds, and software emulation takes hours, so the build does
//! not use either. What dogfooding showed has to be done:
//!
//! * stage everything QEMU opens on the WSL ext4 disk, not `/mnt/c` (9p stalls
//!   the firmware and the installer) and copy the finished disk back;
//! * boot the install media first (a blank disk first leaves OVMF in the EFI
//!   shell) and press Enter every half second from power-on for a minute;
//! * give the guest's cua-spacesd a program firewall rule (done in the guest's
//!   setup script) so Windows does not prompt.
//!
//! [`probe`] answers what is missing and [`setup_steps`] says how to fix each
//! piece: the ones cua can do alone (QEMU, OVMF and swtpm inside an existing
//! distribution) it does; the ones that need an administrator or a reboot
//! (turning WSL on) it explains.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use super::build::{InstallPlan, Installer};
use super::table::WindowsArch;
use crate::error::{ImageError, Result};

/// Where QEMU's monitor and the guest's cua-spacesd are forwarded to on the
/// Windows side.
pub const QMP_PORT: u16 = 4444;
pub const SPACESD_FORWARD_PORT: u16 = 18000;

/// Runs a shell script inside the default WSL distribution.
#[async_trait]
pub trait Wsl: Send + Sync {
    /// Whether `wsl.exe` exists at all.
    fn available(&self) -> bool;
    /// `(exit code, stdout)` of `sh -c script`.
    async fn sh(&self, script: &str, timeout: Duration) -> Result<(i32, String)>;
}

/// The real `wsl.exe`.
pub struct WslExe;

#[async_trait]
impl Wsl for WslExe {
    fn available(&self) -> bool {
        cua_vmm::host::which("wsl").is_some()
    }

    async fn sh(&self, script: &str, timeout: Duration) -> Result<(i32, String)> {
        let run = tokio::process::Command::new("wsl.exe")
            .args(["-e", "sh", "-c", script])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(timeout, run)
            .await
            .map_err(|_| ImageError::Build {
                step: "wsl".into(),
                detail: format!("timed out after {timeout:?}: {script}"),
            })??;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        ))
    }
}

/// What this Windows PC has for a hardware-accelerated Windows build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WslStatus {
    /// `wsl.exe` is there and a distribution answers.
    pub wsl: bool,
    /// `/dev/kvm` opens for reading and writing inside it.
    pub kvm: bool,
    pub qemu: bool,
    pub qemu_img: bool,
    pub ovmf: bool,
}

impl WslStatus {
    pub fn ready(&self) -> bool {
        self.wsl && self.kvm && self.qemu && self.qemu_img && self.ovmf
    }
}

/// Probes the PC.
pub async fn probe(wsl: &dyn Wsl) -> WslStatus {
    if !wsl.available() {
        return WslStatus::default();
    }
    let script = "echo wsl; test -r /dev/kvm -a -w /dev/kvm && echo kvm; \
        command -v qemu-system-x86_64 >/dev/null && echo qemu; \
        command -v qemu-img >/dev/null && echo qemu-img; \
        test -r /usr/share/OVMF/OVMF_CODE_4M.fd -o -r /usr/share/OVMF/OVMF_CODE.fd && echo ovmf";
    match wsl.sh(script, Duration::from_secs(60)).await {
        Ok((_, out)) => {
            let has = |w: &str| out.lines().any(|l| l.trim() == w);
            WslStatus {
                wsl: has("wsl"),
                kvm: has("kvm"),
                qemu: has("qemu"),
                qemu_img: has("qemu-img"),
                ovmf: has("ovmf"),
            }
        }
        Err(_) => WslStatus::default(),
    }
}

/// One thing to fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Setup {
    /// cua runs this inside WSL itself.
    Auto {
        what: &'static str,
        script: &'static str,
    },
    /// The person has to do it (administrator rights, a reboot, the firmware).
    Guide {
        what: &'static str,
        how: &'static str,
    },
}

/// The apt line cua runs in WSL for QEMU, OVMF, swtpm and the disk tools.
pub const INSTALL_QEMU_IN_WSL: &str = "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends qemu-system-x86 qemu-utils ovmf swtpm";

/// What to do about each missing piece, in order.
pub fn setup_steps(s: &WslStatus) -> Vec<Setup> {
    if !s.wsl {
        return vec![Setup::Guide {
            what: "turn on WSL2 with an Ubuntu distribution",
            how: "run `wsl --install -d Ubuntu` in an administrator terminal, restart when it asks, open Ubuntu once to finish its setup, then try again",
        }];
    }
    let mut v = Vec::new();
    if !s.kvm {
        v.push(Setup::Guide {
            what: "give WSL2 hardware virtualization (/dev/kvm)",
            how: "turn on virtualization in the PC's firmware settings, make sure `nestedVirtualization=true` is not disabled in %UserProfile%\\.wslconfig, run `wsl --update` and `wsl --shutdown`, then try again. cua never falls back to software emulation: it takes hours",
        });
    }
    if !(s.qemu && s.qemu_img && s.ovmf) {
        v.push(Setup::Auto {
            what: "install QEMU, OVMF and swtpm inside WSL",
            script: INSTALL_QEMU_IN_WSL,
        });
    }
    v
}

/// `C:\Users\me\x` as `/mnt/c/Users/me/x`.
pub fn to_wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).to_string();
    match s.as_bytes() {
        [d, b':', b'/', ..] => format!("/mnt/{}{}", (*d as char).to_ascii_lowercase(), &s[2..]),
        _ => s,
    }
}

/// The QEMU command line (a shell string) for the install inside WSL, with
/// every file on the staging directory `stage` (the WSL ext4 disk).
pub fn qemu_command(plan: &InstallPlan, stage: &str) -> String {
    let m = plan.memory_mb;
    let c = plan.cpus;
    let args = [
        "qemu-system-x86_64 -enable-kvm -cpu host".to_string(),
        "-machine q35,smm=off".into(),
        format!("-m {m} -smp {c}"),
        "-drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd".into(),
        format!("-drive if=pflash,format=raw,file={stage}/efivars.fd"),
        // AHCI, as the Space's own QEMU boots it, so the sealed disk boots there.
        format!("-drive file={stage}/disk.qcow2,id=disk0,format=qcow2,if=none"),
        "-device ahci,id=ahci".into(),
        "-device ide-hd,drive=disk0,bus=ahci.0,bootindex=1".into(),
        // The Windows ISO first: a blank disk first leaves OVMF in the EFI shell.
        format!("-drive file={stage}/windows.iso,if=none,media=cdrom,readonly=on,id=cd0"),
        "-device ide-cd,drive=cd0,bus=ahci.1,bootindex=0".into(),
        format!("-drive file={stage}/unattend.iso,if=none,media=cdrom,readonly=on,id=cd1"),
        "-device ide-cd,drive=cd1,bus=ahci.2".into(),
        format!("-drive file={stage}/virtio-win.iso,if=none,media=cdrom,readonly=on,id=cd2"),
        "-device ide-cd,drive=cd2,bus=ahci.3".into(),
        "-display none".into(),
        format!("-qmp tcp:0.0.0.0:{QMP_PORT},server,nowait"),
        format!(
            "-netdev user,id=net0,hostfwd=tcp:0.0.0.0:{SPACESD_FORWARD_PORT}-:{}",
            super::unattend::SPACESD_PORT
        ),
        "-device virtio-net-pci,netdev=net0,mac=52:55:00:d1:55:01".into(),
        "-global ICH9-LPC.disable_s3=1 -global ICH9-LPC.disable_s4=1".into(),
        "-rtc base=localtime".into(),
    ];
    args.join(" ")
}

async fn qmp(cmd: &str) -> bool {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let go = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", QMP_PORT))
            .await
            .ok()?;
        let mut buf = vec![0u8; 4096];
        let _n = s.read(&mut buf).await.ok()?;
        s.write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
            .await
            .ok()?;
        let _n = s.read(&mut buf).await.ok()?;
        s.write_all(format!("{cmd}\n").as_bytes()).await.ok()?;
        let _n = s.read(&mut buf).await.ok()?;
        Some(())
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(5), go).await,
        Ok(Some(()))
    )
}

/// Installs through QEMU with KVM in WSL2.
pub struct WslInstaller<W: Wsl + 'static> {
    pub wsl: std::sync::Arc<W>,
}

impl WslInstaller<WslExe> {
    pub fn new() -> Self {
        Self {
            wsl: std::sync::Arc::new(WslExe),
        }
    }
}

impl Default for WslInstaller<WslExe> {
    fn default() -> Self {
        Self::new()
    }
}

/// The text of an error that lists what to do.
pub fn not_ready_message(s: &WslStatus) -> String {
    let steps: Vec<String> = setup_steps(s)
        .into_iter()
        .map(|st| match st {
            Setup::Auto { what, .. } => format!("cua will {what}"),
            Setup::Guide { what, how } => format!("{what}: {how}"),
        })
        .collect();
    format!(
        "this Windows PC is not ready to build a Windows image with hardware acceleration. {}",
        steps.join(". ")
    )
}

#[async_trait]
impl<W: Wsl + 'static> Installer for WslInstaller<W> {
    async fn install(&self, plan: &InstallPlan) -> Result<PathBuf> {
        if plan.arch != WindowsArch::Amd64 {
            return Err(ImageError::Build {
                step: "windows".into(),
                detail: "a Windows PC builds x64 Windows; ARM64 Windows would run under software emulation, which cua does not do".into(),
            });
        }
        let mut status = probe(&*self.wsl).await;
        if status.wsl && status.kvm && !(status.qemu && status.qemu_img && status.ovmf) {
            // Inside an existing distribution cua can do this itself.
            let (code, _) = self
                .wsl
                .sh(
                    &format!("sudo -n sh -c '{INSTALL_QEMU_IN_WSL}' || ({INSTALL_QEMU_IN_WSL})"),
                    Duration::from_secs(900),
                )
                .await?;
            if code == 0 {
                status = probe(&*self.wsl).await;
            }
        }
        if !status.ready() {
            return Err(ImageError::Build {
                step: "windows".into(),
                detail: not_ready_message(&status),
            });
        }
        let stage = format!("$HOME/cua-build/{}", plan.name);
        let to = |p: &Path| to_wsl_path(p);
        let stage_script = format!(
            "set -e; mkdir -p {stage}; \
             cp -u '{}' {stage}/windows.iso; cp -u '{}' {stage}/unattend.iso; cp -u '{}' {stage}/virtio-win.iso; \
             rm -f {stage}/disk.qcow2; qemu-img create -q -f qcow2 {stage}/disk.qcow2 {}G; \
             cp -f /usr/share/OVMF/OVMF_VARS_4M.fd {stage}/efivars.fd",
            to(&plan.windows_iso),
            to(&plan.data_iso),
            to(&plan.virtio_iso),
            plan.disk_gb,
        );
        let (code, _) = self
            .wsl
            .sh(&stage_script, Duration::from_secs(1800))
            .await?;
        if code != 0 {
            return Err(ImageError::Build {
                step: "windows".into(),
                detail: "could not stage the install files on the WSL disk (is it full?)".into(),
            });
        }
        let run = qemu_command(plan, &stage);
        let wsl = self.wsl.clone();
        let timeout = plan.timeout;
        let vm =
            tokio::spawn(async move { wsl.sh(&run, timeout + Duration::from_secs(600)).await });
        let result = drive(plan).await;
        // Whatever happened, the VM goes away before we look at its disk.
        let _ = qmp(r#"{"execute":"quit"}"#).await;
        let _ = tokio::time::timeout(Duration::from_secs(60), vm).await;
        result?;
        let out = plan.work_dir.join("installed.qcow2");
        let copy = format!(
            "set -e; qemu-img convert -O qcow2 {stage}/disk.qcow2 '{}'; rm -rf {stage}",
            to(&out)
        );
        let (code, _) = self.wsl.sh(&copy, Duration::from_secs(1800)).await?;
        if code != 0 || !out.exists() {
            return Err(ImageError::Build {
                step: "windows".into(),
                detail: "could not copy the installed disk back from the WSL disk".into(),
            });
        }
        Ok(out)
    }
}

/// Presses through the boot prompt, then waits for the guest to power itself
/// off: it installs cua-spacesd, seals the image and shuts down
/// (`setup-spacesd.ps1`), so QEMU exiting is the end of the install.
async fn drive(plan: &InstallPlan) -> Result<()> {
    // Enter every 0.3 s for the first 25 s: the "Press any key to boot from
    // CD" prompt is up for a few seconds, and Setup's own window (whose Cancel
    // button an Enter would press) opens later.
    for _ in 0..83 {
        qmp(r#"{"execute":"send-key","arguments":{"keys":[{"type":"qcode","data":"ret"}]}}"#).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let deadline = std::time::Instant::now() + plan.timeout;
    loop {
        // QMP answers while the VM runs; once Windows has powered off QEMU
        // exits and nothing answers.
        if !qmp(r#"{"execute":"query-status"}"#).await {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err(ImageError::Build {
                step: "windows install".into(),
                detail: format!(
                    "Windows did not finish installing within {:?}",
                    plan.timeout
                ),
            });
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        present: bool,
        output: String,
        ran: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Wsl for Fake {
        fn available(&self) -> bool {
            self.present
        }
        async fn sh(&self, script: &str, _: Duration) -> Result<(i32, String)> {
            self.ran.lock().unwrap().push(script.to_string());
            Ok((0, self.output.clone()))
        }
    }

    #[tokio::test]
    async fn no_wsl_means_nothing_is_ready_and_the_guide_says_how() {
        let s = probe(&Fake::default()).await;
        assert_eq!(s, WslStatus::default());
        let steps = setup_steps(&s);
        assert!(matches!(&steps[0], Setup::Guide { how, .. } if how.contains("wsl --install")));
        assert!(not_ready_message(&s).contains("wsl --install"));
    }

    #[tokio::test]
    async fn a_ready_pc_has_nothing_to_set_up() {
        let f = Fake {
            present: true,
            output: "wsl\nkvm\nqemu\nqemu-img\novmf\n".into(),
            ..Default::default()
        };
        let s = probe(&f).await;
        assert!(s.ready());
        assert!(setup_steps(&s).is_empty());
    }

    #[tokio::test]
    async fn missing_qemu_is_installed_by_cua_and_missing_kvm_is_not_faked() {
        let f = Fake {
            present: true,
            output: "wsl\nkvm\n".into(),
            ..Default::default()
        };
        let steps = setup_steps(&probe(&f).await);
        assert_eq!(steps.len(), 1);
        assert!(
            matches!(&steps[0], Setup::Auto { script, .. } if script.contains("qemu-system-x86"))
        );

        let nokvm = WslStatus {
            wsl: true,
            qemu: true,
            qemu_img: true,
            ovmf: true,
            ..Default::default()
        };
        let steps = setup_steps(&nokvm);
        assert!(
            matches!(&steps[0], Setup::Guide { how, .. } if how.contains("software emulation"))
        );
    }

    #[test]
    fn windows_paths_become_mnt_paths() {
        assert_eq!(
            to_wsl_path(Path::new(r"C:\Users\me\x.iso")),
            "/mnt/c/Users/me/x.iso"
        );
        assert_eq!(to_wsl_path(Path::new(r"\\?\D:\a\b")), "/mnt/d/a/b");
        assert_eq!(to_wsl_path(Path::new("/already/linux")), "/already/linux");
    }

    #[test]
    fn the_qemu_command_is_kvm_with_the_install_media_first_and_ext4_files() {
        let plan = InstallPlan {
            name: "n".into(),
            arch: WindowsArch::Amd64,
            windows_iso: "x".into(),
            data_iso: "y".into(),
            virtio_iso: "z".into(),
            disk_gb: 64,
            cpus: 4,
            memory_mb: 6144,
            work_dir: "w".into(),
            timeout: Duration::from_secs(1),
        };
        let c = qemu_command(&plan, "$HOME/cua-build/n");
        assert!(c.contains("-enable-kvm -cpu host"));
        assert!(!c.contains("tcg") && !c.contains("whpx"));
        assert!(c.contains("ide-cd,drive=cd0,bus=ahci.1,bootindex=0"), "{c}");
        assert!(c.contains("file=$HOME/cua-build/n/windows.iso"));
        assert!(!c.contains("/mnt/"), "{c}");
        assert!(c.contains("hostfwd=tcp:0.0.0.0:18000-:3211"));
        assert!(c.contains("-m 6144 -smp 4"));
    }

    #[tokio::test]
    async fn arm64_on_a_windows_pc_is_refused() {
        let plan = InstallPlan {
            name: "n".into(),
            arch: WindowsArch::Arm64,
            windows_iso: "x".into(),
            data_iso: "y".into(),
            virtio_iso: "z".into(),
            disk_gb: 64,
            cpus: 4,
            memory_mb: 6144,
            work_dir: "w".into(),
            timeout: Duration::from_secs(1),
        };
        let i = WslInstaller {
            wsl: std::sync::Arc::new(Fake::default()),
        };
        let e = i.install(&plan).await.unwrap_err().to_string();
        assert!(e.contains("software emulation"), "{e}");
    }

    #[tokio::test]
    async fn an_unready_pc_fails_with_the_steps_and_runs_nothing_heavy() {
        let plan = InstallPlan {
            name: "n".into(),
            arch: WindowsArch::Amd64,
            windows_iso: "x".into(),
            data_iso: "y".into(),
            virtio_iso: "z".into(),
            disk_gb: 64,
            cpus: 4,
            memory_mb: 6144,
            work_dir: "w".into(),
            timeout: Duration::from_secs(1),
        };
        let f = std::sync::Arc::new(Fake {
            present: true,
            output: "wsl\n".into(),
            ..Default::default()
        });
        let i = WslInstaller { wsl: f.clone() };
        let e = i.install(&plan).await.unwrap_err().to_string();
        assert!(e.contains("/dev/kvm"), "{e}");
        // Only probes ran: no staging, no qemu.
        assert!(
            f.ran
                .lock()
                .unwrap()
                .iter()
                .all(|s| !s.contains("qemu-system-x86_64 -enable-kvm"))
        );
    }
}
