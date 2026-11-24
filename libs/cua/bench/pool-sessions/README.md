# Browser sessions on all your machines

Short browser sessions at a high rate across all your machines (`cua sb run --on auto`): every session gets a fresh Space, keeps
a headless Chromium open for `SESSION_S` seconds opening a new page about every 10 s, and the Space is
deleted when it ends. This is the workload behind the fleet benchmark's numbers.

| File | What it does |
|---|---|
| `make-site.py OUTDIR` | Downloads ten real article pages (Wikipedia, CC BY-SA) with their stylesheet and images once, and writes a self-contained test site (`p0.html` .. `p9.html`) so sessions never load third-party sites. |
| `session.sh` | One session inside a Space; prints one JSON line (`pages`, `rendered`, ...). Serve it as `s.sh` on the test site. |
| `run.sh` | Submits sessions (`cua sb run --on auto --detach`) in batches. |
| `sample.sh` | Appends the pool coordinator's counters and per-computer load every 15 s (`cua machines --json`). |
| `node-stats.sh` | On a host: CPU busy, load, memory used and running containers every 5 s. |
| `window.py` | Throughput, concurrency and retries over a window of `sample.sh` output. |
| `analyze.py` | Per run: states, Space start/run/delete times, and (sampled with `SAMPLE=N`) the sessions' own reports. |

Run it:

```bash
python3 make-site.py ./site && cp session.sh ./site/s.sh   # serve ./site (e.g. nginx) where Spaces reach it
bash sample.sh 4200 ./samples.jsonl &
SITE=http://<site address> bash run.sh container gvisor 40000 3000 0.5 1024 180 ghcr.io/trycua/linux:24.04-slim
python3 window.py ./samples.jsonl <start unix ts> <end unix ts>
```

Hosts that overcommit reservations (`CUA_HOST_JOBS_OVERCOMMIT`) take more sessions than their memory
limits add up to, and refuse new ones while their available memory nears the floor.
