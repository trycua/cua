# RFC: a Cua-owned QEMU + WHPX runtime on Windows (spike findings)

Status: spike results, for approval. Nothing here is implemented.

Why: the Windows runtime of #6740 rides on WSL 2, whose management service can wedge
(`wsl -l -v` hangs, and the fix is `wsl --shutdown`, which stops every distribution the
user has). A Cua-owned QEMU with the Windows Hypervisor Platform (WHPX) would not touch
the WSL service at all, and runs unelevated once the Windows feature is on.

## What was measured

PC: Windows 11 Pro 26200, session not elevated, `WHvGetCapability(HypervisorPresent)` = 1,
QEMU 11.1.0 (`-accel help`: `tcg`, `whpx`; machines `q35`, `microvm`). Guest kernel: cua's
pinned Firecracker CI 6.1.155 (`vmlinux`, SHA-256 checked) with a 0.7 MB initramfs (busybox),
2 vCPUs, 256 MB. Harness: `QEMU -accel whpx -M q35 -cpu qemu64 -kernel ... -initrd ...`,
timed from process start.

| Measure | WHPX | TCG (one-off, tiny initramfs; software emulation is not used for real work) |
|---|---|---|
| Kernel + init to the first line of `/init` | 0.82 s (guest uptime 0.52 s) | 1.16 s |
| 3 000 000-iteration `awk` loop in the guest | 0.76 s | 15.9 s (21x slower) |
| Guest sees | `Hypervisor detected: Microsoft Hyper-V`, host build 10.0.26100 | none |
| Ubuntu 24.04.5 base rootfs (78 MB unpacked) as the initramfs, to userland | 1.84 s; glibc 2.39, `/usr/bin/sleep`, `/usr/sbin/chroot` present | not run |

Acceleration is on: the guest reports the Hyper-V hypervisor leaf, and the compute loop
is 21x faster than TCG.

Devices on `q35` under WHPX, all working in one run:

- `virtio-blk-pci`: the guest read the disk image.
- `virtio-net-pci` with user-mode networking: the guest reached a server on the host at
  `10.0.2.2` (egress), and `hostfwd=tcp:127.0.0.1:19000-:9000` reached a server in the guest.
- `virtio-serial-pci` + `virtserialport` on a `-chardev socket` (TCP to the host): the guest wrote
  `AGENT-READY` 1.57 s after QEMU started, the host sent a request and got the reply. This is
  the vsock substitute for the agent channel (a named-pipe chardev is the unelevated alternative).

Overhead of an idle guest: 256 MB guest: 148 MB working set, 365 MB commit; 1 GB guest: 163 MB
working set (the guest's RAM is committed but touched lazily), 1.2 % of one core. Note:
with `-serial stdio` and a NUL stdin QEMU's console reader spins one core (100 %); use `-serial file:` or a pipe/socket chardev.

What does not work, or was not tried:

- `-M microvm` hangs under WHPX in QEMU 11.1.0 (after `ACPI: Core revision`; with `acpi=off` it
  reaches `/init` and then the serial console stalls: interrupt delivery). `q35` is the machine to use.
- No 9p or virtiofs host shares on Windows QEMU (no `-virtfs`; virtiofsd is Linux only): folder
  mounts need a different mechanism (SMB, or a file channel over virtio-serial).
- Not yet tried: an ext4 root disk (no `mke2fs` on Windows: build it inside the guest from the rootfs
  tarball), `dockerd` in the guest, qcow2 snapshots (`savevm`) and memory ballooning.

## Design (proposal)

- **Provision.** A Cua-owned VM under `<cua home>\runtimes\linux\qemu\`: the pinned kernel, the
  pinned Ubuntu 24.04 base rootfs (the engine helper needs `/usr/bin/sleep`, `nft`, `ip`, `nsenter`),
  a sparse disk image, built by a first boot of the guest itself (`mkfs.ext4`, unpack, `apt-get install
  docker.io ...`). QEMU itself: the user's if it is >= 9 (`C:\Program Files\qemu`), else a pinned, checked
  download into the cua home.
- **Start on demand, idle stop.** Same lifecycle as the macOS runtime and #6740: `ensure()` boots
  the VM (about 2 s to the agent, plus Docker), the idle stop powers it off over the agent channel (`ACPI`
  powerdown, then kill), `remove` deletes the directory.
- **Engine.** `dockerd` in the guest on a unix socket; a small Windows-side bridge serves a per-home named
  pipe (as #6740 does) and forwards each connection over a `virtio-serial` port (or a hostfwd on loopback with
  a per-VM token). No TCP port without a token.
- **MicroVM kind.** The same guest runs Firecracker only with nested virtualization (WHPX exposes none by
  default), so microVM Spaces on Windows are QEMU guests on this runtime, not Firecracker.
- **WHP detection without elevation.** `WHvGetCapability(WHvCapabilityCodeHypervisorPresent)` from
  `WinHvPlatform.dll` returns present = 1 here, unelevated. A missing DLL or present = 0 means the feature is off.
- **UAC flow.** When it is off, `doctor` and New Space say so with one button: an elevated
  `Start-Process -Verb RunAs dism /online /enable-feature /featurename:HypervisorPlatform /all /norestart`,
  then a "restart needed" state until `WHvGetCapability` answers. Firmware virtualization off is reported
  as such (`HypervisorPresent` = 0 after the feature is on), with the BIOS/UEFI instruction.
- **Disk guard.** The existing `CUA_DISK_MIN_FREE` guard before creating the disk and before image pulls;
  the sparse disk's growth is reported like #6778.
- **Network policy.** The guest kernel (cua's 6.1) has nftables; whether it has the `bridge` family (the WSL 2
  kernel does not, see #6791) is a build option here, so the full egress policy of the Mac can be enforced.

## Risks

- Windows Hypervisor Platform shares the hypervisor with Hyper-V, WSL 2 and Docker Desktop: fine
  to run together (this PC runs a WSL 2 VM and the QEMU spike at once), but performance numbers under load were not taken.
- QEMU on Windows has no memory ballooning or hugepages; RAM is committed up front.
- No folder mounts yet (above).
- QEMU WHPX has known timer/interrupt gaps (the `microvm` hang): pin a QEMU version per cua release.

## Staged plan

1. Detection and the UAC flow (`doctor`, New Space, "restart needed"): small, no VM.
2. The guest image builder and `ensure()` for an engine-less VM: boot, agent channel, idle stop, remove.
3. `dockerd` in the guest, the pipe bridge, `runtime.linux = builtin` on Windows chooses it when WSL is not
   usable (or by setting).
4. Folder mounts, qcow2 snapshots (checkpoints), the microVM kind.
5. Benchmarks against the macOS rows once 2 and 3 land.
