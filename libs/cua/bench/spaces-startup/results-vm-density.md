# VM density on one Mac: measured 2026-10-20

**Host:** Mac Studio (Mac14,13), Apple M2 Max with 12 CPUs (8 performance + 4
efficiency), 96 GB, macOS 26.6.2. QEMU 10.2.2 from Homebrew on HVF.

**VMs:** QEMU `vm` Spaces with 1 vCPU and 1024 MiB, started 8 at a time
through the SDK (`bench.sh vm-density`). The exec probe runs 20 round trips
on every VM at once. Raw samples are under the run labels below.

## Results

| Run | Build | Image | VMs per step | Stopped at | Why it stopped | Memory per VM | Exec p95 (1 VM → max) | Host at max |
|---|---|---|---|---|---|---|---|---|
| before (`hqm`, bench agent) | main | published `linux:24.04-slim` (cold boots) | headless command | 64 clean; 96 with 22 failed creates; 128 with 32 failed | p95 > 2x at 96 (7.5 ms); thrashing at 128 | ~1,245 MiB RSS | 2.5 → 27.4 ms | 325 MB free at 96; 25 GB compressed and load 73 at 128 |
| R1 `after-published-rollout` | this branch | published `linux:24.04-slim` (cold boots) | `rollout` profile | **66** | admission: VM memory cap (76.8 of 96 GiB) | 1,177 MiB private | 1.1 → 1.5 ms | pressure normal |
| R2 `after-cow-rollout` | this branch | slim disk, current cua-spacesd (warm starts, shared RAM) | `rollout` profile | **96** (+1 other agent's VM) | admission: vCPU limit (8x of 12 CPUs = 96) | 318 MiB private + 704 MiB shared once = **336 MiB** | 1.1 → 1.4 ms | 32.6 GB used by VMs, pressure normal |
| R3 `after-cow-rollout-oc32` | this branch, `CUA_VM_VCPU_OVERCOMMIT=32` | same as R2 | `rollout` profile | **128** | macOS hypervisor limit: QEMU fails with `HV_NO_RESOURCES` (`kern.hv.max_address_spaces` = 128) | 327 MiB | 1.1 → 1.5 ms | 41.8 GB used by VMs, pressure normal |

## What this shows

**Admission stops before the host degrades.** On main, VMs kept starting
until the Mac ran out of memory. Creates then failed, exec p95 rose 11x, and
the host swapped (load 73). With admission, R1 stopped at the memory cap
(`CUA_VM_MAX_RSS_FRACTION` 0.8). Exec p95 stayed at 1.5 ms, and the refusal
names the limit:

```
host capacity: this host is at its VM memory limit: its VMs use 76.4 GiB
(shared snapshot pages counted once) of the 76.8 GiB they may use
(max_rss_fraction 0.8 of 96.0 GiB), and 'bench-vmd-57-1' is expected to need
1.2 GiB more; ...
```

**Copy-on-write sharing is the memory lever.** A cold-booted 1 GiB guest
touches all of its RAM: about 1.2 GiB per VM. VMs that restore one golden
snapshot share its unwritten pages. In R2 each VM's own memory was 318 MiB,
and the 704 MiB snapshot was counted once, which is 3.5x denser. The
published image's cua-spacesd (protocol revision 8) can't become a new
machine, so its VMs still boot cold. Republishing it (E2B migration item 6)
gives every user the R2 numbers.

**Past memory, two limits remain:**

- **The vCPU overcommit** (default 8x) stopped R2 at 96 VMs with exec p95
  flat. These rollout VMs were idle. Raise it with `CUA_VM_VCPU_OVERCOMMIT`
  for workloads that mostly wait.
- **macOS caps a Mac at 128 VMs.** `kern.hv.max_address_spaces` is 128, and
  every QEMU and Apple Virtualization VM on the machine counts, whatever its
  memory. Admission now checks this and refuses with `HostCapacity` instead
  of letting QEMU fail with `HV_NO_RESOURCES`.

**Free page reporting does not return memory on macOS.** This was a VM with
anonymous RAM, `virtio-balloon-pci,free-page-reporting=on`, and the guest's
`virtio_balloon` driver bound (`page_reporting_order` 9). The guest wrote
400 MiB to tmpfs, and QEMU's footprint grew from 1,186 to 1,492 MiB. The
guest then freed it (`free` went from 30 to 412 MiB). QEMU's footprint stayed
at 1,492 MiB 5, 15, 30 and 45 s later.

QEMU discards a reported page with `madvise(MADV_DONTNEED)`, which on
Darwin leaves the page resident and charged to the process. In a separate
test, a 512 MiB anonymous region kept its 512 MiB footprint after
`MADV_DONTNEED`; `MADV_FREE_REUSABLE` or a fresh `mmap(MAP_FIXED)` returned
it. Free page reporting is therefore on by default only on Linux hosts, where
the discard frees the page (`CUA_QEMU_FREE_PAGE_REPORTING=1` forces it on a
Mac).

### Proof of concept: does a patched discard help on a Mac?

Two experiments on a Mac Studio (M2 Max, QEMU 10.2.2 from Homebrew, Lima 2.2.0).
Both used library injection (`DYLD_INSERT_LIBRARIES`) on a local branch only,
to measure; neither is shipped.

| Experiment | What it did | Memory returned to macOS |
|---|---|---|
| QEMU discard | Turned QEMU's `madvise(MADV_DONTNEED)` into `MADV_FREE_REUSABLE`. The guest wrote 700 MiB and freed it. QEMU made 70 discards, 782 MB, none failed. | **0 MiB.** The VM's footprint was 1,068 MiB at boot (a 1 GiB guest is fully resident from the start under HVF) and 1,068 MiB afterwards. |
| Runtime VM balloon | Applied a 2,600 MiB target to the Virtualization.framework memory balloon that Lima attaches but never sets, while a Space kept running. | **0 MiB.** The VM stayed at 4,077 MiB after the guest freed 1.5 GiB. |

So these calls do not return memory on a Mac. The likely cause is that a
guest's RAM is mapped into the hypervisor (HVF, or Virtualization.framework)
and stays charged while mapped; this was not isolated further. A real fix
needs a different mechanism, such as unmapping the freed range from the
hypervisor (a QEMU memory-backend change) or a runtime that implements free
page reporting for macOS; the clean options are in the architecture note in
the PR. What does return
memory today: stopping an idle runtime VM, which is fixed and tested, and
packing VMs through shared golden snapshots, which is the density
result above (318 MiB private per VM).

## Per Mac, extrapolated

Assumptions:

- Memory: the default cap (0.8 of physical memory), less the shared
  snapshot, divided by the private memory per VM.
- vCPUs: 8x the CPUs, 1 vCPU per VM.
- Hypervisor: 128 VMs.

| Mac | Memory allows (shared snapshot) | Memory allows (cold boots) | vCPU limit at 8x | Hypervisor | VMs per Mac |
|---|---|---|---|---|---|
| this one: 96 GB, 12 CPUs | 245 | 66 (measured) | 96 | 128 | **96** (128 at 11x) |
| 64 GB (e.g. 14 CPUs) | 162 | 44 | 112 | 128 | **112** (128 at 10x); 44 cold |
| 128 GB (e.g. 16 CPUs) | 327 | 88 | 128 | 128 | **128**; 88 cold |
| 128 GB, 24+ CPUs | 327 | 88 | 192+ | 128 | **128**; 88 cold |

With shared snapshots, every Mac with 64 GB or more reaches the 128-VM
hypervisor cap before memory. So 256 concurrent rollouts need at least 2
Macs, or 3 at 1 vCPU and 8x on 12-16 CPU Macs. With today's published image
(cold boots, ~1.2 GiB per VM), they need 3 Macs of 128 GB or 6 of 64 GB.
Each rollout's own writes come on top: R2's guests were idle, and a Docker
Compose world that writes 1 GiB adds about 1 GiB per VM.
