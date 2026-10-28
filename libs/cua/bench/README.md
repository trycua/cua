# Spaces benchmarks

Each directory here measures one thing about Spaces through the real `cua`
CLI, SDK and daemon, the path a user or an agent takes. Every bench keeps
its own script, committed baselines and perf guard (`check.py`). `cua bench`
runs them and writes every run in one format:

```bash
cua bench list                                   # names, what each measures, host needs
cua bench run microvm-fork --n 5 --out ./mf      # writes ./mf/result.json, samples.csv, raw/
cua bench check ./mf/result.json --baseline microvm-fork/baseline.json
```

| Bench (`cua bench run`) | Directory | Host |
|---|---|---|
| `spaces-startup.microvm`, `.gvisor`, `.minimal`, `.density` | `spaces-startup/` | Linux with KVM (gVisor for `.gvisor`) |
| `spaces-startup.vm-density` | `spaces-startup/` | macOS (HVF) or Linux with KVM |
| `microvm-fork` | `microvm-fork/` | macOS on Apple silicon |
| `fork-fanout` | `fork-fanout/` | macOS (HVF) or Linux with KVM, a source checkout |
| `fork-batch` | `fork-batch/` | macOS (HVF) or Linux with KVM, the Python SDK |
| `channel` | `channel/` | macOS (HVF) or Linux with KVM |
| `rollouts` | `rollouts/` | macOS or Linux with gVisor |
| `pool-sessions` | `pool-sessions/` | your signed-in machines and a test site |

`streaming/` and `pool-chaos/` are run by hand (see their READMEs).

## How `cua bench` finds the scripts

The `cua` binary embeds the scripts, checkers and baselines of the benches
it wraps (`cuabench.py manifest` lists them), so a released binary runs the
scripts of its own version with no checkout. On first use it unpacks them
to `~/.cache/cua/bench/<version>-<hash>/` (`$XDG_CACHE_HOME` when set) and
runs `cuabench.py` from there with `python3`. `CUA_BENCH_DIR=<dir>` runs
another copy instead (a source checkout's `libs/cua/bench`).

Some benches need a program a release does not ship. `cua bench run` stops
with what to set when one is missing:

| Need | Benches | What |
|---|---|---|
| `ROLLOUT_BENCH` | `spaces-startup.vm-density`, `rollouts` | `cargo build --release -p cua-sdk --example rollout_bench` in `libs/cua` |
| `CHANNEL_BENCH` | `channel` | `cargo build --release -p cua-sdk --example channel_bench` in `libs/cua` |
| a source checkout | `fork-fanout` | it builds and runs an integration test with cargo (`--src DIR`, `CUA_SRC`, or run inside the checkout) |
| `DISK` | `fork-fanout`, `fork-batch` | a Linux Spaces disk (`crane export --platform linux/arm64 ghcr.io/trycua/linux:24.04-slim-disk - \| tar -x disk/disk.img`) |
| the Python SDK | `fork-batch` | `pip install cua` |
| `SITE` | `pool-sessions` | the test site `pool-sessions/make-site.py` writes, served where the Spaces reach it |

Build these from the release tag that matches your `cua --version`.

Every run uses a Spaces home of its own (`<out>/cua-home`, or `CUA_HOME`
when set) and a daemon of the same `cua` binary, never an installed app's
daemon or your own Spaces. `pool-sessions` is the exception: it runs on your
signed-in machines. A bench deletes the Spaces and checkpoints it creates.

## The result format (bench-result v1)

`cua bench run` writes, in its `--out` directory:

| File | What |
|---|---|
| `result.json` | the run, in the format below |
| `samples.csv` | one row per sample, header first; the first column is `i` |
| `raw/` | the bench's own output, its log and the daemon's log (`cua bench check` reads it) |

```json
{
  "schema": "cua.bench-result/v1",
  "bench": "microvm-fork",
  "slug": "microvm-fork",
  "metric": "fork_from_checkpoint_ms",
  "value": 610.0,
  "unit": "ms",
  "stat": "p50",
  "n": 5,
  "percentiles": {"p50": 610.0, "p90": 650.2, "p95": 655.1, "p99": 659.0, "min": 590.0, "max": 660.0},
  "extra": {"checkpoint_p50_ms": 650.0, "per_fork_mib": 1620},
  "samples_csv": "samples.csv",
  "measured_at": "2026-10-24T20:11:00Z",
  "build": {"cua": "0.9.0", "source": "cua 0.9.0 (/usr/local/bin/cua)"},
  "host": {"name": "mac", "model": "Mac14,13", "chip": "Apple M2 Max", "cpus": 12, "memory_gb": 96, "os": "macOS 26.6.2", "where": "local"},
  "environment": {"kind": "microvm", "image": "ghcr.io/trycua/linux:24.04-slim", "load_avg_before": 0.4, "other_vms": 0, "network": "n/a"},
  "method": "What is timed from and to, warmup, n, serial or parallel.",
  "reproduce": "cua bench run microvm-fork --n 5",
  "caveats": [],
  "status": "measured"
}
```

| Field | Meaning |
|---|---|
| `schema` | always `cua.bench-result/v1` |
| `bench` | the name `cua bench run` takes |
| `slug` | a stable id for the measured question (no dates or versions) |
| `metric`, `unit` | the primary metric and its unit |
| `value`, `stat` | the headline number and what it is: `p50`, `mean`, `max`, `min`, `count` or `total` |
| `n` | samples behind `value` |
| `percentiles` | `p50`, `p90`, `p95`, `p99`, `min`, `max` of the primary metric, where the bench has them |
| `extra` | other named numbers of the run (numbers only) |
| `samples_csv` | the samples file, next to `result.json` |
| `measured_at` | UTC time the run finished |
| `build` | `cua` version; `source` says which binary (set `CUA_BENCH_SOURCE` to name a release or commit) |
| `host` | the machine: model, chip, CPUs, memory, OS, `where` (`local`) |
| `environment` | Space kind, image, load average before the run, VMs already running, network |
| `method` | what is timed from and to, what is excluded, n, serial or parallel |
| `reproduce` | the command that repeats the run |
| `caveats` | what a reader must know to read the number right |
| `status` | `measured`; `tbd` files carry no value |

A result exists only for a run that measured something: when a bench's
output has no successful sample, no `result.json` is written.
`bench_result.py` holds the converters from each bench's own output, and
`test_bench_result.py` tests them on the committed baselines and runs.

## Perf guards

`cua bench check RESULT --baseline FILE [--host CLASS]` runs the bench's own
checker on the run's `raw/` output against a committed baseline (a path, or
one relative to this directory) and exits 1 on a regression. `--host` picks
the host class or profile in baselines that hold several (`fork-fanout`,
`fork-batch`). The monthly workflow `.github/workflows/bench-monthly.yml`
runs the macOS set on the self-hosted Mac and fails on any regression.
