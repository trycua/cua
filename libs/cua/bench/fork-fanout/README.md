# Fork fan-out of a live QEMU checkpoint

Many forks of one live world, the shape of RL rollouts: a bake VM dirties a
few hundred MB of RAM, is checkpointed and pinned, and the checkpoint is
forked in waves (every fork of a wave started at once). Forks map the
checkpoint's RAM file copy-on-write, so they share every page they do not
write.

```sh
DISK=<linux Spaces disk> WAVES=8,32,64 bash run.sh out.json
python3 check.py out.json baseline.json --host macos-hvf
```

| File | What it does |
|---|---|
| `run.sh` | Builds and runs the opt-in test `cua-daemon/tests/qemu_fork_fanout.rs` (release), which also checks every fork: it holds the world's RAM and disk state, its own writes reach no other fork, and it does not hold a private copy of the world's RAM. Writes the numbers as JSON. |
| `check.py` | The perf guard: fails when a metric is above 1.5x its baseline (plus a small absolute slack). |
| `baseline.json` | Committed numbers per host class, from real runs (see `source` in each entry). |
| `test_check.py` | Unit tests of `check.py`. |

Metrics per wave: `fork_ms_p50`/`_p95` (fork plus start until the fork's
cua-spacesd answers with its own token), `wall_s` (the whole wave),
`private_mb_per_fork` (host memory the wave added, divided by its forks:
anonymous, wired and compressed pages, which is what each fork's QEMU process
and its copy-on-write copies cost) and `shared_file_mb` (page cache the wave
added, counted once). On macOS, `footprint` does not see guest pages under
HVF, so per-process numbers (`process_*`) show only QEMU's own memory; the
host-wide numbers are the ones to read. Run on an otherwise idle host.

`.github/workflows/nightly-qemu-fork.yml` runs it nightly on Linux (KVM) and
on the self-hosted Mac, and on pull requests that touch the QEMU snapshot
code.
