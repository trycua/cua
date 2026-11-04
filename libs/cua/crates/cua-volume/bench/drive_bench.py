#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
"""Cua Volume benchmark: streaming through the mount versus downloading first,
sync latency between two devices, and listing a 10k-file folder.

Reproducible on a Mac with Docker (colima or Docker Desktop) and ffmpeg:

    cargo build --release -p cua-volume --features s3,nfs --example drive_bench
    python3 crates/cua-volume/bench/drive_bench.py --work ~/scratch/drive-bench

What it does, all under --work (removed afterwards unless --keep):

1. Generates a 4K H.264 test video of about 1 GB with ffmpeg (moov atom at
   the end, as cameras write it) and a 10 GB one by concatenating it
   (stream copy, no re-encode). --sizes picks which.
2. Starts MinIO (versioned bucket) and a toxiproxy in front of it, both in
   Docker and bound to 127.0.0.1, and removes them at the end. The proxy
   adds --latency-ms each way and caps each connection at
   --conn-mbps MB/s, to look like a real cloud bucket.
3. For each store (the local fs backend, MinIO direct, MinIO through the
   proxy) it runs a separate cua home's drive worker (examples/drive_bench),
   uploads the videos, mounts the drive over NFS, and measures, each from a
   cold start (block cache cleared and the volume remounted, so neither the
   drive's cache nor the kernel's holds anything):
     ffprobe; decode one frame at 50% (a seek); decode 10 s from the middle;
     sequential throughput; random 4 KiB read latency p50/p95; the block
     cache hit rate during the 10 s decode.
   Then the download-first baseline: download the whole file (8 parallel
   ranged GETs, like `aws s3 cp`) and run the same ffprobe and seek locally.
4. Sync: two independent cua homes on one bucket. A writes; B's feed event
   and B's mount (stat) are timed. Small (1 KiB) and large (64 MiB) files.
5. Scale: 10,000 files in one folder, listed through the API and through
   the mount (cold and warm).

Memory is bounded: the worker streams, test files are the only large
allocations on disk, and every container has a memory cap.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import shutil
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
WORKSPACE = HERE.parents[2]  # libs/cua
WORKER = WORKSPACE / "target" / "release" / "examples" / "drive_bench"
MINIO_IMAGE = os.environ.get("MINIO_IMAGE", "cgr.dev/chainguard/minio:latest")
TOXI_IMAGE = os.environ.get("TOXIPROXY_IMAGE", "ghcr.io/shopify/toxiproxy:2.12.0")
BUCKET = "cua-volume-bench"
MIB = 1024 * 1024


def log(msg: str) -> None:
    print(f"[bench] {msg}", flush=True)


def sh(*cmd: str, check: bool = True, capture: bool = True) -> str:
    r = subprocess.run(cmd, check=check, capture_output=capture, text=True)
    return r.stdout if capture else ""


def timed(*cmd: str) -> float:
    t0 = time.perf_counter()
    subprocess.run(cmd, check=True, capture_output=True)
    return time.perf_counter() - t0


def pct(xs: list[float], p: float) -> float:
    xs = sorted(xs)
    k = max(0, min(len(xs) - 1, round(p / 100 * (len(xs) - 1))))
    return xs[k]


KEYS: dict[str, str] = {}


class Worker:
    """One cua home's drive (examples/drive_bench), one JSON line per call."""

    def __init__(self, home: Path, env: dict[str, str]):
        env = {**KEYS, **env}
        home.mkdir(parents=True, exist_ok=True)
        e = dict(os.environ)
        e.update(env)
        e["CUA_HOME"] = str(home)
        e["CUA_TEST"] = "1"
        e.pop("CUA_DRIVE_BACKEND", None)
        self.p = subprocess.Popen(
            [str(WORKER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=e
        )
        ready = json.loads(self.p.stdout.readline())
        self.device = ready["device"]

    def call(self, **cmd) -> dict:
        self.p.stdin.write(json.dumps(cmd) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        r = json.loads(line)
        if not r.get("ok"):
            raise RuntimeError(f"{cmd['cmd']}: {r.get('error')}")
        return r

    def close(self) -> None:
        try:
            self.call(cmd="unmount")
        except Exception:
            pass
        try:
            self.p.stdin.write('{"cmd":"quit"}\n')
            self.p.stdin.flush()
            self.p.wait(timeout=120)
        except Exception:
            self.p.kill()


class Docker:
    """MinIO and toxiproxy on a private network, loopback ports only."""

    def __init__(self, work: Path, latency_ms: int, conn_mbps: int):
        self.tag = f"cua-volume-bench-{os.getpid()}"
        try:
            self._start(latency_ms, conn_mbps)
        except BaseException:
            self.close()
            raise

    def _start(self, latency_ms: int, conn_mbps: int) -> None:
        self.net = self.tag
        self.key = "bench" + os.urandom(4).hex()
        self.secret = os.urandom(16).hex()
        sh("docker", "network", "create", self.net)
        sh("docker", "volume", "create", self.tag)
        sh("docker", "run", "-d", "--name", f"{self.tag}-minio", "--network", self.net,
           "--network-alias", "minio", "--memory=1g", "--memory-swap=1g",
           "--user", "0:0", "-p", "127.0.0.1::9000", "-v", f"{self.tag}:/data",
           "-e", f"MINIO_ROOT_USER={self.key}", "-e", f"MINIO_ROOT_PASSWORD={self.secret}",
           MINIO_IMAGE, "server", "/data")
        sh("docker", "run", "-d", "--name", f"{self.tag}-toxi", "--network", self.net,
           "--memory=256m", "-p", "127.0.0.1::8474", "-p", "127.0.0.1::9001", TOXI_IMAGE)
        port = lambda c, p: sh("docker", "port", c, p).split(":")[-1].strip()
        self.minio = f"http://127.0.0.1:{port(f'{self.tag}-minio', '9000/tcp')}"
        self.toxi_api = f"http://127.0.0.1:{port(f'{self.tag}-toxi', '8474/tcp')}"
        self.proxied = f"http://127.0.0.1:{port(f'{self.tag}-toxi', '9001/tcp')}"
        for _ in range(60):
            try:
                urllib.request.urlopen(f"{self.minio}/minio/health/ready", timeout=2)
                break
            except Exception:
                time.sleep(1)
        for _ in range(30):
            try:
                urllib.request.urlopen(f"{self.toxi_api}/version", timeout=2)
                break
            except Exception:
                time.sleep(1)
        self._toxi("/proxies", {"name": "s3", "listen": "0.0.0.0:9001", "upstream": "minio:9000"})
        for stream in ("upstream", "downstream"):
            self._toxi("/proxies/s3/toxics", {"name": f"lat-{stream}", "type": "latency",
                                             "stream": stream, "attributes": {"latency": latency_ms}})
        self._toxi("/proxies/s3/toxics", {"name": "bw", "type": "bandwidth", "stream": "downstream",
                                         "attributes": {"rate": conn_mbps * 1024}})
        self._bucket()
        KEYS["CUA_DRIVE_S3_ACCESS_KEY_ID"] = self.key
        KEYS["CUA_DRIVE_S3_SECRET_ACCESS_KEY"] = self.secret

    def _toxi(self, path: str, body: dict) -> None:
        req = urllib.request.Request(self.toxi_api + path, data=json.dumps(body).encode(),
                                     headers={"Content-Type": "application/json"}, method="POST")
        urllib.request.urlopen(req, timeout=10)

    def _bucket(self) -> None:
        # A throwaway mc on the private network makes the versioned bucket.
        mc = ("docker", "run", "--rm", "--network", self.net, "-e",
              f"MC_HOST_b=http://{self.key}:{self.secret}@minio:9000",
              "cgr.dev/chainguard/minio-client:latest")
        sh(*mc, "mb", "-p", f"b/{BUCKET}")
        sh(*mc, "version", "enable", f"b/{BUCKET}")

    def close(self) -> None:
        sh("docker", "rm", "-f", f"{self.tag}-minio", f"{self.tag}-toxi", check=False)
        sh("docker", "network", "rm", self.net, check=False)
        sh("docker", "volume", "rm", "-f", self.tag, check=False)


def free_port() -> int:
    import socket
    with socket.socket() as so:
        so.bind(("127.0.0.1", 0))
        return so.getsockname()[1]


def wait_url(url: str, tries: int = 60) -> None:
    for _ in range(tries):
        try:
            urllib.request.urlopen(url, timeout=2)
            return
        except urllib.error.HTTPError:
            return
        except Exception:
            time.sleep(0.5)
    raise RuntimeError(f"{url} never answered")


class Local:
    """MinIO and toxiproxy as local binaries bound to 127.0.0.1 (no VM port
    forwarding in the data path). Same interface as Docker."""

    def __init__(self, work: Path, bins: Path, latency_ms: int, conn_mbps: int):
        self.procs: list[subprocess.Popen] = []
        self.data = work / "minio-data"
        self.key = "bench" + os.urandom(4).hex()
        self.secret = os.urandom(16).hex()
        try:
            self._start(bins, latency_ms, conn_mbps)
        except BaseException:
            self.close()
            raise

    def _start(self, bins: Path, latency_ms: int, conn_mbps: int) -> None:
        self.data.mkdir(parents=True, exist_ok=True)
        mp, cp, tp, pp = free_port(), free_port(), free_port(), free_port()
        env = dict(os.environ, MINIO_ROOT_USER=self.key, MINIO_ROOT_PASSWORD=self.secret,
                   MINIO_BROWSER="off")
        self.procs.append(subprocess.Popen(
            [str(bins / "minio"), "server", "--quiet", "--address", f"127.0.0.1:{mp}",
             "--console-address", f"127.0.0.1:{cp}", str(self.data)],
            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        self.procs.append(subprocess.Popen(
            [str(bins / "toxiproxy-server"), "-host", "127.0.0.1", "-port", str(tp)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        self.minio = f"http://127.0.0.1:{mp}"
        self.toxi_api = f"http://127.0.0.1:{tp}"
        self.proxied = f"http://127.0.0.1:{pp}"
        wait_url(f"{self.minio}/minio/health/ready")
        wait_url(f"{self.toxi_api}/version")
        Docker._toxi(self, "/proxies", {"name": "s3", "listen": f"127.0.0.1:{pp}",
                                        "upstream": f"127.0.0.1:{mp}"})
        for stream in ("upstream", "downstream"):
            Docker._toxi(self, "/proxies/s3/toxics", {"name": f"lat-{stream}", "type": "latency",
                                                     "stream": stream, "attributes": {"latency": latency_ms}})
        Docker._toxi(self, "/proxies/s3/toxics", {"name": "bw", "type": "bandwidth", "stream": "downstream",
                                                 "attributes": {"rate": conn_mbps * 1024}})
        mc_env = dict(os.environ, MC_HOST_b=f"http://{self.key}:{self.secret}@127.0.0.1:{mp}",
                      MC_CONFIG_DIR=str(self.data.parent / "mc-config"))
        for args in (["mb", "-p", f"b/{BUCKET}"], ["version", "enable", f"b/{BUCKET}"]):
            subprocess.run([str(bins / "mc"), *args], env=mc_env, check=True, capture_output=True)
        KEYS["CUA_DRIVE_S3_ACCESS_KEY_ID"] = self.key
        KEYS["CUA_DRIVE_S3_SECRET_ACCESS_KEY"] = self.secret

    def close(self) -> None:
        for p in self.procs:
            p.terminate()
        for p in self.procs:
            try:
                p.wait(timeout=20)
            except Exception:
                p.kill()
        shutil.rmtree(self.data, ignore_errors=True)
        shutil.rmtree(self.data.parent / "mc-config", ignore_errors=True)


def make_videos(work: Path, sizes: list[int]) -> dict[int, Path]:
    out: dict[int, Path] = {}
    one = work / "video-1g.mp4"
    if not one.exists():
        log("encoding a ~1 GB 4K H.264 test video (ffmpeg, ultrafast)")
        # 150 Mb/s for 56 s is about 1 GiB; the moov atom stays at the end.
        sh("ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
           "testsrc2=size=3840x2160:rate=30", "-t", "56", "-c:v", "libx264",
           "-preset", "ultrafast", "-b:v", "150M", "-maxrate", "150M", "-bufsize", "300M",
           "-g", "30", "-pix_fmt", "yuv420p", str(one))
    if 1 in sizes:
        out[1] = one
    if 10 in sizes:
        ten = work / "video-10g.mp4"
        if not ten.exists():
            log("building the ~10 GB video (10 x the 1 GB one, stream copy)")
            lst = work / "concat.txt"
            lst.write_text("".join(f"file '{one}'\n" for _ in range(10)))
            sh("ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(lst),
               "-c", "copy", str(ten))
        out[10] = ten
    return out


def duration(path: Path) -> float:
    return float(sh("ffprobe", "-v", "error", "-show_entries", "format=duration",
                    "-of", "csv=p=0", str(path)).strip())


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    # Under $HOME so Docker engines that only share the home directory (colima)
    # can bind-mount the MinIO data; CUA_DRIVE_BENCH_WORK or --work overrides.
    default_work = os.environ.get("CUA_DRIVE_BENCH_WORK") or (
        Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache") / "cua" / "drive-bench"
    )
    ap.add_argument("--work", type=Path, default=Path(default_work))
    ap.add_argument("--sizes", default="1,10", help="video sizes in GB: 1, 10 or both")
    ap.add_argument("--stores", default="fs,s3,s3-latency")
    ap.add_argument("--latency-ms", type=int, default=25, help="each way, through the proxy")
    ap.add_argument("--conn-mbps", type=int, default=50, help="MB/s per connection, through the proxy")
    ap.add_argument("--random-samples", type=int, default=200)
    ap.add_argument("--sync-rounds", type=int, default=20)
    ap.add_argument("--files", type=int, default=10000)
    ap.add_argument("--skip", default="", help="comma list of: stream, sync, scale")
    ap.add_argument("--keep", action="store_true", help="keep the work dir (videos, data)")
    ap.add_argument("--out", type=Path, default=None, help="write results JSON here")
    ap.add_argument("--bin-dir", type=Path, default=None,
                    help="run minio, mc and toxiproxy-server from here on 127.0.0.1 instead of Docker")
    a = ap.parse_args()
    if not WORKER.exists():
        print(f"build the worker first: cargo build --release -p cua-volume --features s3,nfs "
              f"--example drive_bench (expected {WORKER})", file=sys.stderr)
        return 2
    sizes = [int(x) for x in a.sizes.split(",") if x]
    stores = [s for s in a.stores.split(",") if s]
    skip = set(a.skip.split(","))
    a.work.mkdir(parents=True, exist_ok=True)
    results: dict = {"config": {"latency_ms_each_way": a.latency_ms, "conn_mbps": a.conn_mbps,
                                "sizes_gb": sizes, "stores": stores},
                     "stream": {}, "sync": {}, "scale": {}}
    docker = None
    workers: list[Worker] = []
    try:
        if any(s.startswith("s3") for s in stores) or "sync" not in skip or "scale" not in skip:
            if a.bin_dir:
                log(f"starting MinIO and toxiproxy from {a.bin_dir} (127.0.0.1 only)")
                docker = Local(a.work, a.bin_dir, a.latency_ms, a.conn_mbps)
            else:
                log("starting MinIO and toxiproxy (Docker, loopback only)")
                docker = Docker(a.work, a.latency_ms, a.conn_mbps)
            results["config"]["s3_server"] = "local binaries" if a.bin_dir else "docker"
        if "stream" not in skip:
            videos = make_videos(a.work, sizes)
            results["videos"] = {str(k): {"bytes": v.stat().st_size, "seconds": duration(v)}
                                 for k, v in videos.items()}
            for store in stores:
                results["stream"][store] = stream_bench(a, store, videos, docker, workers)
        if "sync" not in skip and docker:
            results["sync"] = sync_bench(a, docker, workers)
        if "scale" not in skip:
            for store in ("fs", "s3"):
                if store == "s3" and not docker:
                    continue
                results["scale"][store] = scale_bench(a, store, docker, workers)
    finally:
        for w in workers:
            w.close()
        if docker:
            docker.close()
        if not a.keep:
            for p in a.work.glob("*"):
                if p.name.startswith("mnt"):
                    continue
                if p.is_dir():
                    shutil.rmtree(p, ignore_errors=True)
                else:
                    p.unlink(missing_ok=True)
    text = json.dumps(results, indent=2)
    print(text)
    if a.out:
        a.out.write_text(text)
    print(table(results))
    return 0


def storage(w: Worker, store: str, docker: Docker | None, root: str) -> None:
    if store == "fs":
        w.call(cmd="storage", backend="fs")
        return
    endpoint = docker.proxied if store == "s3-latency" else docker.minio
    # Keys reach the worker through its environment (it has no credential
    # store), so the storage call carries none.
    w.call(cmd="storage", backend="s3", endpoint=endpoint, bucket=BUCKET, root=root)


def remount(w: Worker, mnt: Path) -> Path:
    w.call(cmd="unmount")
    w.call(cmd="cache_clear")
    st = w.call(cmd="mount", path=str(mnt))["out"]
    if st["state"] != "mounted":
        raise RuntimeError(f"mount: {st}")
    return Path(st["path"])


def stream_bench(a, store: str, videos: dict[int, Path], docker, workers) -> dict:
    log(f"== streaming: {store}")
    home = a.work / f"home-{store}"
    w = Worker(home, {})
    workers.append(w)
    # s3 and s3-latency share one root: upload once (direct), read both ways.
    storage(w, store, docker, "stream/")
    mnt = a.work / f"mnt-{store}" / "Cua Volume"
    out = {}
    for gb, video in videos.items():
        key = f"public/media/{video.name}"
        if store != "s3-latency":
            t = w.call(cmd="put", key=key, file=str(video))["secs"]
            out[f"{gb}g_upload_s"] = t
        dur = duration(video)
        size = video.stat().st_size
        r: dict = {"bytes": size}
        path = remount(w, mnt) / "public" / "media" / video.name
        r["ffprobe_s"] = timed("ffprobe", "-v", "error", "-show_format", "-show_streams", str(path))
        path = remount(w, mnt) / "public" / "media" / video.name
        r["seek_frame_50pct_s"] = timed("ffmpeg", "-v", "error", "-ss", f"{dur / 2:.3f}", "-i",
                                        str(path), "-frames:v", "1", "-f", "null", "-")
        path = remount(w, mnt) / "public" / "media" / video.name
        before = w.call(cmd="cache_stats")["out"]
        r["decode_10s_mid_s"] = timed("ffmpeg", "-v", "error", "-ss", f"{dur / 2:.3f}", "-t", "10",
                                      "-i", str(path), "-f", "null", "-")
        after = w.call(cmd="cache_stats")["out"]
        hits, misses = after["hits"] - before["hits"], after["misses"] - before["misses"]
        r["cache_hit_rate_decode"] = round(hits / (hits + misses), 4) if hits + misses else None
        path = remount(w, mnt) / "public" / "media" / video.name
        want = min(size, 2 * 1024 * MIB)
        t0 = time.perf_counter()
        got = 0
        with open(path, "rb", buffering=0) as f:
            while got < want:
                b = f.read(8 * MIB)
                if not b:
                    break
                got += len(b)
        r["seq_mb_per_s"] = round(got / MIB / (time.perf_counter() - t0), 1)
        path = remount(w, mnt) / "public" / "media" / video.name
        rnd = random.Random(7)
        lat = []
        fd = os.open(path, os.O_RDONLY)
        try:
            for _ in range(a.random_samples):
                off = rnd.randrange(0, size - 4096) // 4096 * 4096
                t0 = time.perf_counter()
                os.pread(fd, 4096, off)
                lat.append((time.perf_counter() - t0) * 1000)
        finally:
            os.close(fd)
        r["rand4k_p50_ms"] = round(pct(lat, 50), 2)
        r["rand4k_p95_ms"] = round(pct(lat, 95), 2)
        # Download-first baseline.
        w.call(cmd="unmount")
        local = a.work / f"download-{store}-{gb}g.mp4"
        r["download_s"] = w.call(cmd="download", key=key, out=str(local), parallel=8)["secs"]
        r["download_then_ffprobe_s"] = r["download_s"] + timed(
            "ffprobe", "-v", "error", "-show_format", "-show_streams", str(local))
        r["download_then_seek_frame_s"] = r["download_s"] + timed(
            "ffmpeg", "-v", "error", "-ss", f"{dur / 2:.3f}", "-i", str(local), "-frames:v", "1",
            "-f", "null", "-")
        local.unlink()
        out[f"{gb}g"] = r
        log(f"{store} {gb} GB: {json.dumps(r)}")
    w.close()
    workers.remove(w)
    if store == "fs":
        shutil.rmtree(home, ignore_errors=True)
    return out


def sync_bench(a, docker: Docker, workers) -> dict:
    log("== sync: two cua homes on one bucket")
    wa = Worker(a.work / "sync-a", {})
    wb = Worker(a.work / "sync-b", {})
    workers += [wa, wb]
    for w in (wa, wb):
        storage(w, "s3", docker, "sync/")
    mnt = remount(wb, a.work / "mnt-sync" / "Cua Volume")
    out = {}
    since = wb.call(cmd="events", since=0, wait_ms=0)["out"]["next"]
    for label, size in (("small_1k", 1024), ("large_64m", 64 * MIB)):
        api, mount = [], []
        for i in range(a.sync_rounds):
            key = f"public/sync/{label}-{i}.bin"
            done = wa.call(cmd="write", key=key, size=size)["t_ms"]
            # B's feed event (the API view).
            while True:
                ev = wb.call(cmd="events", since=since, wait_ms=15000)["out"]
                since = ev["next"]
                hit = [e for e in ev["events"] if e["path"] == key and e["kind"] == "remote_change"]
                if hit:
                    api.append(hit[0]["ts_ms"] - done)
                    break
            # B's mount (a program polling stat, as Finder does).
            p = mnt / "public" / "sync" / f"{label}-{i}.bin"
            while True:
                try:
                    if os.stat(p).st_size == size:
                        mount.append(time.time() * 1000 - done)
                        break
                except FileNotFoundError:
                    pass
                time.sleep(0.02)
        out[label] = {"api_p50_ms": round(pct(api, 50)), "api_p95_ms": round(pct(api, 95)),
                      "mount_p50_ms": round(pct(mount, 50)), "mount_p95_ms": round(pct(mount, 95)),
                      "rounds": a.sync_rounds}
        log(f"sync {label}: {out[label]}")
    for w in (wa, wb):
        w.close()
        workers.remove(w)
    return out


def scale_bench(a, store: str, docker, workers) -> dict:
    log(f"== scale: {a.files} files in one folder ({store})")
    w = Worker(a.work / f"scale-{store}", {})
    workers.append(w)
    storage(w, store, docker, "scale/")
    t_fill = w.call(cmd="fill", folder="public/many/", count=a.files)["secs"]
    # Let the change feed finish publishing the fill (it writes one small
    # object per file) so the listing is timed alone.
    for _ in range(600):
        if w.call(cmd="feed_published")["out"]["published"] >= a.files:
            break
        time.sleep(0.5)
    api = w.call(cmd="ls", folder="public/many/")
    assert api["out"]["entries"] == a.files, api
    api2 = w.call(cmd="ls", folder="public/many/")
    mnt = remount(w, a.work / f"mnt-scale-{store}" / "Cua Volume")
    t0 = time.perf_counter()
    n = len(os.listdir(mnt / "public" / "many"))
    cold = time.perf_counter() - t0
    t0 = time.perf_counter()
    os.listdir(mnt / "public" / "many")
    warm = time.perf_counter() - t0
    assert n == a.files, n
    r = {"fill_s": round(t_fill, 2), "api_ls_s": round(api["secs"], 3),
         "api_ls_again_s": round(api2["secs"], 3),
         "mount_ls_cold_s": round(cold, 3), "mount_ls_warm_s": round(warm, 3)}
    log(f"scale {store}: {r}")
    w.close()
    workers.remove(w)
    return r


def table(r: dict) -> str:
    rows = ["| Store | Size | ffprobe | Seek to 50% | 10 s decode | Seq MB/s | 4K p50 / p95 ms | Hit rate | Download | Download + ffprobe |",
            "|---|---|---|---|---|---|---|---|---|---|"]
    for store, v in r.get("stream", {}).items():
        for size, m in v.items():
            if not isinstance(m, dict):
                continue
            rows.append(
                f"| {store} | {size} | {m['ffprobe_s']:.2f} s | {m['seek_frame_50pct_s']:.2f} s | "
                f"{m['decode_10s_mid_s']:.2f} s | {m['seq_mb_per_s']} | {m['rand4k_p50_ms']} / "
                f"{m['rand4k_p95_ms']} | {m['cache_hit_rate_decode']} | {m['download_s']:.1f} s | "
                f"{m['download_then_ffprobe_s']:.1f} s |")
    return "\n".join(rows)


if __name__ == "__main__":
    sys.exit(main())
