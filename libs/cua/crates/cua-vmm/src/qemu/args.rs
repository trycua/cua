//! Pure QEMU command-line construction (unit-tested without launching QEMU).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::types::{Arch, GuestOs};

/// Firmware selection for one launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Firmware {
    /// SeaBIOS (x86 default).
    Bios,
    /// UEFI with a per-VM writable variable store (pflash pair). The varstore
    /// is qcow2 so `savevm` works (raw pflash cannot hold snapshots).
    UefiPflash {
        code: PathBuf,
        vars: PathBuf,
        vars_format: String,
    },
    /// UEFI via `-bios` (volatile variables).
    UefiBios { code: PathBuf },
}

/// Everything needed to build a `qemu-system-*` argv.
#[derive(Clone, Debug)]
pub struct LaunchConfig {
    pub binary: PathBuf,
    pub name: String,
    pub arch: Arch,
    pub os: GuestOs,
    /// `hvf`, `kvm`, `whpx` or `tcg`.
    pub accel: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk: PathBuf,
    pub disk_format: String,
    /// cloud-init NoCloud seed (read-only virtio disk).
    pub seed_iso: Option<PathBuf>,
    /// Installer ISO attached as CD-ROM and booted first.
    pub install_iso: Option<PathBuf>,
    pub firmware: Firmware,
    /// `(host_port, guest_port)` TCP forwards on 127.0.0.1.
    pub forwards: Vec<(u16, u16)>,
    pub restrict_network: bool,
    pub vnc_display: Option<u16>,
    pub qmp_port: u16,
    pub serial_log: Option<PathBuf>,
    pub pidfile: Option<PathBuf>,
    pub daemonize: bool,
    /// Restore this internal snapshot at boot (`-loadvm`).
    pub loadvm: Option<String>,
    /// virgl GPU acceleration on this DRM render node
    /// (`virtio-gpu-gl-pci`, `-display egl-headless`; VNC still shows it).
    pub virgl_render_node: Option<PathBuf>,
    pub extra: Vec<String>,
}

/// Build the argv (excluding the binary itself).
pub fn build(cfg: &LaunchConfig) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));

    push(&["-name", &cfg.name]);

    // Machine + CPU.
    let hw = cfg.accel != "tcg";
    match cfg.arch {
        Arch::Aarch64 => {
            push(&["-machine", "virt"]);
            push(&["-cpu", if hw { "host" } else { "max" }]);
        }
        Arch::X86_64 => {
            push(&["-machine", "q35,smm=off"]);
            match (hw, cfg.os) {
                // Hyper-V enlightenments, as KubeVirt's Windows templates set
                // (KVM only; hvf/whpx do not implement them).
                (true, GuestOs::Windows) if cfg.accel == "kvm" => push(&[
                    "-cpu",
                    "host,hv_relaxed,hv_vapic,hv_spinlocks=0x1fff,hv_time,hv_vpindex,hv_synic,hv_stimer",
                ]),
                (true, _) => push(&["-cpu", "host"]),
                (false, _) => push(&["-cpu", "max"]),
            }
        }
    }
    if cfg.accel == "tcg" {
        // Multi-threaded TCG is off by default for x86-on-arm (memory-model
        // mismatch); it is stable for Linux guests and several times faster.
        push(&["-accel", "tcg,thread=multi,tb-size=1024"]);
    } else {
        push(&["-accel", &cfg.accel]);
    }
    push(&[
        "-smp",
        &cfg.cpus.to_string(),
        "-m",
        &cfg.memory_mb.to_string(),
    ]);

    // Firmware.
    match &cfg.firmware {
        Firmware::Bios => {}
        Firmware::UefiPflash {
            code,
            vars,
            vars_format,
        } => {
            push(&[
                "-drive",
                &format!(
                    "if=pflash,format=raw,unit=0,readonly=on,file={}",
                    code.display()
                ),
            ]);
            push(&[
                "-drive",
                &format!(
                    "if=pflash,format={vars_format},unit=1,file={}",
                    vars.display()
                ),
            ]);
        }
        Firmware::UefiBios { code } => push(&["-bios", &code.display().to_string()]),
    }

    // Disks.
    push(&[
        "-drive",
        &format!(
            "file={},if=none,id=disk0,format={},cache=writeback,discard=unmap",
            cfg.disk.display(),
            cfg.disk_format
        ),
    ]);
    push(&["-device", "virtio-blk-pci,drive=disk0,bootindex=1"]);
    if let Some(seed) = &cfg.seed_iso {
        push(&[
            "-drive",
            &format!(
                "file={},if=none,id=cidata,format=raw,readonly=on",
                seed.display()
            ),
        ]);
        push(&["-device", "virtio-blk-pci,drive=cidata"]);
    }
    if let Some(iso) = &cfg.install_iso {
        match cfg.arch {
            Arch::X86_64 => push(&["-cdrom", &iso.display().to_string(), "-boot", "d"]),
            Arch::Aarch64 => {
                push(&[
                    "-drive",
                    &format!(
                        "file={},if=none,id=cd0,format=raw,readonly=on,media=cdrom",
                        iso.display()
                    ),
                ]);
                push(&[
                    "-device",
                    "virtio-scsi-pci,id=scsi0",
                    "-device",
                    "scsi-cd,drive=cd0,bootindex=0",
                ]);
            }
        }
    }

    // Network: user-mode NAT with loopback-only forwards.
    let mut net = String::from("user,id=net0");
    if cfg.restrict_network {
        net.push_str(",restrict=on");
    }
    for (host, guest) in &cfg.forwards {
        net.push_str(&format!(",hostfwd=tcp:127.0.0.1:{host}-:{guest}"));
    }
    push(&["-netdev", &net]);
    push(&["-device", "virtio-net-pci,netdev=net0"]);

    // Display + input: a framebuffer for VNC/screendump and an absolute
    // pointer so QMP input-send-event `abs` coordinates work.
    match (&cfg.virgl_render_node, cfg.arch) {
        // virgl: the guest's 3D goes to the host GPU; the headless EGL
        // display still feeds VNC and screendump.
        (Some(_), _) => push(&["-device", "virtio-gpu-gl-pci"]),
        (None, Arch::Aarch64) => push(&["-device", "virtio-gpu-pci"]),
        (None, Arch::X86_64) => push(&["-vga", "std"]),
    }
    push(&[
        "-device",
        "qemu-xhci,id=xhci",
        "-device",
        "usb-kbd",
        "-device",
        "usb-tablet",
    ]);
    match &cfg.virgl_render_node {
        Some(node) => push(&[
            "-display",
            &format!("egl-headless,rendernode={}", node.display()),
        ]),
        None => push(&["-display", "none"]),
    }
    match cfg.vnc_display {
        Some(d) => push(&["-vnc", &format!("127.0.0.1:{d}")]),
        None => push(&["-vnc", "none"]),
    }

    // Management.
    push(&[
        "-qmp",
        &format!("tcp:127.0.0.1:{},server=on,wait=off", cfg.qmp_port),
    ]);
    match &cfg.serial_log {
        Some(p) => push(&["-serial", &format!("file:{}", p.display())]),
        None => push(&["-serial", "none"]),
    }
    push(&["-monitor", "none"]);
    if let Some(p) = &cfg.pidfile {
        push(&["-pidfile", &p.display().to_string()]);
    }
    if let Some(s) = &cfg.loadvm {
        push(&["-loadvm", s]);
    }
    if cfg.daemonize {
        push(&["-daemonize"]);
    }
    a.extend(cfg.extra.iter().cloned());
    a
}

/// Disk format inferred from the file extension (for boot disks that are not
/// our own overlays).
pub fn disk_format_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("qcow2") => "qcow2",
        Some("vhdx") => "vhdx",
        Some("vmdk") => "vmdk",
        _ => "raw",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(arch: Arch, accel: &str) -> LaunchConfig {
        LaunchConfig {
            binary: PathBuf::from("/usr/bin/qemu"),
            name: "vm1".into(),
            arch,
            os: GuestOs::Linux,
            accel: accel.into(),
            cpus: 2,
            memory_mb: 2048,
            disk: PathBuf::from("/s/vm1/disk.qcow2"),
            disk_format: "qcow2".into(),
            seed_iso: Some(PathBuf::from("/s/vm1/seed.iso")),
            install_iso: None,
            firmware: Firmware::Bios,
            forwards: vec![(40022, 22), (40080, 8080)],
            restrict_network: false,
            vnc_display: Some(3),
            qmp_port: 4444,
            serial_log: Some(PathBuf::from("/s/vm1/serial.log")),
            pidfile: Some(PathBuf::from("/s/vm1/qemu.pid")),
            daemonize: true,
            loadvm: None,
            virgl_render_node: None,
            extra: vec![],
        }
    }

    #[test]
    fn virgl_swaps_the_display_device_and_keeps_vnc() {
        for arch in [Arch::Aarch64, Arch::X86_64] {
            let mut c = cfg(arch, "kvm");
            c.virgl_render_node = Some(PathBuf::from("/dev/dri/renderD128"));
            let a = build(&c);
            assert!(has(&a, ["-device", "virtio-gpu-gl-pci"]), "{a:?}");
            assert!(has(
                &a,
                ["-display", "egl-headless,rendernode=/dev/dri/renderD128"]
            ));
            assert!(has(&a, ["-vnc", "127.0.0.1:3"]), "VNC still shows it");
            assert!(!has(&a, ["-display", "none"]));
            assert!(!has(&a, ["-device", "virtio-gpu-pci"]));
            assert!(!has(&a, ["-vga", "std"]));
        }
        assert!(has(&build(&cfg(Arch::X86_64, "kvm")), ["-display", "none"]));
    }

    fn has(args: &[String], pair: [&str; 2]) -> bool {
        args.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1])
    }

    #[test]
    fn aarch64_hvf_uses_virt_host_cpu_and_pflash() {
        let mut c = cfg(Arch::Aarch64, "hvf");
        c.firmware = Firmware::UefiPflash {
            code: "/fw/code.fd".into(),
            vars: "/s/vm1/vars.qcow2".into(),
            vars_format: "qcow2".into(),
        };
        let a = build(&c);
        assert!(has(&a, ["-machine", "virt"]));
        assert!(has(&a, ["-cpu", "host"]));
        assert!(has(&a, ["-accel", "hvf"]));
        assert!(has(
            &a,
            [
                "-drive",
                "if=pflash,format=raw,unit=0,readonly=on,file=/fw/code.fd"
            ]
        ));
        assert!(has(
            &a,
            [
                "-drive",
                "if=pflash,format=qcow2,unit=1,file=/s/vm1/vars.qcow2"
            ]
        ));
        assert!(has(&a, ["-device", "virtio-gpu-pci"]));
        assert!(a.contains(&"-daemonize".to_string()));
    }

    #[test]
    fn x86_on_tcg_uses_q35_max_cpu_and_mttcg() {
        let a = build(&cfg(Arch::X86_64, "tcg"));
        assert!(has(&a, ["-machine", "q35,smm=off"]));
        assert!(has(&a, ["-cpu", "max"]));
        assert!(has(&a, ["-accel", "tcg,thread=multi,tb-size=1024"]));
        assert!(has(&a, ["-vga", "std"]));
    }

    #[test]
    fn forwards_are_loopback_only_and_restrict_is_opt_in() {
        let mut c = cfg(Arch::X86_64, "kvm");
        let a = build(&c);
        assert!(has(
            &a,
            [
                "-netdev",
                "user,id=net0,hostfwd=tcp:127.0.0.1:40022-:22,hostfwd=tcp:127.0.0.1:40080-:8080"
            ]
        ));
        assert!(
            !a.iter().any(|x| x.contains("restrict=")),
            "egress is on by default"
        );
        c.restrict_network = true;
        let a = build(&c);
        // restrict=on cuts guest egress; the SDK's loopback forwards stay.
        assert!(has(
            &a,
            [
                "-netdev",
                "user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:40022-:22,\
                 hostfwd=tcp:127.0.0.1:40080-:8080"
            ]
        ));
    }

    #[test]
    fn seed_is_readonly_virtio_and_management_sockets_present() {
        let a = build(&cfg(Arch::X86_64, "kvm"));
        assert!(has(
            &a,
            [
                "-drive",
                "file=/s/vm1/seed.iso,if=none,id=cidata,format=raw,readonly=on"
            ]
        ));
        assert!(has(&a, ["-qmp", "tcp:127.0.0.1:4444,server=on,wait=off"]));
        assert!(has(&a, ["-vnc", "127.0.0.1:3"]));
        assert!(has(&a, ["-serial", "file:/s/vm1/serial.log"]));
        assert!(has(&a, ["-pidfile", "/s/vm1/qemu.pid"]));
    }

    #[test]
    fn loadvm_and_install_iso() {
        let mut c = cfg(Arch::X86_64, "kvm");
        c.loadvm = Some("ck1".into());
        c.install_iso = Some("/iso/win.iso".into());
        let a = build(&c);
        assert!(has(&a, ["-loadvm", "ck1"]));
        assert!(has(&a, ["-cdrom", "/iso/win.iso"]));
        let mut c = cfg(Arch::Aarch64, "hvf");
        c.install_iso = Some("/iso/deb.iso".into());
        let a = build(&c);
        assert!(has(&a, ["-device", "scsi-cd,drive=cd0,bootindex=0"]));
    }

    #[test]
    fn disk_format_from_extension() {
        assert_eq!(disk_format_for("a/b.QCOW2".as_ref()), "qcow2");
        assert_eq!(disk_format_for("a/b.img".as_ref()), "raw");
        assert_eq!(disk_format_for("a/b.vhdx".as_ref()), "vhdx");
    }
}
