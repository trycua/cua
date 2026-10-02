# libs/images

Reference guest images for the cua SDK, built for amd64 and arm64:

| Image | Contents | Used for |
|---|---|---|
| `linux` | Ubuntu 24.04, XFCE on Xvfb `:1`, AT-SPI, headless PipeWire audio, Firefox, **cua-spacesd** on `:3211` (with the HTML5 viewer at `/viewer`), conformance fixtures | The default recommended "Spaces image": `ghcr.io/trycua/linux` |
| `plain/ubuntu-xfce-vnc` | Ubuntu 24.04, XFCE on Xvnc `:1` (RFB `:5901`) and nothing else | Proving that sandboxes don't need cua daemons (VNC only) |
| `plain/ubuntu-server` | Ubuntu 24.04 and OpenSSH | Proving that sandboxes don't need cua daemons (SSH only) |
| `macos` | The Lume macOS base plus **cua-spacesd** at login, TCC grants seeded (see [macOS](#macos-lume)) | `ghcr.io/trycua/macos`, local Lume on Apple silicon |
| `omarchy` | Omarchy edge (Arch, Hyprland) with Omarchy's `cua-driver-bin` and `cua-hyprland-plugin` turned on, plus **cua-spacesd**. VM only, amd64 | `ghcr.io/trycua/omarchy:edge` ([guide](../../docs/content/docs/cua-sdk/guides/omarchy.mdx)) |

`cua sb create linux` resolves to `ghcr.io/trycua/linux:24.04`; pass any of these references (or a local tag) as the image, with `--on local|docker|qemu|fleet`. The same image runs locally and on Fleet.

## Canonical published images

Each image publishes directly under `ghcr.io/trycua/<os>` (`linux`, `windows`, `macos`, `omarchy`, `bench-*`). Linux release tags (`linux-image-v*`, or a `workflow_dispatch` with `publish`) run `.github/workflows/cd-image-linux.yml`. Older repos keep their existing tags untouched (`scripts/images/check-tag-safety.sh` and the CLI's tag policy guard every push).

`ghcr.io/trycua/cua-desktop-linux` is the pre-rename name of `ghcr.io/trycua/linux`. It is frozen and read-only: nothing pushes to it, and `scripts/images/check-image-refs.py` fails any new reference to the name outside its allowlisted legacy paths. For one release the old name still resolves where tools look it up: `build.sh cua-desktop-linux` and `cua images build libs/images/cua-desktop-linux` build `linux`, the image manifest lists it in `aliases`, and the e2e suites read `CUA_E2E_DISK_CUA_DESKTOP_LINUX`.

| Reference | Contents |
|---|---|
| `ghcr.io/trycua/linux:24.04` | OCI index, rootfs for docker / gVisor, amd64 + arm64 |
| `ghcr.io/trycua/linux:24.04-disk` | OCI index, KubeVirt containerDisk (`/disk/disk.img`, uid 107), amd64 + arm64 |
| `ghcr.io/trycua/windows:2022`, `2022-disk` | containerDisk, amd64, built by `windows-2022/` (`cd-image-windows.yml`): the Windows workspace plus cua-spacesd, gated on `cua-spacesd doctor --strict` in the guest |
| `ghcr.io/trycua/macos:26` | Lume image built by `macos/`, full tier (slim plus the Command Line Tools, Homebrew and dev tools); pin `26-20261001-021b87d` |
| `ghcr.io/trycua/macos:26-slim` | Lume image built by `macos/`, slim tier (the base plus cua-spacesd and Google Chrome); pin `26-slim-20261001-021b87d` |
| `ghcr.io/trycua/macos:15` | Lume image (copy of `macos-sequoia-cua`) |

- Every floating tag has an immutable dated pin: `<tag>-<yyyymmdd>-<sha7>`. Pins and per-arch children are written once. In the canonical repos only the floating tags (`<version>[-slim|-xcode[-X.Y]][-disk]`: `24.04`, `24.04-disk`, `2022`, `2022-disk`, `26`, `26-slim`, `15`) ever move, and only to a pin's digest. The macOS tiers also push `<pin>-raw` (the plain `lume push`) before `annotate.sh` writes the pin; it never moves.
- The first cua-spacesd `macos:26` (base plus cua-spacesd, no Chrome) stays at its pins `26-20260925-56503f3` (the `lume push`) and `26-20260925-3262e04` (annotated). It is the base every slim tier builds from (`macos/versions.env`), so it needs no other tag.
- The rootfs index carries the annotation `ai.cua.image.variants`, a JSON map of variant to pinned ref (`{"containerdisk": "ghcr.io/trycua/linux@sha256:..."}`). The disk index points back at the immutable rootfs pin. ghcr has no OCI 1.1 referrers API, so the link is annotation only.
- Copies carry `ai.cua.image.source` (the source ref and digest).
- Every primary also carries `ai.cua.image.os` (`linux`, `windows`, `macos`) and `ai.cua.spacesd` (`true` or `false`, what the image runs; images also carry the pre-rename `ai.cua.env-driver`, which the resolver still reads). The resolver prefers them over the repository name and port heuristics. The Linux configs carry both as labels too. Linux and `macos:26` have cua-spacesd; the Windows and `macos:15` copies do not (computer-server era).
- `scripts/images/annotate-canonical.sh` re-publishes this metadata without rebuilding: new immutable pins, then the moving tags.

## macOS (Lume)

`macos/` builds `ghcr.io/trycua/macos` with Lume instead of docker, in tiers: `26-slim` (the base VM, autologin user `lume`, SIP off, ssh, plus cua-spacesd and Google Chrome), `26` (full: slim plus the Command Line Tools, Homebrew and dev tools) and `26-xcode` (full plus Xcode). Each tier clones the gated VM of the tier below; pinned versions are in `macos/versions.env`. `cua images release` runs the pipeline (`macos/release.json`), on a Mac by hand and in `cd-image-macos.yml` on the self-hosted runner alike:

```bash
# per tier, in order (full clones <prefix>-slim, xcode clones <prefix>-full): build, then the gates
# (doctor --strict + ax probe from the build, and live input on a clone: tests/e2e_macos_input.rs)
cua images release libs/images/macos --tier slim --var prefix=cua-e2e-macos-<name> [--var base_vm=cua-base-<sha256(ref)[:12]>]
cua images release libs/images/macos --tier full --var prefix=cua-e2e-macos-<name>
# every pin: push <pin>-raw, annotate (tier, content key), attach the doctor report
# (record_ledger=true also records it on the image-doctor-ledger branch), verify
cua images release libs/images/macos --tier slim --var prefix=... --resume --publish --var record_ledger=true
# then the floating tags, through check-tag-safety.sh --moving
cua images release libs/images/macos --tier slim --var prefix=... --resume --publish --promote --steps promote
# gate a published image in a fresh VM (also the optional verify step, --assume pull-verify)
scripts/images/image-doctor-lume.sh --image ghcr.io/trycua/macos:26 --out /tmp/doctor --strict
```

Pass the same `--stamp` (and `--work`, when set) to every run of one release. `tiers.sh` still builds and gates the tiers in one go without publishing; a release run whose `--stamp` names the revision `tiers.sh` built reuses its gated VMs (`<prefix>-<tier>`, with `--var out=` pointing at its report) instead of rebuilding them.

- **Daemon:** `/Applications/Cua Spacesd.app` (ad-hoc signed by default; `CUA_ENV_CODESIGN_IDENTITY` for a real identity), `/usr/local/bin/cua-spacesd` links to it. The LaunchAgent `/Library/LaunchAgents/com.trycua.spacesd.plist` starts `/opt/cua/bin/start-spacesd.sh` in the GUI session at every login, on `0.0.0.0:3211` (QUIC media on 3212).
- **Token:** first of `$CUA_ENV_TOKEN`, the Lume setup share the SDK writes (`/Volumes/My Shared Files/setup/env-token`), `/etc/cua/env-token`, then the token an earlier boot kept. It is copied to `~/.cua/spacesd/token` (owner `lume`, 0600, directory 0700), which the driver reads. With none, the driver starts in bootstrap mode and the SDK installs one with `Init`. No token ships in the image.
- **TCC:** `files/seed-tcc.sh` writes Accessibility, Screen Recording and PostEvent rows to the system TCC database with a csreq derived from the installed bundle (`codesign -d -r-`; for an ad-hoc signature that is its cdhash, so the rows hold for exactly the binary shipped), and seeds the ReplayKit approval ledger so no capture alert appears.
- **Identity:** `/etc/cua-image/manifest.json` (from `image.json` claims and `cua-spacesd build-info`), `/etc/cua-image/spacesd-source`, `/etc/cua-image/variant` (`lume`).
- **Gate:** `doctor-gate.sh` runs `cua-spacesd doctor --strict` in the guest over `lume ssh` after a reboot, then `tools/ax_probe.py` (the official MCP SDK against the image's `/mcp`: permissions, an AX tree read, CGEvent clicks in Calculator and a session-keyed `move_cursor` that must return). The build fails unless both pass. The release's `input/arm64/lume` gate (`live-input-gate.sh`) then boots a clone of the built VM and runs `libs/cua/crates/cua-spaces-ext/tests/e2e_macos_input.rs`: a Dock click on a whole-display stream must be delivered and change the screen. The pushed VM itself never boots again after its sanitize.

## Image references

`sandbox-images.json` is the one list of images the repo points at: the apps, the CLI (`cua images ls`), the docs tables and cua-bench read it. Benchmark entries name their `bench/<id>/lock.json` pins and the benchmarks they run.

- A new or renamed tag goes in `sandbox-images.json` first. Then run `pnpm --dir docs docs:generate:sandbox` and `python3 scripts/images/gen-image-constants.py`.
- `scripts/images/check-image-refs.py` (**CI: Check Image Refs**) fails on any image ref in code, tests, CI, Dockerfiles or docs that is not a catalog entry, a lock pin, or a reasoned entry of `scripts/images/image-refs-allowlist.json` (legacy, cloud-built, third-party and synthetic test refs). Unused allowlist entries fail too.
- Docs pages list images through generated regions (`GENERATED:catalog-cua-images`, `GENERATED:catalog-benchmarks`), not hand-typed tables.

## One definition, two outputs

Each image is one Dockerfile, the **container rootfs**. The VM disk is derived from that same rootfs, so both artifacts carry the same guest.

| Output | Tag (cloud convention) | Runs on | Init |
|---|---|---|---|
| rootfs | `<repo>:docker-<tag>` | docker, gVisor (`runsc`) | supervisord |
| containerDisk | `<repo>:<tag>` (`FROM scratch` + `/disk/disk.img`, uid 107) | KubeVirt, local QEMU | systemd |

Known limitation: Firefox does not start under gVisor on amd64. Its wasm2c sandboxing needs `arch_prctl(ARCH_SET_GS)`, which gVisor does not implement on x86_64. Use Chromium there (cua-driver's browser tools already do). Firefox works under runc, in the VM, and under gVisor on arm64. The image's `launch_app` claim lists this in `limits`, and the doctor launches Chromium on that runtime and says why.

```
Dockerfile ──buildx──▶ rootfs image ──▶ :docker-<tag>
                           │
         common/vm/Dockerfile (kernel+initramfs, systemd, cloud-init,
                           │   netplan 99-kubevirt.yaml, sshd, qemu-guest-agent;
                           │   enables the units listed in /etc/cua-image/vm-units)
                           ▼
         buildx --output type=tar,dest=-  │ (piped, never touches the host fs)
                           ▼
         common/disk-builder (unprivileged container, target arch)
           mkfs.ext4 -d rootfs  +  mkfs.vfat/mtools ESP  +  sfdisk GPT
           UEFI: grub-mkstandalone → EFI/BOOT/BOOT{AA64,X64}.EFI
           BIOS (amd64): grub-mkimage core + boot.img placed by hand (no device probing)
           qemu-img convert -c → disk.img (20G virtual, ~0.5 GB compressed)
                           ▼
         common/containerdisk.Dockerfile ──▶ :<tag>
```

### Why this disk approach

The disk builder needs no loop devices, nbd, libguestfs, `--privileged` or KVM. Every filesystem is built as a plain file and spliced into the disk at its partition offset. That is the only approach that works the same way:

- inside Colima on an arm64 Mac, where the Docker VM has no nbd module and loop partitions don't show up in containers;
- on GitHub runners.

Other design choices:

- **Bootloader.** GRUB is written into the image, never installed in the rootfs, so `grub-probe` never runs against a device that doesn't exist.
- **Disk layout** (the same on both arches):

  | Partition | Size | Purpose |
  |---|---|---|
  | p1 | 1 MiB | BIOS boot |
  | p2 | 64 MiB | ESP, label `UEFI` |
  | p3 | rest | ext4, label `cloudimg-rootfs` |

  cloud-init `growpart` grows p3.
- **Firmware.**
  - amd64 boots under both SeaBIOS (KubeVirt's default `bios`) and UEFI.
  - arm64 boots UEFI only, which is all QEMU virt and KubeVirt support on arm64.

## Build

```bash
# rootfs only (host arch), without the driver
libs/images/build.sh linux --build-arg CUA_SPACESD_SOURCE=none
# all three outputs
libs/images/build.sh plain/ubuntu-server --outputs rootfs,disk,containerdisk
# other arch (binfmt: Rosetta/qemu-user), or several
libs/images/build.sh linux --platform linux/amd64,linux/arm64 ...
# join pushed per-arch tags into multi-arch indexes (a registry you own)
libs/images/build.sh manifest localhost:5000/linux pr-1234abcd
```

- **Local tags:** `cua-e2e-local/<name>:docker-local-<arch>` (rootfs) and `cua-e2e-local/<name>:local-<arch>` (containerDisk).
- **Disks:** written to `~/.cache/cua-images/<name>/<arch>/disk.img`.

### Refresh the images the tests use

The e2e suite (`tests/e2e/cua-sdk`) and the spacesd conformance runner default to `cua-e2e-local/linux:docker-local-<arch>`. A cached tag keeps the driver it was built with, so rebuild it after changing `libs/cua-spacesd`:

```bash
libs/images/linux/build-spacesd-linux.sh     # host arch driver into dist/<arch>/
libs/images/build.sh linux                      # cua-e2e-local/linux:docker-local-<arch>
# QEMU lane disk (optional): ~/.cache/cua-images-e2e/linux/<arch>/disk.img
libs/images/build.sh linux --tag e2e --outputs rootfs,disk --out ~/.cache/cua-images-e2e
```

Check which driver an image carries with `docker run --rm --entrypoint cat <image> /etc/cua-image/spacesd-source` (`local`, `release` or `none`) and compare `docker image inspect -f '{{.Created}}'` with your last driver change.

### With the cua CLI

`cua images build <dir>` runs the same pipeline as `build.sh` (an image's own
`vm.Dockerfile` replaces `common/vm/Dockerfile`), `cua images pack` pushes the
disk the doctor checked as a containerDisk, `cua images publish` writes the
immutable pins and `cua images promote` moves the floating tags after the gates
(`.github/workflows/cd-image-linux.yml`, `.github/workflows/cd-image-omarchy.yml`).

```bash
cua images build libs/images/linux --outputs rootfs,disk        # host arch
cua images build libs/images/omarchy --outputs rootfs,disk      # amd64; emulated on arm64 hosts
cua images pack libs/images/omarchy --tag ci-1 --push
cua images publish libs/images/omarchy --tag ci-1 --series edge --record pins.json
cua images promote pins.json
```

### Release pipeline: `cua images release`

Every published image has a `release.json` next to its definition (`linux`,
`omarchy`, `windows-2022`, `bench/osworld`, `bench/web`). It lists the steps as
the same scripts and `cua images` calls CI runs, in phases: `prepare`, `build`,
`gate` (doctor lanes, conformance, e2e, smokes), `stage` (content digest, the
doctored bytes saved), then with `--publish` `push`, `publish` (immutable pins)
and `verify`, and with `--promote` `promote`. One command runs it, on a laptop
or a CI runner alike:

```bash
# See the plan: what runs, what this host skips and why
cua images release libs/images/linux --tier slim --dry-run
# Build and gate one arch; after a failure fix it and resume
cua images release libs/images/linux --tier slim --arch arm64
cua images release libs/images/linux --tier slim --arch arm64 --resume
# Rerun from the gates (the build must have passed with the same inputs)
cua images release libs/images/linux --tier slim --arch arm64 --from gate
# Publish and promote once every gate passed (here, or in merged CI evidence)
cua images release libs/images/omarchy --publish --promote --resume
```

- **Work directory:** `--work`, else `$CUA_RELEASE_WORK`, else
  `~/.cua/build/release-<name>[-<tier>]` (the build cache `cua cache prune`
  manages). Build outputs go to `out/`, the doctored bytes to
  `artifacts/<arch>/`, and everything to keep to `evidence/`: one log per step
  (`logs/`), the summary table (`summary-<scope>.md|json`, also the CI job
  summary), doctor reports, pins. `evidence/` is a plain directory CI uploads
  as-is.
- **Resume:** each step's result is recorded in `evidence/state-<scope>.json`
  with a fingerprint of its command and the tree (HEAD plus the uncommitted
  diff). `--resume` skips steps that passed with the same inputs and whose
  outputs still exist; `--from <phase|step>` reruns from there and refuses if
  an earlier step has not passed.
- **Host capabilities:** steps name what they need (`docker`, `runsc`, `kvm`,
  `native-<arch>`, `accel-<arch>`, `windows`); a host without it skips the
  step and says why. `--assume kvm` or `--assume '!runsc'` overrides the
  detection; CI runners pass `--assume lowdisk` for the OSWorld reclaim steps.
- **Gates hold:** push refuses unless every required gate of the arches passed,
  and promote unless every required verify step passed, in this run or in a
  `state-*.json` merged from other jobs. So CI splits arches and phases across
  jobs (`--arch`, `--steps`, `--scope`) and the gates still hold.
- **Attest:** every image's `attest` step attaches its doctor reports to the
  pushed per-arch children and, with `--var record_ledger=true` (CI publish
  runs), records them on the `image-doctor-ledger` branch, before the floating
  tags move. Windows attests the build gate and the pulled verify on the disk
  child both of its indexes list; the benchmark images attest the smokes of
  every arch and variant of the staged pins (`scripts/bench-images/attest.sh`).
- **Catalog:** the last promote step, `catalog`, moves the promoted tags'
  `digest` in `sandbox-images.json` and records their `sizes` (download,
  unpacked and disk bytes per platform, what the Spaces apps show) with
  `scripts/images/record-image-sizes.py`; commit the file. CI fails when an
  entry's sizes were measured at another digest.
- **CI is a thin wrapper:** `cd-image-linux.yml`, `cd-image-omarchy.yml`,
  `cd-bench-images.yml` and `cd-image-windows.yml` install the host tools
  (`scripts/images/install-release-tools.sh`) and run slices of the command.
  To reproduce a job, run its `cua images release` line with the job's
  `--stamp`.

#### Linux publishing (`cd-image-linux.yml`)

1. **Release jobs, per tier and arch on native runners:** `--steps prepare,build,gate,stage`: `cua images build --target <tier>`, then the gates on the unpublished bytes: `cua-spacesd doctor --strict` on the rootfs under runc and gVisor, on the amd64 disk under QEMU+KVM (local token, then the claim-secrets share; arm64 disks get the same doctor under QEMU TCG, as hosted arm64 runners have no KVM), conformance, the claim-token path and amd64 parity. Nothing is pushed.
2. **Push the doctored bytes** (publish runs only), `--steps push/<arch>`: dated per-arch children `ghcr.io/trycua/linux:docker-<tag>-<arch>` (rootfs) and `ghcr.io/trycua/linux:<tag>-<arch>` (containerDisk), `<tag>` being `build-[slim-]<yyyymmdd>-<sha7>`.
3. **Attest, pin, verify, promote**, `--steps attest,publish,verify,promote`: every doctor report becomes an OCI referrer of the child it checked and an image-doctor ledger entry; `<series>-disk-<yyyymmdd>-<sha7>` and `<series>-<yyyymmdd>-<sha7>` are written, refusing any that exist; the pins' children must be the pushed digests and their disks hash to the doctored `disk.img`; only then `<series>-disk` and `<series>` move.

A publish run whose gates and pushes passed but that stopped before its floating tags moved is finished with `workflow_dispatch` `promote_run=<run id>`: it merges that run's gated evidence, push records and publish evidence and runs `--resume --steps attest,publish,verify,promote` (steps that already passed there are skipped), building and pushing no image. A child without a doctor verdict passes verify only when no doctor lane ran for it (`scripts/images/verify_pins.py`). Every arch's disk gets the strict VM doctor: hosted arm64 runners have no KVM, so `image-doctor-lane.sh` runs it under QEMU TCG with stretched budgets (`--timeout-scale`, default 8 under TCG). `workflow_dispatch` `doctor_children=arm64@sha256:...,...` doctors disk children that are already published and attests them (report referrer and ledger entry) without pushing any image. It runs this commit's doctor (`image-doctor-lane.sh --doctor-bin`) against the image's own service, since published disks bake the older doctor. The lane waits for cloud-init and a serving desktop before the doctor starts, and under TCG the timing checks (audio uplink tone, A/V sync) stretch with `CUA_DOCTOR_TIMEOUT_SCALE`; accelerated lanes (scale 1) are unchanged.

Pull requests run step 1 only. The plain images are built and smoke tested in the same workflow and not published.

### cua-spacesd

`CUA_SPACESD_SOURCE` selects where the image gets the driver:

| Value | Behaviour |
|---|---|
| `local` (default) | Copies `linux/dist/<arch>/cua-spacesd`. `linux/build-spacesd-linux.sh [arm64] [amd64]` produces it by building `libs/cua-spacesd` in `rust:1-bookworm`: natively, via a multiarch cross toolchain (`CROSS_MODE=cross`), or under emulation (`CROSS_MODE=emulate`). |
| `release` | Downloads `CUA_SPACESD_RELEASE_URL`, a placeholder pattern with `{version}` and `{arch}`. |
| `none` | No driver. The supervisor program idles. |

The driver runs inside the desktop session, bound to `0.0.0.0:$CUA_ENV_PORT` (3211, QUIC media on 3212). A non-loopback bind needs a token, so a missing token is fatal rather than anonymous.

It gets its token through the `CUA_ENV_TOKEN` environment variable, never through argv. The first of these sources that is set wins:

1. `$CUA_ENV_TOKEN`
2. `$CUA_ENV_TOKEN_FILE`
3. `/run/cua/env-token`
4. `/etc/cua/env-token`, for example from cloud-init `write_files` on Fleet

If none of them is set, a random token is generated into `/run/cua/env-token` (root:cua, 0640).

#### Fleet claim token (await-token-file mode)

On Fleet the env token is per claim. The operator puts it in a Secret: `/run/cua/env-token`, root 0600, empty until a claim binds, rewritten on rotation, emptied on release. The driver never accepts a token over the network.

- **Detection** (`files/env-token-mode.sh`): await mode is on when `/run/cua` is a mount point and neither `CUA_ENV_TOKEN` nor `/etc/cua/env-token` is set. `CUA_ENV_AWAIT_TOKEN_FILE=1|0` forces it. Local docker (the SDK passes `CUA_ENV_TOKEN`) and plain QEMU boots are unchanged.
- **Driver:** starts as `cua` with `--await-token-file` and binds `:3211` with no token. Until a token arrives it answers only `GetCapabilities` and `Health`. It then installs, rotates and revokes the token as the file changes (polled every 500 ms).
- **Privilege split:** `cua-spacesd token-sync` runs as root (supervisord `cua-env-token-sync`, systemd `cua-env-token-sync.service`). It mirrors the root-only file to `/run/cua-env/env-token` (cua 0600, directory root 0755), which is what the driver follows. If `cua` can read `/run/cua/env-token` directly, the driver follows that file instead.
- **gVisor / macOS pods:** Kubernetes mounts the Secret at `/run/cua`.
- **KubeVirt:** `run-cua.mount` mounts virtiofs tag `cua-claim-secrets` read-only at `/run/cua`, before the sync and driver units. A udev rule pulls it in only when a virtio-fs device exists, so VMs without the share boot clean and keep their local token.
- **Local VM test:** `linux/smoke-claim-token-vm.sh` boots the disk with a 9p share (macOS QEMU has no virtiofsd) under the same tag, plus a `run-cua.mount` drop-in for `Type=9p`. Everything above the mount is the real path. virtiofs itself is only exercised on KubeVirt.
- `ensure-env-token.sh` leaves `/run/cua` alone in await mode.

Requirements for the cloud pod spec (trycua/cloud#7885):

- Mount the Secret as a directory at `/run/cua`. Do not use `subPath`: a `subPath` file is not a mount point of `/run/cua` and never receives updates.
- Keep the container's user root (the image default). supervisord runs the root sync helper and drops the desktop, driver and other programs to `cua`.
- If the pod must run as non-root (`runAsUser`), set `fsGroup: 1000` and the Secret's `defaultMode: 0440`. The `cua` user can then read the file and the driver follows it directly. Keep it out of "other" (not 0444): the driver refuses world-readable token files unless it runs as root in a container.

## Test

```bash
libs/images/linux/smoke-test.sh --runtime runc    # or runsc (gVisor)
libs/images/linux/smoke-claim-token.sh --runtime runsc   # Fleet claim token path (tmpfs /run/cua)
libs/images/plain/smoke-test.sh ubuntu-xfce-vnc --runtime runsc
libs/images/plain/smoke-test.sh ubuntu-server --runtime runsc
libs/images/common/boot-qemu.sh ~/.cache/cua-images/linux/arm64/disk.img \
    --probe 3211 --ssh --env-token t0ken
```

- `common/tools/rfb_snapshot.py` is a stdlib-only RFB client. The plain VNC image test uses it to grab frames from the host without anything installed in the guest.
- CI (`.github/workflows/cd-image-linux.yml`) builds every image × arch on native runners, doctors `linux` (smoke tests for the plain images), boots the disks under QEMU, and publishes `ghcr.io/trycua/linux` (see [Linux publishing](#linux-publishing-cd-image-linuxyml)).

## Fixtures (`/opt/cua/fixtures`, linux and omarchy)

Use `cua-fixtures start|stop|status [grid|form|http|tone|avsync|all]`. `all` covers every fixture except `avsync`. Each fixture logs JSONL to `/tmp/cua-fixtures/<name>.jsonl`.

- **`grid`** is an 8×6 grid of 80 px cells, titled "CUA Fixture Grid", with WM_CLASS `cua-fixture-grid`.
  - Cell (c, r) is painted `rgb(c*255//7, r*255//5, 128)`, so any pixel identifies its cell.
  - Every pointer, scroll, key, focus and configure event is logged with its cell.
- **`form`** is "CUA Fixture Form", with accessible widgets `Name`, `Notes`, `Subscribe`, `Color`, `Submit` and `Status`. It logs changes and submits.
- **`http`** listens on `0.0.0.0:18080`. Routes: `/`, `/health`, `/bytes/<n>` (deterministic bytes, sha256 in `X-Sha256`) and `POST /echo`.
- **`tone`** (`/opt/cua/fixtures/tone`) loops a fixed sequence into the default sink: 440 Hz for 500 ms, 250 ms of silence, 880 Hz for 500 ms, 250 ms of silence. It logs each segment's start.
- **`avsync`** opens "CUA Fixture AV Sync". At every wall-clock second it turns white for 100 ms and plays a 1 kHz, 100 ms beep at the same instant. It logs `flash`, `flash_drawn` and `beep` with a shared `boundary` and one monotonic clock. The skew budget is ±40 ms.
- **`audio_probe.py`** records from any Pulse source and reports level plus Goertzel tone power as JSON. It is the in-guest audio oracle.

## Audio

Headless PipeWire runs under supervisord in the container and as `cua-audio.service` in the VM. It consists of `pipewire`, `wireplumber` and `pipewire-pulse`, and the Pulse socket is `/run/cua-desktop/pulse/native`.

| Node | Kind | Role |
|---|---|---|
| `cua_desktop` | Null sink, the default | Receives everything apps play. Desktop audio is captured from its monitor, `cua_desktop.monitor`. |
| `cua_mic` | Source, the default | The virtual microphone apps record from. It is the far end of a loopback. |
| `cua_mic_in` | Sink | The near end of that loopback. The uplink feeds the mic by playing into it, for example `pacat --device=cua_mic_in`. |

- It works under both runc and runsc.
- The user-session PipeWire units are masked, so an SSH login in the VM doesn't start a second server.
- The session runtime dir is `/run/cua-desktop`, not `/run/user/1000`. logind would mount over `/run/user/1000` and remove it for each login.

`desktop-env <cmd>` runs a command inside the session environment: `DISPLAY`, the fixed session bus `unix:path=/run/cua-desktop/bus`, and the a11y bridge.
