"""Cua jobs runtime: the `cua` module that train.py files import.

Pure Python >= 3.10, standard library only.  It has two jobs:

* Describe a job.  ``App.function(...)`` stores a JSON-able spec that the CLI
  reads (``python3 -m _cua_entry --spec FILE FN``) without importing heavy
  libraries, so module-level code in a job file must stay light.
* Tell a running job where it is.  ``Cluster`` holds the gang's primitives
  (rank, world size, attempt, peer addresses, run id, data_dir).  Before your
  function runs, ``_cua_entry`` exports the standard launcher environment
  (MASTER_ADDR/MASTER_PORT/RANK/WORLD_SIZE/NODE_RANK/LOCAL_RANK and an MLX
  hostfile with MLX_HOSTFILE/MLX_RANK), restores data_dir from the run's store
  and keeps it synced, so any framework works unchanged.

Optional sugar lives in separate modules: ``cua.checkpoint`` (atomic
checkpoint dirs, newest checkpoint) and ``cua.progress`` (explicit progress
reports; without them the runtime counts output and CPU time as progress).
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import urllib.parse
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

__all__ = ["App", "Image", "Retries", "Secret", "Cluster", "Function", "KINDS", "NETWORKS",
           "parse_timeout", "parse_nodes", "check_store_url"]

# On Linux hosts with an NVIDIA accelerator these requirement names are
# rewritten (the CLI applies this table and records it in the job env).
CUDA_REWRITES = {"mlx": "mlx[cuda]"}
# Added when a rewrite applies: the CUDA runtime headers NVRTC compiles MLX's
# kernels against (without them MLX falls back to /usr/local/cuda, whose
# headers may be a newer CUDA than the wheel's NVRTC understands).
CUDA_EXTRAS = ["nvidia-cuda-runtime-cu12"]
# Space kinds a job member can run in ("" = the engine picks: a dedicated
# machine runs in its own Space, a Mac in a macOS VM, a shared Linux host in
# a container).
KINDS = ("vm", "container", "microvm", "machine")
# How ranks reach each other: "mesh" (default; each rank's own CUA_PEERS port
# is routed between machines, enough for MLX's ring and torch's rendezvous) or
# "direct" (every port between the placed host addresses, which NCCL/Gloo
# need; only in a dedicated machine's own Space, e.g. `cua cloud add aws` hosts).
NETWORKS = ("mesh", "direct")
# With an s3:// store these are copied from the submitting shell when set
# (otherwise the host's instance role is used).
AWS_ENV = ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION",
           "AWS_DEFAULT_REGION", "AWS_ENDPOINT_URL_S3")


# --------------------------------------------------------------------------
# Spec helpers
# --------------------------------------------------------------------------
_UNITS = {"s": 1, "m": 60, "h": 3600, "d": 86400}


def parse_timeout(value: Any) -> int | None:
    """'2h' / '30m' / '1h30m' / '90s' / 3600 / '3600' -> seconds."""
    if value is None:
        return None
    if isinstance(value, bool):
        raise ValueError("timeout must be a duration, not a bool")
    if isinstance(value, (int, float)):
        secs = int(value)
    else:
        s = str(value).strip().lower().replace(" ", "")
        if re.fullmatch(r"\d+", s):
            secs = int(s)
        elif re.fullmatch(r"(\d+[smhd])+", s):
            secs = sum(int(n) * _UNITS[u] for n, u in re.findall(r"(\d+)([smhd])", s))
        else:
            raise ValueError(f"bad timeout {value!r}: use e.g. '2h', '30m', '1h30m' or seconds")
    if secs <= 0:
        raise ValueError("timeout must be positive")
    return secs


def parse_nodes(value: Any) -> dict[str, int]:
    """int, (min, max), [min, max] or 'min:max' -> {"min": a, "max": b}."""
    if value is None:
        lo = hi = 1
    elif isinstance(value, int) and not isinstance(value, bool):
        lo = hi = value
    elif isinstance(value, str):
        parts = value.split(":")
        if len(parts) == 1:
            lo = hi = int(parts[0])
        elif len(parts) == 2:
            lo, hi = int(parts[0]), int(parts[1])
        else:
            raise ValueError(f"bad nodes {value!r}")
    else:
        try:
            lo, hi = (int(x) for x in value)
        except (TypeError, ValueError):
            raise ValueError(f"nodes must be an int or (min, max), got {value!r}") from None
    if lo < 1 or hi < lo:
        raise ValueError(f"nodes must satisfy 1 <= min <= max, got {lo}:{hi}")
    return {"min": lo, "max": hi}


def check_store_url(url: str | None) -> str:
    """Validate a store URL without touching it ('' = no shared store)."""
    if not url:
        return ""
    u = urllib.parse.urlsplit(str(url))
    if u.scheme == "s3" and u.netloc:
        return str(url)
    if u.scheme == "file" and u.path.startswith("/") and u.netloc in ("", "localhost"):
        return str(url)
    raise ValueError(f"store must be 's3://bucket/prefix' or 'file:///abs/path', got {url!r}")


class Retries:
    """Retry policy for code failures (a member exited non-zero or hit its own
    timeout).  Lost computers, daemon restarts and start-barrier timeouts do not
    use these retries; the pool has a separate budget for them.

    The delay before retry n (1-based) is a random value in
    [0, min(max_delay, initial_delay * backoff_coefficient ** (n - 1))] (full jitter)."""

    def __init__(self, max_retries: int = 3, backoff_coefficient: float = 2.0,
                 initial_delay: float = 2.0, max_delay: float = 60.0):
        if max_retries < 0:
            raise ValueError("max_retries must be >= 0")
        if backoff_coefficient < 1.0:
            raise ValueError("backoff_coefficient must be >= 1")
        if initial_delay < 0 or max_delay < initial_delay:
            raise ValueError("need 0 <= initial_delay <= max_delay")
        self.max_retries = int(max_retries)
        self.backoff_coefficient = float(backoff_coefficient)
        self.initial_delay = float(initial_delay)
        self.max_delay = float(max_delay)

    def to_spec(self) -> dict[str, Any]:
        return {"max_retries": self.max_retries, "backoff_coefficient": self.backoff_coefficient,
                "initial_delay": self.initial_delay, "max_delay": self.max_delay}


class Secret:
    """Names of local env vars the CLI copies into the job environment."""

    def __init__(self, names: Iterable[str]):
        self.names = list(names)

    @classmethod
    def from_local_env(cls, *names: str) -> "Secret":
        return cls(names)

    def to_spec(self) -> list[str]:
        return list(self.names)


class Image:
    """Python version plus pip requirements, installed by uv in the Space."""

    def __init__(self, python: str = "3.12", packages: Iterable[str] = ()):
        self.python_version = str(python)
        self.packages = list(packages)

    @classmethod
    def python(cls, version: str = "3.12") -> "Image":
        return cls(version)

    def uv_pip_install(self, *packages: str) -> "Image":
        flat: list[str] = []
        for p in packages:
            flat.extend(p if isinstance(p, (list, tuple)) else [p])
        return Image(self.python_version, self.packages + flat)

    pip_install = uv_pip_install

    def to_spec(self) -> dict[str, Any]:
        return {
            "python": self.python_version,
            "packages": list(self.packages),
            "cuda_rewrites": dict(CUDA_REWRITES),
            "cuda_extras": list(CUDA_EXTRAS),
        }

    def resolved_packages(self, linux_nvidia: bool = False) -> list[str]:
        """The requirement list for a host (mlx -> mlx[cuda] on Linux + NVIDIA)."""
        if not linux_nvidia:
            return list(self.packages)
        out = []
        rewritten = False
        for req in self.packages:
            m = re.match(r"^([A-Za-z0-9_.\-]+)(.*)$", req)
            if m and m.group(1).lower() in CUDA_REWRITES:
                out.append(CUDA_REWRITES[m.group(1).lower()] + m.group(2))
                rewritten = True
            else:
                out.append(req)
        if rewritten:
            names = {re.split(r"[^A-Za-z0-9_.\-]", r, maxsplit=1)[0].lower() for r in out}
            out += [x for x in CUDA_EXTRAS if x not in names]
        return out


class Function:
    """A registered job function; calling it runs the wrapped function."""

    def __init__(self, fn: Callable, spec: dict[str, Any]):
        self.fn = fn
        self.spec = spec
        self.__name__ = fn.__name__
        self.__doc__ = fn.__doc__
        self.__wrapped__ = fn

    def __call__(self, *args, **kwargs):
        return self.fn(*args, **kwargs)


class App:
    def __init__(self, name: str):
        self.name = name
        self.functions: dict[str, Function] = {}

    def function(self, image: Image | None = None, nodes: Any = 1, accelerator: str | None = None,
                 kind: str | None = None, store: str | None = None, network: str = "mesh",
                 on: str = "auto", retries: Any = 3, timeout: Any = "2h",
                 env: dict[str, str] | None = None, secrets: Iterable[Secret] | None = None,
                 barrier_timeout: Any = "15m", stall_timeout: Any = "10m", infra_retries: int = 10):
        """Register a job function.  Every member (rank) runs in its own Space.

        nodes: 2, or (min, max): the gang size; the pool places as many as fit.
        kind: the Space each member runs in: "vm", "container", "microvm" or
            "machine" (a dedicated host's own Space).  Default: the engine picks.
        store: where data_dir lives between attempts and computers
            ("s3://bucket/prefix" or "file:///mnt/shared/path").  Without it
            data_dir stays on each host and survives retries on that host only.
            With s3://, the AWS_* credentials set in your shell go with the
            job (or the host's instance role is used).
        network: "mesh" (default) routes each rank's own port between
            computers (MLX ring, torch rendezvous); "direct" opens every port
            between the hosts' own addresses, which NCCL/Gloo need (dedicated
            computers in one network only, e.g. `cua cloud add aws` hosts).
        barrier_timeout: how long members wait for the whole gang to be ready.
        stall_timeout: end the attempt as stalled when a member shows no
            progress (output, CPU time, or cua.progress) for this long; None
            disables it.
        infra_retries: budget for lost computers / never-started / barrier
            timeouts (refills after a member runs 10 minutes)."""
        if isinstance(retries, Retries):
            retries_spec = retries.to_spec()
        else:
            retries_spec = Retries(int(retries)).to_spec()
        if accelerator not in (None, "", "gpu", "apple", "nvidia"):
            raise ValueError(f"accelerator must be gpu, apple or nvidia, got {accelerator!r}")
        if kind not in (None, "", *KINDS):
            raise ValueError(f"kind must be one of {', '.join(KINDS)} (or omitted), got {kind!r}")
        if network not in NETWORKS:
            raise ValueError(f"network must be 'mesh' or 'direct', got {network!r}")
        if not (on in ("auto", "aws") or str(on).startswith("host:")):
            raise ValueError(f"on must be 'auto', 'aws' or 'host:NAME', got {on!r}")
        secret_names: list[str] = []
        for s in secrets or []:
            secret_names.extend(s.to_spec())
        parsed_nodes = parse_nodes(nodes)
        timeout_secs = parse_timeout(timeout)
        env_spec = {str(k): str(v) for k, v in (env or {}).items()}
        image_spec = (image or Image.python()).to_spec()
        store_url = check_store_url(store)
        if store_url:
            env_spec.setdefault("CUA_CHECKPOINT_URL", store_url)
            if store_url.startswith("s3://"):
                secret_names += [n for n in AWS_ENV if os.environ.get(n) and n not in secret_names]
        if network == "direct":
            env_spec["CUA_PEER_ROUTE"] = "direct"
        barrier_secs = parse_timeout(barrier_timeout)
        stall_secs = parse_timeout(stall_timeout) if stall_timeout not in (None, 0, "0") else 0
        if int(infra_retries) < 0:
            raise ValueError("infra_retries must be >= 0")

        def deco(fn: Callable) -> Function:
            spec = {
                "app": self.name,
                "name": fn.__name__,
                "image": image_spec,
                "nodes": parsed_nodes,
                "accelerator": accelerator or "",
                "kind": kind or "",
                "store": store_url,
                "network": network,
                "on": on,
                "retries": retries_spec,
                "timeout_secs": timeout_secs,
                "env": env_spec,
                "secrets": secret_names,
                "barrier_timeout_secs": barrier_secs,
                "stall_timeout_secs": stall_secs,
                "infra_retries": int(infra_retries),
            }
            f = Function(fn, spec)
            self.functions[fn.__name__] = f
            return f

        return deco


# --------------------------------------------------------------------------
# Cluster: the gang's primitives
# --------------------------------------------------------------------------
class Cluster:
    """Where this member is in the gang.

    rank, world_size  this member's rank (0 = main) and the gang size
    attempt           1 for the first try; +1 each time the gang is restarted
    run_id            stable across attempts
    peers             "ip:port" of every rank (each rank's MLX ring port)
    ips               the peers' addresses, by rank
    master_addr       rank 0's address (torch.distributed / accelerate)
    master_port       the rendezvous port on rank 0 (its own CUA_PEERS port:
                      the one port the pool routes to it)
    data_dir          persistent: the newest snapshot of the run's data_dir
                      is restored here before your function runs (with
                      store=), so a restarted gang continues from it
    """

    def __init__(self, rank: int = 0, world_size: int = 1, peers: Sequence[str] | None = None,
                 data_dir: str | os.PathLike | None = None, attempt: int = 1, run_id: str = ""):
        self.rank = int(rank)
        self.world_size = int(world_size)
        self.peers = list(peers or [])
        self.attempt = int(attempt)
        self.run_id = run_id
        self.data_dir = Path(data_dir) if data_dir else Path(".cua-job-data").resolve()
        if self.world_size < 1 or not 0 <= self.rank < self.world_size:
            raise ValueError(f"bad rank {self.rank} for world size {self.world_size}")
        if self.world_size > 1 and len(self.peers) != self.world_size:
            raise ValueError(f"CUA_PEERS has {len(self.peers)} entries for world size {self.world_size}")
        self.ips = [p.rpartition(":")[0] for p in self.peers]
        if self.peers:
            host, _, port = self.peers[0].rpartition(":")
            self.master_addr, self.master_port = host, int(port)
        else:
            self.master_addr, self.master_port = "127.0.0.1", 29500

    @classmethod
    def from_env(cls, env: dict[str, str] | None = None) -> "Cluster":
        e = os.environ if env is None else env
        peers = [p.strip() for p in e.get("CUA_PEERS", "").split(",") if p.strip()]
        world = int(e.get("CUA_WORLD_SIZE", "1") or 1)
        return cls(
            rank=int(e.get("CUA_RANK", "0") or 0),
            world_size=world,
            peers=peers if (world > 1 or len(peers) == 1) else [],
            data_dir=e.get("CUA_JOB_DATA") or None,
            attempt=int(e.get("CUA_ATTEMPT", "1") or 1),
            run_id=e.get("CUA_RUN_ID", ""),
        )

    def __repr__(self) -> str:
        return (f"Cluster(rank={self.rank}, world_size={self.world_size}, attempt={self.attempt}, "
                f"run_id={self.run_id!r}, master={self.master_addr}:{self.master_port}, "
                f"data_dir={str(self.data_dir)!r})")

    # ---- launch environment ---------------------------------------------
    def _mlx_hostfile(self) -> list[list[str]]:
        """MLX ring hostfile: one entry per rank, each a list of the "ip:port"
        strings that rank listens on (the structure `mlx.launch --backend ring`
        writes)."""
        return [[p] for p in self.peers]

    def _launch_env(self) -> dict[str, str]:
        """The standard variables launchers read (exported before your
        function runs): torchrun / accelerate / torch.distributed env://, and
        MLX_RANK for mx.distributed (MLX_HOSTFILE is added by the runtime once
        the hostfile is written)."""
        env = {
            "MASTER_ADDR": self.master_addr, "MASTER_PORT": str(self.master_port),
            "RANK": str(self.rank), "WORLD_SIZE": str(self.world_size),
            "NODE_RANK": str(self.rank), "LOCAL_RANK": "0", "LOCAL_WORLD_SIZE": "1",
        }
        if self.world_size > 1:
            env["MLX_RANK"] = str(self.rank)
        return env

    # ---- conveniences: fill in the flags, run your command ---------------
    def mlx_launch(self, argv: Sequence[str], env: dict[str, str] | None = None) -> None:
        """Run an MLX program as this rank, e.g.
        ``cluster.mlx_launch(["mlx_lm.lora", "--model", ..., "--train"])``.

        Like ``mlx.launch --backend ring`` for one rank: the program inherits
        MLX_HOSTFILE and MLX_RANK, so ``mx.distributed.init()`` joins the
        gang's ring (world size 1: no hostfile, a single-process group).  A
        ``.py`` file runs with this Python.  Raises SystemExit when it fails."""
        cmd = self._argv(argv)
        self._run(cmd, env, Path(argv[0]).name)

    def _torchrun_flags(self, nproc_per_node: int = 1) -> list[str]:
        """The flags `torchrun` needs for this gang (static rendezvous on rank 0)."""
        return ["--nnodes", str(self.world_size), "--node_rank", str(self.rank),
                "--nproc_per_node", str(nproc_per_node),
                "--master_addr", self.master_addr, "--master_port", str(self.master_port)]

    def torchrun(self, argv: Sequence[str], nproc_per_node: int = 1,
                 env: dict[str, str] | None = None) -> None:
        """``torchrun <flags for this gang> *argv``, e.g.
        ``cluster.torchrun(["train_ddp.py", "--epochs", "3"])``.
        Raises SystemExit when it fails."""
        cmd = [sys.executable, "-m", "torch.distributed.run", *self._torchrun_flags(nproc_per_node), *argv]
        self._run(cmd, env, "torchrun")

    @staticmethod
    def _argv(argv: Sequence[str]) -> list[str]:
        argv = [str(a) for a in argv]
        if not argv:
            raise ValueError("empty command")
        if argv[0].endswith(".py"):
            return [sys.executable, *argv]
        if os.sep not in argv[0] and not shutil.which(argv[0]):
            # Console scripts of the job's environment sit next to its Python.
            local = Path(sys.executable).parent / argv[0]
            if local.exists():
                argv[0] = str(local)
        return argv

    @staticmethod
    def _run(cmd: list[str], env: dict[str, str] | None, name: str) -> None:
        sys.stdout.flush()
        sys.stderr.flush()
        p = subprocess.Popen(cmd, env={**os.environ, **(env or {})})
        try:
            rc = p.wait()
        except BaseException:
            # The member is being stopped (SIGTERM, Ctrl-C) or failed: stop
            # the command too.  torchrun / mlx.launch pass SIGTERM on to their
            # workers, which may sit in process groups of their own.
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.wait()
            raise
        if rc != 0:
            raise SystemExit(f"cua: {name} exited with status {rc}")
