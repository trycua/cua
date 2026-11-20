# Channel benchmark

How fast a coordinator (an RL trainer, an agent loop) talks to its Spaces
through the SDK: a call that runs nothing, `exec` round trips, file upload
and download, and a process's streamed stdout. `examples/channel_bench`
measures one Space; `check.py` compares a run with a committed baseline.

```bash
cd libs/cua
cargo build --release -p cua-cli -p cua-sdk --example channel_bench
# Same host: a Linux VM Space on this machine, created, measured, deleted.
CUA=target/release/cua BENCH=target/release/examples/channel_bench \
  FILES=1:20,4:20,16:10,100:10,512:5 \
  bash bench/channel/run.sh bench/channel/baselines/macos-m2max-qemu.json
# A Space one of your other machines provides (LAN, over the mesh):
cua spaces create linux --on host:<machine> --kind vm --name lan1
BENCH=target/release/examples/channel_bench \
  bash bench/channel/run.sh bench/channel/baselines/aws-lan-mesh-c7g.json lan1
```

The run fails when a latency (p50, p95) is above 1.5x its baseline, a
throughput below baseline / 1.5, a baseline's absolute target is missed, or
the run took another route than the baseline's (`route_prefix`: a LAN run
that fell back to the relay fails). `RECORD=1` (or a missing baseline)
writes the run as a new baseline to commit.

| Metric | What |
|---|---|
| `rpc_health_ms` | `Health`: the transport's round trip alone |
| `exec_run_ms`, `exec_sh_ms` | run `true` directly, or through `/bin/sh` |
| `upload_<n>MiB_ms`, `download_<n>MiB_ms` | one file each way (`MiBps_p50`) |
| `stream_stdout` | MiB/s of a process's stdout, and bytes skipped (cua-spacesd keeps a bounded scrollback and never waits for a slow reader) |

The summary line also carries the route the calls took (`local …`,
`relay …`, `mesh direct <ip:port> rtt <ms> ms`, `mesh relayed`), the same
string `Space.route()` returns.

## The bulk lane (same machine, Linux VM)

A Linux VM Space on the machine running the coordinator shares a second
disk (`bulk.img` in the VM's directory, virtio-blk serial `cuabulk`) with its
host. A file of 2 MiB or more moves through it instead of the guest's
user-mode network, which tops out at about 250-370 MiB/s on QEMU/HVF: the
guest stages (or takes) the bytes in a slot of the disk, the client reads
(or writes) the same slot through the file, and the transfer is checked
against the SHA-256 the guest computed. The client uses the lane only after
the guest proved it is the same disk (the identity in the disk's first block
equals the one the guest advertises), and any doubt takes the network path.
`CUA_BULK_LANE=off` turns it off on either side. The guest end ships with
cua-spacesd and the image's `71-cua-bulk-lane.rules` (gives the image user
the device); an image without them simply never offers the lane.

## Baselines

| File | Where |
|---|---|
| `ci-ubuntu-24.04-kvm-lane.json` | GitHub `ubuntu-24.04` runner, QEMU/KVM, same host, bulk lane (the nightly `Perf: cua channel` workflow) |
| `macos-m2max-qemu.json` | Mac Studio M2 Max, QEMU/HVF Linux VM Space, same host |
| `aws-lan-mesh-c7g.json` | Two AWS machines in one subnet (a datacenter LAN, not a wired office LAN), a pool Space over the mesh's direct path |

Each file's `setup` says how it was measured.
