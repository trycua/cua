# Windows images in Cua Spaces

`windows` has always been a published image (`ghcr.io/trycua/windows:2022`, Windows
Server 2022, pulled). This adds the other Windows versions as **images built on
your computer**: pick one, and the first Space of it installs Windows unattended
from an ISO, seals the result as a base disk, and every later Space is a fast clone.

## Aliases

Windows aliases follow the convention of the other OS aliases: `<os>[:<version>]`
resolves to `ghcr.io/trycua/<os>:<tag>` (`macos:tahoe` is `macos:26`; `linux` is
`linux:24.04`). Word forms normalise to the tag the way `tahoe` and `sequoia` do.

| Alias | Tag | Name | Built from | Archs | Evaluation |
|---|---|---|---|---|---|
| `windows`, `windows:2022` | `2022` | Windows Server 2022 | **pulled** (published containerDisk) | amd64 | 180 days |
| `windows:11` (`win11`, `11-enterprise-eval`) | `11` | Windows 11 Enterprise Evaluation | Microsoft evaluation ISO, or yours | amd64; arm64 with your ISO | 90 days |
| `windows:2025` (`server-2025`) | `2025` | Windows Server 2025 Evaluation | evaluation ISO, or yours | amd64 | 180 days |
| `windows:2019` (`server-2019`) | `2019` | Windows Server 2019 Evaluation | evaluation ISO, or yours | amd64 | 180 days |
| `windows:10` (`win10`) | `10` | Windows 10 Enterprise Evaluation | evaluation ISO, or yours | amd64 | 90 days |

The table is `cua_image::windows::table` (code) and the picker entries are in
`libs/images/sandbox-images.json` (`local_build: "windows-iso"`; they resolve while
`published` is false, like a `derived_from` image). Which editions ship as aliases is
a default; adding one is a table row plus a catalog row.

Microsoft's evaluation centre publishes **no ARM64 Windows 11 ISO**, so on an Apple
silicon Mac `windows:11` needs your own ARM64 ISO
(<https://www.microsoft.com/software-download/windows11arm64>): `--iso`.

## ISO resolution

1. `--iso <path>` (remembered per version, so the daemon that builds finds it again),
   or `CUA_WINDOWS_ISO`. Checked: exists, over 1 GiB, ISO 9660 signature, and a name
   that says `arm64` is refused for an x64 build and the reverse. `--iso-sha256 <hex>`
   pins the digest.
2. The cached ISO `$CUA_HOME/images/windows/iso/windows-<tag>-<arch>.iso`, held to the
   digest recorded when it was downloaded.
3. Microsoft's evaluation media: find the `go.microsoft.com/fwlink` for the version's
   Evaluation Center page (falling back to a known link id), follow it, and download with
   cua's resumable fetcher (`.partial` file, HTTP Range, retries). Microsoft publishes no
   checksum for these, so the first download is trusted and its SHA-256 recorded; a cached
   ISO that no longer matches is downloaded again.

If nothing works the error says to download the ISO yourself and pass `--iso`.

## The build

`windows::build::ensure_base` (run by the disk resolver the first time a `windows:<v>`
Space is created; `cached_base` makes later creates instant):

1. Disk guard: refuse before downloading anything when the volume holding
   `$CUA_HOME/images/windows` cannot take about 40 GiB while keeping `CUA_DISK_MIN_FREE`.
2. A build lock (`build.lock`), so two creates do not install Windows twice; a lock older
   than 4 hours is a crashed build's.
3. Resolve the ISO and the virtio-win ISO (storage and network drivers).
4. Generate `Autounattend.xml` and `setup-spacesd.ps1` and write them on a small
   ISO 9660 + Joliet data disc (in Rust, no `genisoimage`); Setup reads
   `Autounattend.xml` from any drive, so the Windows ISO stays untouched.
5. Install in a VM until cua-spacesd answers on port 3211, power it off with ACPI, keep the
   disk as `base.qcow2` plus `base.json` (ISO digest, account, spacesd version). A disk
   under 4 GiB is an unfinished install and is refused.

The answer file: GPT (EFI, MSR, NTFS), edition by name (Server) or index 1, TPM/Secure
Boot/RAM/CPU checks bypassed (`LabConfig`), virtio storage and network drivers in WinPE and
in the installed system, a local administrator without a password signed in automatically
(`CUA_WINDOWS_USER`, default `User`), every OOBE page skipped, UAC off, RDP on, sleep and
Windows Update off. `setup-spacesd.ps1` runs at first logon: it installs cua-spacesd as a
logon task, opens its port **and gives the program a firewall rule** (without it Windows
prompts and blocks the daemon), and installs a **disk guard** task (below 3 GB free it trims
logs and writes `C:\ProgramData\cua\disk-low.txt`). On your own consumer ISO, `CUA_WINDOWS_EDITION`
selects an edition by name (`Windows 11 Pro` also gets the generic Pro key).

## Runtimes and hosts

`--kind` and `--runtime` keep their meaning: a Windows image is a VM, runtime `qemu`
(`placement::select`). What differs is who installs it:

| Host | Build runs in | Accelerator | Runtime management |
|---|---|---|---|
| Linux x64 | cua-vmm QEMU | KVM | `cua runtime setup qemu` (apt, user-local) |
| macOS Apple silicon | cua-vmm QEMU | HVF (ARM64 guest only) | `brew install qemu` (existing) |
| Windows PC x64 | **QEMU with KVM inside WSL2** | KVM | probe `wsl`, `/dev/kvm`, QEMU, OVMF; cua installs QEMU/OVMF/swtpm in the distribution; guides WSL / virtualization (they need an administrator or a reboot); `winget install SoftwareFreedomConservancy.QEMU` for native QEMU |

**Never software emulation.** A foreign-architecture guest (x64 Windows on an ARM Mac,
ARM64 Windows on a Windows PC) is refused with the reason, and cua-vmm refuses a Windows guest
that would run under TCG unless the accelerator was set explicitly. Why WSL2: QEMU for
Windows under WHPX hangs in OVMF's flash on current builds, and TCG takes hours. Dogfooding
(`fix/winlike-windows-builder-whpx`) found the rest, all built in: stage files on the WSL ext4
disk, not `/mnt` (9p stalls); boot the install media first (a blank disk first leaves OVMF in the
EFI shell); press Enter every half second from power-on for a minute to catch "Press any key to
boot from CD".

Known gap: building on a Windows PC works through WSL2; **running** a UEFI Windows guest from
the Rust QEMU runtime on a Windows PC still uses WHPX, which hangs in OVMF on current QEMU. Until
that is fixed upstream or the runtime runs in WSL2 too, use the Python SDK's `QEMUWSL2Runtime`
or Hyper-V there.

## Trying it

```sh
cua sb create windows:11                      # evaluation ISO downloaded, built once, cached
cua sb create windows:11 --iso ~/Win11_ARM64.iso   # your ISO (needed on a Mac)
cua sb create windows:2025 --name srv
cua sb create windows:11 --kind vm --runtime qemu --cpu 4 --memory 8GB
```

In the New Space dialog (the catalog entries are offered in the image picker; auto-download is the default) pick Windows 11 Enterprise Evaluation (or Server 2025/2022): the
progress shows "Finding the ISO", "Downloading", "Installing Windows (about 20 to 40 minutes,
once)", "Sealing the image".

UI gap: the dialog has no ISO-path field yet. That needs `isoPath` on the wizard's `CreatePlan` and
the UniFFI/TS generated bindings regenerated; until then register an ISO once with
`cua sb create windows:11 --iso <path>` (or set `CUA_WINDOWS_ISO`), and the dialog uses it.

## Guard rails from dogfooding

* Disk: the build refuses on a full volume before downloading; in the guest the disk guard task
  trims logs and flags low space.
* `cua sb exec NAME bash -c 'echo "a b"'` now keeps words with spaces, quotes and backslashes
  whole (it used to join arguments with spaces and lose the quoting).
* Long scripts: do not pass them on the command line (`-EncodedCommand` over a few KB fails with
  "The command line is too long"). Copy the script, then run it:
  copy it with `cua sb cp` to the guest (for a Windows daemon: `direct:<addr>:C:/script.ps1`), then run `powershell -File C:/script.ps1`.
* `sb exec` has a 120 s default limit: `--timeout 0` for long jobs (or detach them).
* `sb exec <local space>` runs on the Linux side of a Space that has one, while
  `sb exec direct:<addr>` runs on the Windows daemon: pick the target deliberately.

## Not verified here

No real Windows ISO was downloaded or installed in this change's tests; the install itself,
the Mac (HVF, ARM64) path and the WSL2 execution are covered by unit tests of everything
around them (command lines, answer files, resolution, caching, refusals), not by a full run.
