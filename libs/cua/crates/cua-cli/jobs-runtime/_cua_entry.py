"""Entry point for cua jobs (pure Python, standard library only).

    python3 -m _cua_entry --spec FILE FN    print the function spec as JSON
    python  -m _cua_entry --launch FILE FN  pick this host's packages, then run
    python  -m _cua_entry FILE FN           run FN as this member of the gang

Launch mode (what `cua run` starts on every host) reads CUA_JOB_PYTHON and
CUA_JOB_PACKAGES (a JSON list), applies the CUDA rewrites (mlx -> mlx[cuda])
when this is a Linux host with an NVIDIA GPU, and execs
`uv run --no-project --python PY --with PKG... python -m _cua_entry FILE FN`.

Run mode, before the function runs:
  * exports MASTER_ADDR/MASTER_PORT/RANK/WORLD_SIZE/NODE_RANK/LOCAL_RANK and,
    for a gang, an MLX ring hostfile with MLX_HOSTFILE/MLX_RANK;
  * with a store (CUA_CHECKPOINT_URL, from `store=`), makes data_dir exactly
    the run's newest committed snapshot (rank 0 first claims the run for this
    attempt, which fences older attempts out of the store);
  * joins the start barrier (CUA_BARRIER);
  * feeds the stall watchdog (CUA_HEARTBEAT) from output and CPU time.
While it runs, rank 0 syncs data_dir to the store every CUA_SYNC_SECS (30)
and once more when it ends (also on failure or SIGTERM).  Rank 0's return
value goes into the file named by CUA_RESULT_PATH as a gzip tar: a returned
file or directory under its own name, bytes as result.bin, anything else as
result.json; `cua run` unpacks it into ./cua-runs/<run>/.
"""
from __future__ import annotations

import importlib.util
import inspect
import json
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import traceback
from pathlib import Path


_SYSTEM_CUDA = re.compile(r"^/usr/local/cuda(-[0-9.]+)?(/|$)")


def clean_ld_library_path(value: str) -> str:
    """Drop system CUDA toolkit dirs from an LD_LIBRARY_PATH value.

    GPU images (e.g. the AWS Deep Learning AMI) export their own CUDA/cuDNN in
    LD_LIBRARY_PATH.  They shadow the CUDA libraries shipped inside the
    `mlx[cuda]` wheels, and MLX then fails (CUDNN_STATUS_SUBLIBRARY_LOADING_FAILED
    or NVRTC header errors).  Set CUA_KEEP_LD_LIBRARY_PATH=1 to opt out."""
    return ":".join(p for p in value.split(":") if p and not _SYSTEM_CUDA.match(p))


def maybe_reexec_clean_ld() -> None:
    """glibc reads LD_LIBRARY_PATH at process start, so fix it by re-exec."""
    ld = os.environ.get("LD_LIBRARY_PATH", "")
    if (not sys.platform.startswith("linux") or not ld
            or os.environ.get("CUA_KEEP_LD_LIBRARY_PATH") or os.environ.get("_CUA_LD_CLEANED")):
        return
    cleaned = clean_ld_library_path(ld)
    if cleaned == ld:
        return
    os.environ["_CUA_LD_CLEANED"] = "1"
    if cleaned:
        os.environ["LD_LIBRARY_PATH"] = cleaned
    else:
        del os.environ["LD_LIBRARY_PATH"]
    os.execv(sys.executable, [sys.executable, *sys.orig_argv[1:]])


def load(file: str):
    path = Path(file).resolve()
    if not path.is_file():
        raise SystemExit(f"cua: job file not found: {file}")
    sys.path.insert(0, str(path.parent))
    spec = importlib.util.spec_from_file_location(path.stem, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[path.stem] = mod
    spec.loader.exec_module(mod)
    return mod


def find_function(mod, name: str):
    import cua

    fn = getattr(mod, name, None)
    if not isinstance(fn, cua.Function):
        avail = sorted(
            n for n, v in vars(mod).items() if isinstance(v, cua.Function)
        )
        raise SystemExit(
            f"cua: {name!r} is not a @app.function in this file"
            + (f" (available: {', '.join(avail)})" if avail else " (no functions found)")
        )
    return fn


def _result_path(value) -> str | None:
    """The file or directory that goes back to the caller (None: nothing)."""
    if value is None:
        return None
    if isinstance(value, (str, os.PathLike)) and Path(value).exists():
        return str(Path(value).resolve())
    out = Path(tempfile.mkdtemp(prefix="cua-result-"))
    if isinstance(value, (bytes, bytearray)):
        out = out / "result.bin"
        out.write_bytes(bytes(value))
    else:
        out = out / "result.json"
        out.write_text(json.dumps(value))
    return str(out)


def has_nvidia_gpu() -> bool:
    """A Linux host with a working NVIDIA driver (`nvidia-smi -L` lists a GPU)."""
    if not sys.platform.startswith("linux") or not shutil.which("nvidia-smi"):
        return False
    try:
        out = subprocess.run(["nvidia-smi", "-L"], capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return False
    return out.returncode == 0 and "GPU" in out.stdout


def launch_argv(file: str, name: str, env: dict[str, str], nvidia: bool) -> list[str]:
    """The `uv run` command line of this member on this host."""
    import cua

    python = env.get("CUA_JOB_PYTHON") or "3.12"
    packages = json.loads(env.get("CUA_JOB_PACKAGES") or "[]")
    image = cua.Image(python, packages)
    argv = ["uv", "run", "--no-project", "--python", python]
    for p in image.resolved_packages(linux_nvidia=nvidia):
        argv += ["--with", p]
    return argv + ["python", "-m", "_cua_entry", file, name]


def launch(file: str, name: str) -> int:
    nvidia = has_nvidia_gpu()
    argv = launch_argv(file, name, dict(os.environ), nvidia)
    uv = shutil.which("uv")
    if not uv:
        print("cua: uv is not on PATH here (the job's Space provides it)", file=sys.stderr)
        return 127
    pkgs = [a for i, a in enumerate(argv) if i > 0 and argv[i - 1] == "--with"]
    print(f"cua: python {argv[4]}, packages {' '.join(pkgs) or '(none)'}"
          + (" (NVIDIA GPU: CUDA builds)" if nvidia else ""), flush=True)
    os.execv(uv, argv)
    return 0  # not reached


def write_result(value, result_to: str) -> None:
    """Rank 0's result as a gzip tar (the file or directory under its own name)."""
    path = _result_path(value)
    if not path:
        return
    tmp = result_to + ".tmp"
    with tarfile.open(tmp, "w:gz") as tar:
        tar.add(path, arcname=Path(path).name)
    os.replace(tmp, result_to)


# --------------------------------------------------------------------------
# Liveness: output and CPU time count as progress
# --------------------------------------------------------------------------
_ANSI_RE = re.compile(rb"\x1b\[[0-9;]*[A-Za-z]")
_STEP_RE = re.compile(rb"(?i)\b(?:iter|iteration|step|epoch)\s*[:#=]?\s*(\d+)")


def _parse_cputime(s: str) -> float:
    """ps TIME ("[[dd-]hh:]mm:ss[.ss]") -> seconds."""
    days = 0
    if "-" in s:
        d, s = s.split("-", 1)
        days = int(d)
    secs = 0.0
    for part in s.split(":"):
        secs = secs * 60 + float(part)
    return days * 86400 + secs


def tree_cpu_seconds(root: int | None = None) -> float | None:
    """CPU seconds used by `root` (default: this process) and its descendants
    (Linux: /proc; elsewhere: ps).  None when neither is available."""
    root = os.getpid() if root is None else root
    procs: dict[int, tuple[int, float]] = {}  # pid -> (ppid, cpu seconds)
    if os.path.isdir("/proc/self"):
        tick = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100
        for d in os.listdir("/proc"):
            if not d.isdigit():
                continue
            try:
                with open(f"/proc/{d}/stat", "rb") as f:
                    raw = f.read()
            except OSError:
                continue
            rest = raw[raw.rfind(b")") + 2:].split()
            try:
                procs[int(d)] = (int(rest[1]), (int(rest[11]) + int(rest[12])) / tick)
            except (IndexError, ValueError):
                continue
    else:
        try:
            out = subprocess.run(["ps", "-axo", "pid=,ppid=,time="], capture_output=True, text=True,
                                 timeout=10).stdout
        except (OSError, subprocess.SubprocessError):
            return None
        for line in out.splitlines():
            parts = line.split()
            if len(parts) == 3:
                try:
                    procs[int(parts[0])] = (int(parts[1]), _parse_cputime(parts[2]))
                except ValueError:
                    continue
    if root not in procs:
        return None
    children: dict[int, list[int]] = {}
    for pid, (ppid, _c) in procs.items():
        children.setdefault(ppid, []).append(pid)
    total, todo = 0.0, [root]
    while todo:
        pid = todo.pop()
        total += procs[pid][1]
        todo.extend(c for c in children.get(pid, []) if c != pid)
    return total


class Liveness:
    """Feeds the pool's stall watchdog (the CUA_HEARTBEAT file) so user code
    never has to: new output on stdout/stderr (of this process and every
    child) and growing CPU time of the process tree both count as progress.
    The step shown is the last "step N" / "iter N" seen in the output.  Once
    user code calls cua.progress.step(), only its reports count."""

    def __init__(self, cpu_every: float = 10.0, cpu_min: float = 0.5):
        self.step = 0
        self._written = -1
        self.cpu_every = cpu_every
        self.cpu_min = cpu_min
        self._stop = threading.Event()
        self._threads: list[threading.Thread] = []
        self._orig: dict[int, int] = {}

    def beat(self, force: bool = False) -> None:
        from cua import progress
        if progress.explicit:
            return
        step = self.step
        if progress.write(step, force=force or step != self._written):
            self._written = step

    def _pump(self, r: int, out: int) -> None:
        while True:
            try:
                data = os.read(r, 65536)
            except OSError:
                return
            if not data:
                return
            view = memoryview(data)
            while view:
                try:
                    n = os.write(out, view)
                except OSError:
                    break
                view = view[n:]
            found = _STEP_RE.findall(_ANSI_RE.sub(b"", data))
            if found:
                self.step = int(found[-1])
            self.beat()

    def _cpu(self) -> None:
        last = tree_cpu_seconds()
        while not self._stop.wait(self.cpu_every):
            cur = tree_cpu_seconds()
            if cur is not None and last is not None and cur - last >= self.cpu_min:
                self.beat()
            last = cur if cur is not None else last

    def start(self) -> "Liveness":
        sys.stdout.flush()
        sys.stderr.flush()
        for fd in (1, 2):
            self._orig[fd] = os.dup(fd)
            r, w = os.pipe()
            os.dup2(w, fd)
            os.close(w)
            t = threading.Thread(target=self._pump, args=(r, self._orig[fd]), daemon=True,
                                 name=f"cua-output-{fd}")
            t.start()
            self._threads.append(t)
        threading.Thread(target=self._cpu, daemon=True, name="cua-cpu").start()
        return self

    def stop(self) -> None:
        self._stop.set()
        sys.stdout.flush()
        sys.stderr.flush()
        for fd, orig in self._orig.items():
            os.dup2(orig, fd)  # closes our write end of the pipe
        for t in self._threads:
            t.join(timeout=2.0)  # a child that still holds the pipe keeps it open
        self.beat()


# --------------------------------------------------------------------------
# Start barrier
# --------------------------------------------------------------------------
def wait_barrier(barrier_dir: str | None, rank: int, timeout: float = 900.0) -> None:
    """Report this member ready, then wait until every member of the gang is
    (the host writes `go` into CUA_BARRIER; `abort` ends the attempt).  No-op
    outside a gang barrier."""
    if not barrier_dir:
        return
    bar = Path(barrier_dir)
    bar.mkdir(parents=True, exist_ok=True)
    tmp = bar / f".ready.{os.getpid()}"
    tmp.write_text(json.dumps({"t_ms": int(time.time() * 1000), "rank": rank, "pid": os.getpid()}))
    os.replace(tmp, bar / "ready")
    # The coordinator owns the timeout (it cancels the gang); this local
    # limit only guards against a host that never answers.
    limit = timeout + 60.0
    t0 = time.monotonic()
    said = False
    while True:
        if (bar / "go").exists():
            if said:
                log(f"gang ready after {time.monotonic() - t0:.0f}s")
            return
        if (bar / "abort").exists():
            why = (bar / "abort").read_text().strip() or "the gang did not start"
            raise SystemExit(f"cua: start barrier aborted: {why}")
        if time.monotonic() - t0 > limit:
            raise RuntimeError(f"cua: start barrier: no 'go' from the host after {limit:.0f}s")
        if not said and time.monotonic() - t0 > 5:
            log("waiting for the other members to be ready")
            said = True
        time.sleep(0.1)


def log(msg: str) -> None:
    print(f"cua: {msg}", flush=True)


_KINDS = {
    "vm": "a VM",
    "container": "a container",
    "microvm": "a microVM",
    "machine": "the computer's own Space (a dedicated host)",
}


def space_line(env=os.environ) -> str | None:
    """Where this member runs, from CUA_SPACE_KIND (set by the host): the
    first line a member prints, so the run's log says it.  None outside a
    Space (a local test run)."""
    kind = env.get("CUA_SPACE_KIND", "").strip()
    if not kind:
        return None
    if sys.platform == "darwin":
        system = f"macOS {platform.mac_ver()[0] or platform.release()}"
    else:
        system = f"{platform.system()} {platform.release()}"
    where = _KINDS.get(kind, f"a {kind} Space")
    if kind != "machine":
        where += " Space, fresh for this attempt"
    rank, world = env.get("CUA_RANK", "0"), env.get("CUA_WORLD_SIZE", "1")
    return f"rank {rank}/{world} runs in {where} (kind={kind}, {system})"


# --------------------------------------------------------------------------
# Run one member
# --------------------------------------------------------------------------
def export_launch_env(cluster) -> None:
    """MASTER_ADDR/.../LOCAL_RANK for torch, MLX_HOSTFILE/MLX_RANK for MLX."""
    os.environ.update(cluster._launch_env())
    if cluster.world_size > 1:
        tag = re.sub(r"[^A-Za-z0-9_.-]", "_", cluster.run_id or "local")
        path = Path(tempfile.gettempdir()) / f"cua-mlx-hostfile-{tag}-a{cluster.attempt}-r{cluster.rank}.json"
        tmp = path.with_name(path.name + ".tmp")
        tmp.write_text(json.dumps(cluster._mlx_hostfile()))
        os.replace(tmp, path)
        os.environ["MLX_HOSTFILE"] = str(path)


class _Syncer:
    """Rank 0: sync data_dir to the store every `every` seconds in the background."""

    def __init__(self, sync, every: float):
        self.sync = sync
        self.every = every
        self._stop = threading.Event()
        self._t = threading.Thread(target=self._loop, daemon=True, name="cua-sync")

    def _loop(self) -> None:
        while not self._stop.wait(self.every):
            self._once(final=False)

    def _once(self, final: bool) -> None:
        try:
            path = self.sync.sync(final=final)
            if path:
                log(f"data_dir synced ({path})")
        except Exception as e:  # the store is down: keep training, retry next time
            log(f"data_dir sync failed: {e}")

    def start(self) -> "_Syncer":
        self._t.start()
        return self

    def stop(self) -> None:
        self._stop.set()
        self._t.join()
        self._once(final=True)


def _on_sigterm(signum, _frame):
    raise SystemExit(128 + signum)


def run(fn) -> int:
    import cua
    from cua import _store

    cluster = cua.Cluster.from_env()
    where = space_line()
    if where:
        log(where)
    cluster.data_dir.mkdir(parents=True, exist_ok=True)
    live = Liveness().start() if os.environ.get("CUA_HEARTBEAT") else None
    syncer = None
    try:
        export_launch_env(cluster)
        url = os.environ.get("CUA_CHECKPOINT_URL", "")
        sync = None
        if url:
            store = _store.CheckpointStore(url, cluster.run_id, cluster.attempt)
            sync = _store.DataDirSync(store, cluster.data_dir, cluster.rank, cluster.world_size, log=log)
            try:
                sync.restore()
            except _store.StaleAttempt as e:
                log(f"this attempt was superseded, stopping: {e}")
                return 1
        # Start barrier: this member's environment and data_dir are ready;
        # wait until every member's is (no-op outside a gang barrier).
        wait_barrier(os.environ.get("CUA_BARRIER"), cluster.rank,
                     float(os.environ.get("CUA_BARRIER_TIMEOUT", "900") or 900))
        if live:
            live.beat(force=True)
        if sync is not None and cluster.rank == 0:
            syncer = _Syncer(sync, float(os.environ.get("CUA_SYNC_SECS", "30") or 30)).start()
        signal.signal(signal.SIGTERM, _on_sigterm)
        sig = inspect.signature(fn.fn)
        try:
            value = fn.fn(cluster) if sig.parameters else fn.fn()
        finally:
            if syncer is not None:
                syncer.stop()
        result_to = os.environ.get("CUA_RESULT_PATH")
        if cluster.rank == 0 and result_to:
            write_result(value, result_to)
        return 0
    finally:
        if live:
            live.stop()


def main(argv: list[str]) -> int:
    if argv and argv[0] == "--launch":
        if len(argv) != 3:
            print(__doc__, file=sys.stderr)
            return 2
        return launch(argv[1], argv[2])
    spec_mode = bool(argv) and argv[0] == "--spec"
    args = argv[1:] if spec_mode else argv
    if len(args) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    file, name = args
    fn = find_function(load(file), name)
    if spec_mode:
        spec = dict(fn.spec)
        spec["file"] = Path(file).name
        spec["function"] = name
        print(json.dumps(spec, sort_keys=True))
        return 0

    maybe_reexec_clean_ld()
    # The decorator's env (e.g. CUA_CHECKPOINT_URL from store=) also applies
    # when the job runs without the CLI; values already set win.
    for k, v in (fn.spec.get("env") or {}).items():
        os.environ.setdefault(k, v)
    return run(fn)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except SystemExit:
        raise
    except BaseException:
        traceback.print_exc()
        sys.exit(1)
