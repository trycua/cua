# Committed runs

`2026-10-24-m2max-96g/`: the output of `cua bench run microvm-fork --n 3`
with `FANOUT=2` on a Mac Studio (Apple M2 Max, 12 CPUs, 96 GB, macOS 26.6.2),
a source build of `cua` and `cua-vz`, with other VMs running on the host. It
is the converter tests' fixture (`../../test_bench_result.py`), not a
baseline: `../baseline.json` is.
