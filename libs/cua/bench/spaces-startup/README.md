# Spaces startup, snapshot and density benchmarks

`bench.sh` drives the real `cua` CLI and daemon. Raw samples go to
`$OUT/*.jsonl`, and the summary goes to `$OUT/summary.json` or to the last
line of each density file.

| Phase | Host | What it measures |
|---|---|---|
| `microvm`, `minimal`, `gvisor`, `density` | Linux with KVM | Firecracker and gVisor starts, checkpoints, forks and density |
| `vm-density` | macOS with HVF, or Linux with KVM | How many QEMU `vm` Spaces one host runs, and what each one costs |
| `guard SUMMARY BASELINE` | any | Fails when a `vm-density` run falls past the baseline's thresholds |

## VM density (`vm-density`)

```bash
cargo build --release -p cua-cli --bin cua
cargo build --release -p cua-sdk --example rollout_bench  # a separate build: --example builds no binaries
CUA=target/release/cua ROLLOUT_BENCH=target/release/examples/rollout_bench \
  CUA_HOME=/tmp/vmd-home OUT=/tmp/vmd VMD_PROFILE=rollout \
  bash bench/spaces-startup/bench.sh vm-density
```

The bench starts `VMD_STEP` VMs at a time through the SDK, which is the path a
trainer takes. All the VMs restore one golden snapshot. After each step it:

1. samples the host with `sample_host.py`;
2. runs 20 exec round trips on every VM at once.

It stops at the first of these:

- a create fails (`create failed`);
- the daemon refuses a VM (`admission refused (HostCapacity)`);
- exec p95 goes over 2x its value with one VM;
- memory pressure turns critical;
- it reaches `VMD_MAX`.

### What `sample_host.py` records

| Field | Meaning |
|---|---|
| `vm_private_mb` | Sum of each QEMU process's own memory (macOS `phys_footprint`, Linux `Anonymous` + `Swap`): the guest's writes and QEMU itself |
| `vm_shared_mb` | Resident part of each snapshot RAM file the VMs map, counted once |
| `vm_rss_mb` | Private + shared: what the VMs use together, which is what admission counts |
| `vm_rss_sum_mb` | The plain RSS sum, for comparison. It counts shared pages once per VM |
| `host.available_mb`, `host.pressure` | The kernel's view (macOS `kern.memorystatus_level` and pressure level; Linux `MemAvailable` and PSI) |

The summary also extrapolates to 64 GB and 128 GB Macs (`fit_by_memory`). The
formula is the default VM memory cap (0.8 of physical memory) less the
shared snapshot memory, divided by the median private memory per VM.

## Baselines and the guard

`baseline/*.json` holds committed runs, one per host type, with thresholds:

- `min_running_ratio`: fail when the most VMs falls under that share of the
  baseline;
- `max_private_ratio`: fail when the median private memory per VM grows past
  that multiple;
- `max_exec_ratio`: fail when exec p95 with one VM grows past that multiple.

`ci-vm-density.yml` runs the bench nightly on a KVM runner and checks it
against `baseline/linux-kvm-gha-ubuntu-24.04.json`. Hosted runners can't
virtualize on macOS, so run the bench and guard by hand on a Mac against
`baseline/macos-*.json`.
