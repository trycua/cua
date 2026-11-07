# microVM fork benchmark on Windows (QEMU + WHPX)

Same harness as `../microvm-fork` (the Mac numbers behind cua.ai/compare), with the host
memory probe changed. It drives the real `cua` CLI and daemon. Image
`ghcr.io/trycua/linux:24.04-slim`, 2 vCPU / 2 GB Spaces, 256 MiB written to the source's
tmpfs before the checkpoint. This PC: Windows 11 Pro 26200, QEMU 11.1 with
`-accel whpx`, debug build of `cua.exe`, `N=5`, `FANOUT=4`. Raw samples: `results/`.

| Row | Mac (cua-vz, M2 Max) p50 | Windows (QEMU/WHPX) p50 | Why |
|---|---|---|---|
| checkpoint | 650 ms | 308 ms | disk only: CPUs stopped, `qemu-img convert -B template` of the sparse scratch |
| restore (fork of a checkpoint) | 610 ms | 3970 ms | cold boot of the saved disk |
| fork of a running Space | 1160 ms | 4052 ms | checkpoint + cold boot |
| fan-out (4 at once) | 1460 ms | 5202 ms | 4 cold boots in parallel |
| create from a warm pool | (claim) | 360 ms | pooled VM is booted and paused; claim = hard links + resume |
| memory per running fork | 1620 MiB | 684 MiB | guest uses 344 MiB; QEMU commits what the guest touches |

## Why restore and fork stay slower

WHPX does not support QEMU migration (`migrate` fails: the accelerator has no dirty-page
tracking or state save), so a running VM's memory cannot be saved and restored. This is
a platform limit, not tuning: a restore must boot Linux. Where the ~4 s goes: VM boot to
the init ~1.2 s, the image entrypoint ~1.3 s, `cua-spacesd` ~1.0 s, the rest CLI/daemon.
The two things that remove it are a pool (claim 0.36 s, shown above) and keeping the
source running. Checkpoint is faster than the Mac because it saves no memory.

## Iterations

| # | Change | Effect |
|---|---|---|
| 0 | first working checkpoint/fork (copy of the whole raw scratch disk) | fork 6-7 s |
| 1 | scratch = qcow2 over a shared read-only template | checkpoint 300 ms, fork ~4.1 s |
| 2 | agent wait attempt 250 ms (slirp holds a SYN to a not-yet-listening guest ~2 s) | boot-to-ping 2.5 s artifact gone, -2 s |
| 3 | ACPI power-off instead of reboot (QEMU hangs on reboot under WHPX) | stop/start reliable |
| 4 | warm pool: booted + paused members, claim by NTFS hard links (daemon reads Windows host memory) | create 360 ms |
| 5 | final run (`results/iter2`) | table above |

Not kept: nothing regressed. Rows that stay slower: restore, fork, fan-out (memory
snapshots are impossible under WHPX, see above).
