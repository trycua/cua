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
| `windows:11` (`win11`, `11-enterprise-eval`) | `11` | Windows 11 | amd64: Microsoft evaluation ISO; arm64: the ARM64 ISO Microsoft offers for download; or yours | amd64, arm64 | 90 days (amd64 evaluation); arm64 installs unactivated |
| `windows:2025` (`server-2025`) | `2025` | Windows Server 2025 Evaluation | evaluation ISO, or yours | amd64 | 180 days |
| `windows:2019` (`server-2019`) | `2019` | Windows Server 2019 Evaluation | evaluation ISO, or yours | amd64 | 180 days |
| `windows:10` (`win10`) | `10` | Windows 10 Enterprise Evaluation | evaluation ISO, or yours | amd64 | 90 days |

The table is `cua_image::windows::table` (code) and the picker entries are in
`libs/images/sandbox-images.json` (`local_build: "windows-iso"`; they resolve while
`published` is false, like a `derived_from` image). Which editions ship as aliases is
a default; adding one is a table row plus a catalog row.

Microsoft's evaluation centre publishes **no ARM64 Windows 11 ISO**. On an Apple silicon Mac
`windows:11` is ARM64 Windows (native speed, and it runs x64 apps) and its ISO comes from the
page Microsoft itself offers for it, <https://www.microsoft.com/software-download/windows11arm64>:
see "ISO resolution". You can still pass your own with `--iso`.

## ISO resolution

1. `--iso <path>` (remembered per version, so the daemon that builds finds it again),
   or `CUA_WINDOWS_ISO`. Checked: exists, over 1 GiB, ISO 9660 signature, and a name
   that says `arm64` is refused for an x64 build and the reverse. `--iso-sha256 <hex>`
   pins the digest.
2. The cached ISO `$CUA_HOME/images/windows/iso/windows-<tag>-<arch>.iso`, held to the
   digest recorded when it was downloaded.
3. **Microsoft's media, by architecture** (`WindowsVersion::media`):
   * *Evaluation Center* (Windows 11 and Server on x64): find the `go.microsoft.com/fwlink`
     for the version's page (falling back to a known link id), follow it, and download with cua's
     resumable fetcher (`.partial` file, HTTP Range, retries). Microsoft publishes no checksum
     for these, so the first download is trusted and its SHA-256 recorded; a cached ISO that no
     longer matches is downloaded again.
   * *Consumer download* (Windows 11 ARM64): Microsoft's software-download service, the one
     behind the "Download Windows 11" web pages and used by tools like Fido. cua reads the current
     edition id from the page (`<option value="3816">Windows 11 (multi-edition ISO for Arm64)`;
     ids change every release, so they are never hard-coded), registers a random session id,
     asks `getskuinformationbyproductedition` for the English release and
     `GetProductDownloadLinksBySku` for a link on `software.download.prss.microsoft.com`
     (valid 24 hours), and downloads it resumably. **The page publishes the SHA-256 of every
     language's ISO, and cua checks the download against it**: a truncated or altered file is
     deleted, never installed from. No account is needed. The ISO is downloaded from Microsoft
     to this computer for the person who asked for it; cua never bundles, mirrors or republishes it.

If nothing works the error says to download the ISO yourself and pass `--iso`.

An ISO of your own is installed as Windows 11 (or 10) Pro with Microsoft's generic key,
unless its file name says `eval` (evaluation media carries its own key); without a key Setup
would wait at "Product key". Server ISOs pick their edition by name.

A Windows or macOS computer needs the cua daemon to outlive the command: `cua sb create
windows:11` starts the daemon when none runs, and `--embedded` is refused in plain words.

### Licence

An ARM64 Windows 11 build installs **Windows 11 Pro from Microsoft's consumer media, unactivated**
(the answer file uses the public generic Pro key, which selects the edition and activates
nothing). Unactivated Windows 11 runs with a watermark and limited personalisation until a
licence is added; Microsoft offers no evaluation edition for ARM64. This is the consumer
route Microsoft documents for personal use and testing, not an evaluation: whether cua should ship
it as the default (versus asking for `--iso`) is a product decision, flagged in the pull request.

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
5. Install in a VM. At the first logon the guest runs `setup-spacesd.ps1`: it downloads the newest
   published cua-spacesd with a Windows build (the pinned one when GitHub cannot be read), installs it
   exactly as the published Windows image does (`C:\Program Files\Cua\spacesd`, a scheduled task in
   the desktop session, bootstrap mode on `0.0.0.0:3211`, the token directory with its ACL), proves it
   listens, then **seals** the image (no token, no machine key, no per-machine state, as
   `libs/images/windows-2022/seal.ps1`) and powers off. The host waits for the power-off, keeps
   the disk as `base.qcow2` plus `base.json` (ISO digest, account, spacesd version). A disk
   under 4 GiB is an unfinished install and is refused. The install VM is stopped and deleted if the
   create is cancelled; the ISO download resumes from its `.partial` file.
6. Every Space is a copy-on-write clone of `base.qcow2` that boots in about 30 seconds; the create
   returns when cua-spacesd answers (the Space token is installed by the first authenticated `Init`).

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

**`--runtime auto` and software emulation.** `auto` picks QEMU for a Windows guest and the accelerator
for the guest's architecture: HVF for ARM64 Windows on Apple silicon, KVM on Linux, WSL2/KVM on a
Windows PC. A guest of the *other* architecture (x64 Windows on an Apple silicon Mac, ARM64 Windows on
an x64 computer) could only run in software emulation (TCG). cua **refuses that by default, in plain
words, before anything is downloaded**: x64 Windows under TCG on an M-series Mac takes hours to install
and is too slow for an agent's screenshot and click loop (its readiness and exec time-outs expire first).
The message names the way out: on a Mac use `windows:11` (ARM64, which runs x64 apps), and
`CUA_ALLOW_SOFTWARE_EMULATION=1` accepts the slowness knowingly (the install is then given 12 hours).
The refusal lives in `cua_vmm::auto::select`, in the QEMU runtime's start and in the build, from one
message (`cua_vmm::qemu::windows_emulation_message`). Why WSL2 on a Windows PC: QEMU for
Windows under WHPX hangs in OVMF's flash on current builds. Dogfooding
(`fix/winlike-windows-builder-whpx`) found the rest, all built in: stage files on the WSL ext4
disk, not `/mnt` (9p stalls); boot the install media first (a blank disk first leaves OVMF in the
EFI shell); press Enter to catch "Press any key to boot from CD". On a Mac and on Linux the installer
reads the VM's screen and presses Enter only while the prompt is showing (black with text in the top
rows), and stops when it is gone: a blind timer reached Setup's own window, whose focused button is
Cancel, twice ("Are you sure you want to quit?" stalled the install). The WSL2 driver has no screen
access and presses for the first 14 s only.

**ARM64 Windows on a Mac** runs `qemu-system-aarch64 -accel hvf -cpu host` on the `virt` machine with
edk2 firmware (the one Homebrew's QEMU ships; Microsoft's Secure Boot key pair is used when present,
and Setup's Secure Boot and TPM checks are bypassed in the answer file), an NVMe system disk, the
install and data discs as USB storage on xHCI, `virtio-net` (driver from virtio-win, loaded by the
answer file), and `ramfb` (800x600). There is no TPM on a Mac: HVF cannot map the TPM device's MMIO window
(`HV_BAD_ARGUMENT` at QEMU start), so Setup's TPM check is bypassed in the answer file and the
guest runs without one (arm64 Linux hosts with KVM keep a swtpm TPM 2.0).

Known gap: building on a Windows PC works through WSL2; **running** a UEFI Windows guest from
the Rust QEMU runtime on a Windows PC still uses WHPX, which hangs in OVMF on current QEMU. Until
that is fixed upstream or the runtime runs in WSL2 too, use the Python SDK's `QEMUWSL2Runtime`
or Hyper-V there.

## Trying it

```sh
cua sb create windows:11                      # ISO downloaded, built once (about 15 minutes on an M-series Mac, the download included), cached
cua sb create windows:11 --iso ~/Win11_ARM64.iso   # your own ISO instead
cua sb create windows:2025 --name srv         # x64 hosts
cua sb create windows:11 --kind vm --runtime qemu --cpu 4 --memory 8GB
```

In the New Space dialog the catalog entries are offered in the image picker; auto-download is the
default. The first Space of a version shows "Downloading Windows (first time only)" with the ISO's
bytes and time left, then "Installing Windows (first time only)" with a bar that only moves forward
(the app watches for a stalled create, so the build keeps its fraction rising even while Windows
writes nothing), then the ordinary "Preparing the disk", "Booting Windows" and "Starting Cua inside
the Space". Cancelling the create stops the install VM.

Choose Windows ISO...: the host op `windows.chooseIso {image}` opens the native picker (SwiftUI
`NSOpenPanel`, Electron's open dialog), then `windows_use_iso` (cua-spaces-ffi) checks the file and
registers it for the image, exactly as `--iso` does, so the create that follows installs from it
with no environment variable. A wrong file is refused at once in plain words. It is offered (1) as a
link in the Windows step of New Space and (2) on the failed create's screen, beside "Open Microsoft's
download page" and Try again, whenever the download is limited (Sentinel) or fails for another
reason (the core's `CreateFix` with action `choose-windows-iso`). After the pick the failed create
runs again as a new row. The CLI's own failure text says `--iso <path>`.

## Guard rails from dogfooding

* Disk: the build refuses on a full volume before downloading; in the guest the disk guard task
  trims logs and flags low space.
* `cua sb exec NAME bash -c 'echo "a b"'` now keeps words with spaces, quotes and backslashes
  whole (it used to join arguments with spaces and lose the quoting).
* Long scripts: do not pass them on the command line (`-EncodedCommand` over a few KB fails with
  "The command line is too long"). Copy the script, then run it:
  copy it with `cua sb cp` to the guest (for a Windows daemon: `direct:<addr>:C:/script.ps1`), then run `powershell -File C:/script.ps1`.
* `sb exec` has a 120 s default limit: `--timeout 0` for long jobs (or detach them).
* `cua sb exec NAME ...` on a Windows Space runs the line in `cmd.exe /d /s /c` (it used to try
  `/bin/sh`, which a Windows guest does not have).

## What is verified, and what is not

Verified on a Mac (Apple silicon, macOS 26.6, QEMU 10.2, HVF; a 12-core Mac Studio), from the CLI and in
the real Cua Spaces Local app:

* Nothing to Windows 11 Space, unattended: the ARM64 ISO is downloaded from Microsoft (8.5 GB, the
  SHA-256 on Microsoft's page matched; 7 minutes), Windows 11 installs, cua-spacesd installs, the disk is
  sealed and the Space boots to a desktop. With the ISO cached that is 8 to 9 minutes (app, New Space,
  Windows 11, Create Space: "Downloading Windows (first time only)" with bytes and time left, then
  "Installing Windows (first time only)" with a bar that moves, then "Booting Windows", then Running with the
  desktop streaming in the Space page).
* A second Space is a copy-on-write clone: cua-spacesd answers 36 to 40 seconds after Create (CLI 36 s, app
  36 s and 40 s, the second while the first was running).
* In the guest, through cua-spacesd: `cua sb screenshot`, `cua sb exec` (cmd.exe), `cua do click/type/key`
  (Notepad opened from the Start menu and typed into), and the stream and window list in the app.
* Cancelling a create (`cua spaces cancel`, the call the app's Cancel makes) stops the install VM and its TPM within seconds
  and releases the build lock.
* `--runtime auto`: ARM64 Windows on Apple silicon runs QEMU with HVF (`auto::select` unit test; the real
  runs). x64 Windows there is refused in plain words with the way out, in the New Space dialog, `cua sb
  create` and `cua spaces create` (the real CLI answers were read).
* Microsoft limits how many download links one network gets in a short time ("Sentinel marked this request
  as rejected"); the create then fails with that cause in words and `--iso` as the way out. It happened
  after about eight link requests in a day of testing.

Not verified:

* An interrupted ISO download resuming (unit tested with a fake server; a real retry was blocked by the
  rate limit above).
* Windows 10, Server 2019 and Server 2025 builds (x64): no x64 host was available for a real install; the
  answer file, ISO resolution and runtime selection are unit tested. The Evaluation Center scraping of the
  x64 media was not run against the live pages.
* The Windows PC path (WSL2 with KVM, WHPX): unit tested only. The install driver now waits for the guest to
  power itself off (as on the Mac) and presses Enter for the first 14 s only; no Windows PC has run it.
* The ARM64 Windows build of cua-spacesd: no release carries one yet. The guest tries the native asset first
  and falls back to the x64 build, which Windows 11 on ARM runs under its own emulation; that is what was
  verified.
* The display is `ramfb` at 800x600 on ARM64. virtio-win has an ARM64 `viogpudo`, which would allow a larger
  desktop; not done.
* No ISO-path field in the New Space dialog (`isoPath` needs the create request, the SDK options and the
  bindings changed): `cua sb create windows:11 --iso <path>` registers one. Windows 11 on a Mac needs none.
* Not done from the dogfooding list: `sb screenshot` stale frames, long-log disk usage in Spaces (guest disk
  guard only), exec 120 s ceiling (documented `--timeout 0`).
