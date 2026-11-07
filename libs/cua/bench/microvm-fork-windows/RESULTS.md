# microVM fork benchmark on Windows (QEMU + WHPX)

Same harness as `../microvm-fork` (the one behind the published Mac numbers), with the host
memory probe changed. It drives the real `cua` CLI and daemon. Image
`ghcr.io/trycua/linux:24.04-slim`, 2 vCPU / 2 GB Spaces, 256 MiB written to the source's
tmpfs before the checkpoint. This PC: Windows 11 Pro 26200, QEMU 11.1 with `-accel whpx`, the **release** `cua-local.exe`
with its daemon, `N=10`, `FANOUT=4`, the machine otherwise idle. Raw samples: `results/iter3`.

| Row | Mac (cua-vz, M2 Max) p50 | Windows cold boot p50 | Windows with a warm standby p50 |
|---|---|---|---|
| checkpoint | 650 ms | 299 ms | 344 ms |
| restore (fork of a checkpoint) | 610 ms | 3952 ms | 485 ms (p95 4147 ms) |
| fork of a running Space | 1160 ms | 4060 ms | 4049 ms (not served by a standby) |
| fan-out (4 at once, per Space) | 1460 ms (all 4) | 5092 ms (all 4) | 2910 ms each, 5861 ms for all 4 |
| create from a warm pool | (claim) | 360 ms | 360 ms |
| memory per running fork | 1620 MiB | 697 MiB | 814 MiB (includes the standbys held) |

Cold-boot columns: release `cua-local.exe`, `N=10`, `FANOUT=4`, idle machine (`results/iter3`).
Standby column: debug build (the CLI is not the bottleneck), `N=10` back to back (`results/iter5-standby`).

A restore or fork here means **cold boot from the disk state** (a warm standby is the same
thing booted ahead of time): files, installed packages and
the filesystem come back, running processes do not (they start again from the image's entrypoint).

## Why restore and fork stay slower

WHPX does not support QEMU migration (`migrate` fails: the accelerator has no dirty-page
tracking or state save), so a running VM's memory cannot be saved and restored. This is
a platform limit, not tuning: a restore must boot Linux. Where the ~4 s goes: VM boot to
the init ~1.2 s, the image entrypoint ~1.3 s, `cua-spacesd` ~1.0 s, the rest CLI/daemon.
`migrate` fails with "State blocked due to missing dirty memory tracking support, and some
system register/state save-restore": QEMU 11.1 registers that blocker unconditionally for WHPX,
so only a patched QEMU could lift it (not something the app can install with winget). The
release build is no faster than the debug one (3952 vs 3970 ms): the time is in the guest.
The one thing that removes it is a pool (claim 0.36 s, shown above) for new Spaces. Checkpoint is faster than the Mac because it saves no memory.

## Iterations

| # | Change | Effect |
|---|---|---|
| 0 | first working checkpoint/fork (copy of the whole raw scratch disk) | fork 6-7 s |
| 1 | scratch = qcow2 over a shared read-only template | checkpoint 300 ms, fork ~4.1 s |
| 2 | agent wait attempt 250 ms (slirp holds a SYN to a not-yet-listening guest ~2 s) | boot-to-ping 2.5 s artifact gone, -2 s |
| 3 | ACPI power-off instead of reboot (QEMU hangs on reboot under WHPX) | stop/start reliable |
| 4 | warm pool: booted + paused members, claim by NTFS hard links (daemon reads Windows host memory) | create 360 ms |
| 5 | release build, N=10 (`results/iter3`) | table above; same as debug within 1% |
| 6 | `-cpu host` instead of qemu64; `-smp 4` instead of 2; 4 GB instead of 2 GB; | no effect (ready 3.6 s either way) or worse; not kept |
| 7 | warm standbys keyed to a checkpoint's disk (`CUA_MICROVM_STANDBY`, default 2): a VM booted from the checkpoint's disk and kept paused; a fork of the checkpoint takes it (hard links, resume, identity) and the missing ones are rebuilt in the background; evicted when the checkpoint is deleted, after 30 minutes idle, when the host is short of memory, or for a busier checkpoint | restore p50 3952 -> 485 ms; 7 of 10 back-to-back restores hit (about 450 ms), the others boot cold (4.1 s) because a standby takes a boot (4+ s) to rebuild and the loop forks faster than that |

Standby semantics: still "cold boot from the disk state" (processes start from the image's
entrypoint, files come from the checkpoint); only the boot happens before the fork. A fork of a
running Space is a checkpoint plus a fork, so it is not served by a standby and stays at 4 s.
A standby holds about 700 MiB of host memory while it waits; `CUA_MICROVM_STANDBY=0` turns them off.

Not kept: nothing regressed. Rows that stay slower: restore, fork, fan-out (memory
snapshots are impossible under WHPX, see above).
