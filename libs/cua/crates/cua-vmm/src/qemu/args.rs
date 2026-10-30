//! Pure QEMU command-line construction (unit-tested without launching QEMU).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{Arch, GuestOs};

/// The piece size writes to a UEFI variable store are split into, so that
/// all-zero pieces are recorded as zero clusters (a qcow2 cluster).
const VARS_PIECE: u32 = 64 * 1024;

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

/// Where guest RAM lives. Every launch names its RAM `mem0`, so the state of
/// any instance loads into any other of the same shape (a migration stream
/// names RAM blocks by their backend id).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Memory {
    /// Anonymous host memory (`memory-backend-ram`).
    #[default]
    Ram,
    /// A file mapped as guest RAM (`memory-backend-file`). `shared`: guest
    /// writes reach the file (a golden snapshot being recorded); otherwise
    /// the file is mapped copy-on-write, so every VM restored from it shares
    /// its unmodified pages through the host page cache.
    File { path: PathBuf, shared: bool },
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
    /// Optional swtpm control socket.
    pub tpm_socket: Option<PathBuf>,
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
    /// Guest RAM backing.
    pub memory: Memory,
    /// Start paused, waiting for `migrate-incoming` (`-incoming defer`).
    pub incoming: bool,
    /// virgl GPU acceleration on this DRM render node
    /// (`virtio-gpu-gl-pci`, `-display egl-headless`; VNC still shows it).
    pub virgl_render_node: Option<PathBuf>,
    /// A virtio-balloon device with free page reporting: the guest reports
    /// the memory it freed and QEMU discards it, so the host gets it back.
    /// Only over anonymous RAM ([`Memory::Ram`]): QEMU discards file-backed
    /// RAM by punching holes in the file, which over a shared snapshot file
    /// would erase pages every VM restored from it still reads.
    pub free_page_reporting: bool,
    /// No display or input hardware (a guest without a desktop): no
    /// framebuffer, USB keyboard or tablet, and no VNC. Saves the host the
    /// framebuffer's memory and the display's timers on every VM.
    pub headless: bool,
    /// The bulk-transfer disk (see [`crate::bulk`]): a raw file attached as
    /// a virtio-blk device the guest's cua-spacesd finds by its serial.
    pub bulk: Option<PathBuf>,
    pub extra: Vec<String>,
}

impl LaunchConfig {
    /// Addresses every file QEMU opens by path and may re-open later (a
    /// block node's file, the firmware images, the file-backed RAM) through
    /// `alias` instead of through `dir`, wherever the path lies under `dir`.
    ///
    /// One rule for all of them, so a new `file=` argument cannot be missed:
    /// a migration to a file ends by inactivating the block layer, and
    /// file-posix then re-opens each node by the path it was started with.
    /// A warm-pool member's directory is renamed when a Space claims it, so
    /// every path under it must go through a link that follows the rename.
    /// (The log and pidfile are written, never re-opened, and keep their
    /// paths.)
    pub fn address_through(&mut self, dir: &Path, alias: &Path) {
        let via = |p: &mut PathBuf| {
            if let Ok(rest) = p.strip_prefix(dir) {
                *p = alias.join(rest);
            }
        };
        via(&mut self.disk);
        for p in [&mut self.seed_iso, &mut self.install_iso, &mut self.bulk]
            .into_iter()
            .flatten()
        {
            via(p);
        }
        match &mut self.firmware {
            Firmware::Bios => {}
            Firmware::UefiPflash { code, vars, .. } => {
                via(code);
                via(vars);
            }
            Firmware::UefiBios { code } => via(code),
        }
        if let Memory::File { path, .. } = &mut self.memory {
            via(path);
        }
    }
}

/// Build the argv (excluding the binary itself).
pub fn build(cfg: &LaunchConfig) -> Vec<String> {
    let windows_arm = cfg.os == GuestOs::Windows && cfg.arch == Arch::Aarch64;
    let mut a: Vec<String> = Vec::new();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));

    push(&["-name", &cfg.name]);

    // Machine + CPU.
    let hw = cfg.accel != "tcg";
    match cfg.arch {
        Arch::Aarch64 => {
            push(&["-machine", "virt,memory-backend=mem0"]);
            push(&["-cpu", if hw { "host" } else { "max" }]);
        }
        Arch::X86_64 => {
            push(&["-machine", "q35,smm=off,memory-backend=mem0"]);
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
    let size = format!("size={}M", cfg.memory_mb);
    match &cfg.memory {
        Memory::Ram => push(&["-object", &format!("memory-backend-ram,id=mem0,{size}")]),
        Memory::File { path, shared } => push(&[
            "-object",
            &format!(
                "memory-backend-file,id=mem0,{size},mem-path={},share={},prealloc=off",
                path.display(),
                if *shared { "on" } else { "off" }
            ),
        ]),
    }

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
            // After an incoming migration (a restore, a fork) QEMU's pflash
            // writes the whole flash back to this drive in one request. The
            // aarch64 store is 64 MiB, nearly all zeros, so every restored
            // VM would allocate all 64 MiB on disk (its checkpoint's copy
            // holds about 1 MiB). `detect-zeroes` only sees whole requests,
            // so a pass-through `blkdebug` node (no rules) splits writes
            // into 64 KiB pieces, one qcow2 cluster each, and the format
            // node records the all-zero pieces as zero clusters.
            push(&[
                "-drive",
                &format!(
                    "if=pflash,unit=1,driver=blkdebug,max-transfer={VARS_PIECE},\
                     image.driver={vars_format},image.detect-zeroes=on,\
                     image.file.filename={}",
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
    // Windows boots from AHCI, as its image is built and validated: its
    // virtio storage driver is not a boot driver there, so over virtio-blk
    // it cannot find its system volume and lands in WinRE. Everything else
    // boots virtio.
    if windows_arm {
        push(&["-device", "nvme,drive=disk0,serial=cua-disk0,bootindex=1"]);
        push(&["-device", "qemu-xhci,id=xhci"]);
    } else if cfg.os == GuestOs::Windows {
        push(&[
            "-device",
            "ahci,id=ahci",
            "-device",
            "ide-hd,drive=disk0,bus=ahci.0,bootindex=1",
        ]);
    } else {
        push(&["-device", "virtio-blk-pci,drive=disk0,bootindex=1"]);
    }
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
                if windows_arm {
                    push(&["-device", "usb-storage,drive=cd0,bus=xhci.0,bootindex=0"]);
                } else {
                    push(&["-device", "virtio-scsi-pci,id=scsi0"]);
                    push(&["-device", "scsi-cd,drive=cd0,bootindex=0"]);
                }
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
    if cfg.free_page_reporting {
        push(&[
            "-device",
            "virtio-balloon-pci,id=balloon0,free-page-reporting=on",
        ]);
    }
    if cfg.os == GuestOs::Linux {
        // Host entropy for the guest's RNG from the first instruction on
        // (no firmware RNG on these machines), and a fresh source after a
        // restore.
        push(&["-device", "virtio-rng-pci"]);
    }
    if let Some(bulk) = &cfg.bulk {
        // Host page cache on both sides of the device (`writeback`), so a
        // write by one side is read by the other as soon as it completes.
        push(&[
            "-drive",
            &format!(
                "file={},if=none,id=cuabulk,format=raw,cache=writeback,aio=threads",
                bulk.display()
            ),
        ]);
        push(&[
            "-device",
            &format!(
                "virtio-blk-pci,drive=cuabulk,serial={}",
                crate::bulk::SERIAL
            ),
        ]);
    }

    // Display + input: a framebuffer for VNC/screendump and an absolute
    // pointer so QMP input-send-event `abs` coordinates work.
    if cfg.headless {
        if cfg.arch == Arch::X86_64 {
            push(&["-vga", "none"]);
        }
        push(&["-display", "none", "-vnc", "none"]);
    } else {
        display(&mut a, cfg);
    }

    // The TPM is independent of the display: a headless restart keeps it.
    if let Some(socket) = &cfg.tpm_socket {
        a.extend([
            "-chardev".to_string(),
            format!("socket,id=chrtpm,path={}", socket.display()),
            "-tpmdev".into(),
            "emulator,id=tpm0,chardev=chrtpm".into(),
            "-device".into(),
            match cfg.arch {
                Arch::Aarch64 => "tpm-tis-device,tpmdev=tpm0",
                Arch::X86_64 => "tpm-tis,tpmdev=tpm0",
            }
            .into(),
        ]);
    }

    // Management.
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
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
    if cfg.incoming {
        push(&["-incoming", "defer"]);
    }
    if cfg.daemonize {
        push(&["-daemonize"]);
    }
    a.extend(cfg.extra.iter().cloned());
    a
}

/// The display and input devices, VNC and the display backend.
fn display(a: &mut Vec<String>, cfg: &LaunchConfig) {
    let windows_arm = cfg.os == GuestOs::Windows && cfg.arch == Arch::Aarch64;
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match (&cfg.virgl_render_node, cfg.arch) {
        _ if windows_arm => push(&["-device", "ramfb"]),
        // virgl: the guest's 3D goes to the host GPU; the headless EGL
        // display still feeds VNC and screendump.
        (Some(_), _) => push(&["-device", "virtio-gpu-gl-pci"]),
        (None, Arch::Aarch64) => push(&["-device", "virtio-gpu-pci"]),
        (None, Arch::X86_64) => push(&["-vga", "std"]),
    }
    if !windows_arm {
        push(&["-device", "qemu-xhci,id=xhci"]);
    }
    push(&["-device", "usb-kbd", "-device", "usb-tablet"]);
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
            tpm_socket: None,
            firmware: Firmware::Bios,
            forwards: vec![(40022, 22), (40080, 8080)],
            restrict_network: false,
            vnc_display: Some(3),
            qmp_port: 4444,
            serial_log: Some(PathBuf::from("/s/vm1/serial.log")),
            pidfile: Some(PathBuf::from("/s/vm1/qemu.pid")),
            daemonize: true,
            loadvm: None,
            memory: Memory::Ram,
            incoming: false,
            virgl_render_node: None,
            free_page_reporting: false,
            headless: false,
            bulk: None,
            extra: vec![],
        }
    }

    #[test]
    fn headless_has_no_display_input_or_vnc() {
        for arch in [Arch::Aarch64, Arch::X86_64] {
            let mut c = cfg(arch, "hvf");
            c.headless = true;
            let a = build(&c);
            assert!(has(&a, ["-display", "none"]), "{a:?}");
            assert!(has(&a, ["-vnc", "none"]), "{a:?}");
            for dev in [
                "virtio-gpu-pci",
                "usb-tablet",
                "usb-kbd",
                "qemu-xhci,id=xhci",
            ] {
                assert!(!a.iter().any(|x| x == dev), "{dev} in {a:?}");
            }
            assert_eq!(has(&a, ["-vga", "none"]), arch == Arch::X86_64);
            // Management stays.
            assert!(has(&a, ["-qmp", "tcp:127.0.0.1:4444,server=on,wait=off"]));
            assert!(has(&a, ["-serial", "file:/s/vm1/serial.log"]));
        }
    }

    #[test]
    fn free_page_reporting_adds_a_balloon_only_when_asked() {
        let mut c = cfg(Arch::Aarch64, "hvf");
        assert!(!build(&c).iter().any(|x| x.contains("balloon")));
        c.free_page_reporting = true;
        assert!(has(
            &build(&c),
            [
                "-device",
                "virtio-balloon-pci,id=balloon0,free-page-reporting=on"
            ]
        ));
    }

    #[test]
    fn the_bulk_disk_is_a_virtio_blk_with_a_serial_and_the_host_page_cache() {
        let mut c = cfg(Arch::Aarch64, "hvf");
        assert!(!build(&c).iter().any(|a| a.contains("cuabulk")));
        c.bulk = Some(PathBuf::from("/s/vm1/bulk.img"));
        let a = build(&c);
        assert!(has(
            &a,
            [
                "-drive",
                "file=/s/vm1/bulk.img,if=none,id=cuabulk,format=raw,cache=writeback,aio=threads"
            ]
        ));
        assert!(has(
            &a,
            ["-device", "virtio-blk-pci,drive=cuabulk,serial=cuabulk"]
        ));
    }

    #[test]
    fn ram_is_always_named_mem0_and_file_memory_maps_private_or_shared() {
        let a = build(&cfg(Arch::Aarch64, "hvf"));
        assert!(has(&a, ["-machine", "virt,memory-backend=mem0"]));
        assert!(has(
            &a,
            ["-object", "memory-backend-ram,id=mem0,size=2048M"]
        ));
        assert!(!a.contains(&"-incoming".to_string()));
        assert!(has(&a, ["-device", "virtio-rng-pci"]));
        let mut c = cfg(Arch::X86_64, "kvm");
        c.memory = Memory::File {
            path: "/g/mem.ram".into(),
            shared: false,
        };
        c.incoming = true;
        let a = build(&c);
        assert!(has(&a, ["-machine", "q35,smm=off,memory-backend=mem0"]));
        assert!(has(
            &a,
            [
                "-object",
                "memory-backend-file,id=mem0,size=2048M,mem-path=/g/mem.ram,share=off,prealloc=off"
            ]
        ));
        assert!(has(&a, ["-incoming", "defer"]));
        c.memory = Memory::File {
            path: "/g/mem.ram".into(),
            shared: true,
        };
        assert!(build(&c).iter().any(|x| x.contains("share=on")));
        c.os = GuestOs::Windows;
        assert!(!has(&build(&c), ["-device", "virtio-rng-pci"]));
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

    #[test]
    fn windows_arm_devices_and_tpm() {
        let mut c = cfg(Arch::Aarch64, "hvf");
        c.os = GuestOs::Windows;
        c.seed_iso = None;
        c.install_iso = Some("/install.iso".into());
        let a = build(&c);
        assert_eq!(
            a,
            serde_json::from_str::<Vec<String>>(include_str!("../../tests/argv/windows-arm.json"))
                .unwrap()
        );
        for device in [
            "ramfb",
            "nvme,drive=disk0,serial=cua-disk0,bootindex=1",
            "virtio-net-pci,netdev=net0",
            "usb-storage,drive=cd0,bus=xhci.0,bootindex=0",
        ] {
            assert!(has(&a, ["-device", device]));
        }
        let controller = a.iter().position(|s| s == "qemu-xhci,id=xhci").unwrap();
        for (i, arg) in a.iter().enumerate() {
            if arg.contains("bus=xhci.0") {
                assert!(controller < i);
            }
            assert!(
                !arg.contains("virtio-gpu")
                    && !arg.contains("virtio-scsi")
                    && !arg.contains("ahci")
            );
            assert!(!arg.contains("tpm") && arg != "-chardev");
        }
        c.seed_iso = Some("/seed.iso".into());
        let seeded = build(&c);
        assert!(has(&seeded, ["-device", "virtio-blk-pci,drive=cidata"]));
        assert_eq!(
            seeded.iter().filter(|s| *s == "qemu-xhci,id=xhci").count(),
            1
        );
        for arch in [Arch::Aarch64, Arch::X86_64] {
            c.arch = arch;
            c.tpm_socket = Some("/vm/tpm/swtpm.sock".into());
            let a = build(&c);
            assert!(has(
                &a,
                ["-chardev", "socket,id=chrtpm,path=/vm/tpm/swtpm.sock"]
            ));
            assert!(has(&a, ["-tpmdev", "emulator,id=tpm0,chardev=chrtpm"]));
            let mut h = c.clone();
            h.headless = true;
            assert!(has(
                &build(&h),
                ["-tpmdev", "emulator,id=tpm0,chardev=chrtpm"]
            ));
            assert!(has(
                &a,
                [
                    "-device",
                    if arch == Arch::Aarch64 {
                        "tpm-tis-device,tpmdev=tpm0"
                    } else {
                        "tpm-tis,tpmdev=tpm0"
                    }
                ]
            ));
        }
    }

    #[test]
    fn unchanged_vectors() {
        for (name, arch, os, iso, virgl) in [
            ("windows-x86", Arch::X86_64, GuestOs::Windows, true, false),
            ("linux-arm", Arch::Aarch64, GuestOs::Linux, true, false),
            ("linux-x86", Arch::X86_64, GuestOs::Linux, false, false),
            ("virgl-arm", Arch::Aarch64, GuestOs::Linux, false, true),
            ("virgl-x86", Arch::X86_64, GuestOs::Linux, false, true),
        ] {
            let mut c = cfg(arch, "kvm");
            c.os = os;
            c.install_iso = iso.then(|| "/install.iso".into());
            c.virgl_render_node = virgl.then(|| "/dev/dri/renderD128".into());
            let actual = serde_json::to_string_pretty(&build(&c)).unwrap();
            let path =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/argv/{name}.json"));
            assert_eq!(actual, std::fs::read_to_string(path).unwrap(), "{name}");
        }
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
        assert!(has(&a, ["-machine", "virt,memory-backend=mem0"]));
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
                "if=pflash,unit=1,driver=blkdebug,max-transfer=65536,image.driver=qcow2,\
                 image.detect-zeroes=on,image.file.filename=/s/vm1/vars.qcow2"
            ]
        ));
        assert!(has(&a, ["-device", "virtio-gpu-pci"]));
        assert!(a.contains(&"-daemonize".to_string()));
    }

    #[test]
    fn the_writable_varstore_records_zero_pieces_and_the_code_stays_read_only() {
        // Every accelerator and arch that boots UEFI from a pflash pair:
        // macOS (hvf), Linux (kvm), Windows hosts (whpx) and tcg.
        for (arch, accel) in [
            (Arch::Aarch64, "hvf"),
            (Arch::Aarch64, "kvm"),
            (Arch::X86_64, "kvm"),
            (Arch::X86_64, "whpx"),
            (Arch::X86_64, "tcg"),
        ] {
            let mut c = cfg(arch, accel);
            c.firmware = Firmware::UefiPflash {
                code: "/fw/code.fd".into(),
                vars: "/s/vm1/efivars.qcow2".into(),
                vars_format: "qcow2".into(),
            };
            let a = build(&c);
            let drives: Vec<&String> = a
                .windows(2)
                .filter(|w| w[0] == "-drive" && w[1].starts_with("if=pflash"))
                .map(|w| &w[1])
                .collect();
            assert_eq!(
                drives,
                [
                    "if=pflash,format=raw,unit=0,readonly=on,file=/fw/code.fd",
                    "if=pflash,unit=1,driver=blkdebug,max-transfer=65536,image.driver=qcow2,\
                     image.detect-zeroes=on,image.file.filename=/s/vm1/efivars.qcow2",
                ],
                "{arch:?} {accel}"
            );
        }
        // SeaBIOS and `-bios` UEFI have no varstore drive.
        let a = build(&cfg(Arch::X86_64, "kvm"));
        assert!(!a.iter().any(|s| s.starts_with("if=pflash")));
    }

    #[test]
    fn x86_on_tcg_uses_q35_max_cpu_and_mttcg() {
        let a = build(&cfg(Arch::X86_64, "tcg"));
        assert!(has(&a, ["-machine", "q35,smm=off,memory-backend=mem0"]));
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

    /// Every path QEMU may re-open goes through the alias, whichever argument
    /// carries it: the drift guard for a new `file=` argument. A claimed
    /// pool member's directory is renamed under its running QEMU, and a
    /// migration to a file re-opens each block node by its start-up path.
    #[test]
    fn address_through_rewrites_every_path_qemu_can_reopen() {
        let dir = PathBuf::from("/vms/cua-pool-a");
        let alias = PathBuf::from("/vms/_run/cua-pool-a");
        for arch in [Arch::Aarch64, Arch::X86_64] {
            let mut c = cfg(arch, "kvm");
            c.disk = dir.join("disk.qcow2");
            c.seed_iso = Some(dir.join("seed.iso"));
            c.install_iso = Some(dir.join("install.iso"));
            c.bulk = Some(dir.join("bulk.img"));
            c.firmware = Firmware::UefiPflash {
                code: PathBuf::from("/usr/share/AAVMF/code.fd"),
                vars: dir.join("efivars.qcow2"),
                vars_format: "qcow2".into(),
            };
            c.memory = Memory::File {
                path: dir.join("ram"),
                shared: true,
            };
            c.serial_log = Some(dir.join("serial.log"));
            c.pidfile = Some(dir.join("qemu.pid"));
            c.address_through(&dir, &alias);
            let a = build(&c);
            let joined = a.join("\n");
            for f in [
                "disk.qcow2",
                "seed.iso",
                "install.iso",
                "bulk.img",
                "efivars.qcow2",
                "ram",
            ] {
                assert!(
                    joined.contains(&format!("{}/{f}", alias.display())),
                    "{f} not through the alias: {a:?}"
                );
            }
            // Nothing that is re-opened names the directory: only the
            // serial log and the pidfile, which are written, not re-opened.
            for tok in &a {
                for part in tok.split([',', '=']) {
                    let part = part.strip_prefix("file:").unwrap_or(part);
                    let written = part.ends_with("serial.log") || part.ends_with("qemu.pid");
                    assert!(
                        written || !Path::new(part).starts_with(&dir),
                        "{part} still names the directory: {a:?}"
                    );
                }
            }
            // Paths outside the directory (shared firmware) are untouched.
            assert!(joined.contains("/usr/share/AAVMF/code.fd"), "{a:?}");
        }
        // The UefiBios variant and a path outside the directory.
        let mut c = cfg(Arch::X86_64, "kvm");
        c.firmware = Firmware::UefiBios {
            code: dir.join("bios.fd"),
        };
        c.disk = PathBuf::from("/images/base.qcow2");
        c.address_through(&dir, &alias);
        let a = build(&c);
        assert!(has(&a, ["-bios", "/vms/_run/cua-pool-a/bios.fd"]), "{a:?}");
        assert!(a.join(" ").contains("file=/images/base.qcow2,"), "{a:?}");
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

    /// The Windows image is built and validated (libs/images/windows-2022/
    /// build-image.sh, scripts/images/image-doctor-windows.sh) on AHCI: over
    /// virtio-blk it cannot find its system volume and lands in WinRE
    /// (#4777).
    #[test]
    fn windows_guest_boots_from_ahci_like_the_image_build() {
        let mut c = cfg(Arch::X86_64, "kvm");
        c.os = GuestOs::Windows;
        c.seed_iso = None;
        let a = build(&c);
        assert!(has(&a, ["-device", "ahci,id=ahci"]), "{a:?}");
        assert!(
            has(&a, ["-device", "ide-hd,drive=disk0,bus=ahci.0,bootindex=1"]),
            "{a:?}"
        );
        assert!(!a.iter().any(|x| x.starts_with("virtio-blk-pci")), "{a:?}");

        // Linux guests keep virtio.
        let a = build(&cfg(Arch::X86_64, "kvm"));
        assert!(has(
            &a,
            ["-device", "virtio-blk-pci,drive=disk0,bootindex=1"]
        ));
        assert!(!a.iter().any(|x| x.starts_with("ahci")));
    }

    #[test]
    fn disk_format_from_extension() {
        assert_eq!(disk_format_for("a/b.QCOW2".as_ref()), "qcow2");
        assert_eq!(disk_format_for("a/b.img".as_ref()), "raw");
        assert_eq!(disk_format_for("a/b.vhdx".as_ref()), "vhdx");
    }
}
