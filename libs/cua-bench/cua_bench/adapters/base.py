"""``BenchAdapter``: one base class for bringing an upstream benchmark to cua-bench.

An adapter is still an ordinary cua-bench task (``tasks/<bench>/main.py``):
it declares the benchmark once and ``register()`` turns it into the usual
``@cb.tasks_config`` / ``@cb.setup_task`` / ``@cb.evaluate_task`` /
``@cb.solve_task`` functions, so ``cb run``, ``cb interact``, the runner and
harnesses that import task modules see nothing new::

    import cua_bench as cb
    from cua_bench.adapters import BenchAdapter

    class MyBench(BenchAdapter):
        id, version = "mybench", "1.0"
        image = "ghcr.io/trycua/bench-mybench@sha256:..."   # one index, all variants
        kinds = ("container", "vm")
        requires = frozenset({"egress"})

        def load_tasks(self, split):
            return [cb.Task(description=..., metadata={...}) for ...]

        async def setup(self, task, session, ep): ...
        async def evaluate(self, task, session, ep): return 1.0

    MyBench().register(globals())

What the base class does for every adapter:

* the environment: each task's ``computer`` becomes ``{"provider": "native",
  "setup_config": {"os_type", "image", "kinds", "requires",
  "server_port", ...}}`` from the class attributes, so image, variant and
  kind selection (``--kind auto|container|vm``, VM-only OSes) and the
  requirement check happen in ``cb run`` before any sandbox starts;
* :class:`Endpoints` (``ep``): the benchmark's own server and other guest
  ports, the same way locally and on Fleet;
* timeouts: setup, evaluate and the oracle are bounded (``timeouts``).
"""

from __future__ import annotations

import asyncio
import inspect
from dataclasses import dataclass
from typing import Any, Literal, Mapping, Optional

import cua_bench as cb

Score = Any  # a number, a bool, a list of numbers or a dict (see runner.episode.reward_of)


@dataclass(frozen=True)
class ServerSpec:
    """The benchmark's own server inside the guest (its controller API).

    ``port`` is exposed and used as the sandbox readiness probe, and is
    reachable as the ``server`` service (``await ep.url()``).
    """

    port: int
    protocol: Literal["http", "https", "tcp"] = "http"
    health: str = "/"


class Endpoints:
    """How an adapter reaches guest ports, the same locally and on Fleet.

    * ``await ep.url()`` / ``await ep.url("server")``: the benchmark server;
    * ``await ep.url(9222)``: any exposed guest port;
    * ``await ep.request("server", "POST", "/reset", json=...)``: an HTTP
      request through the SDK's service API;
    * ``await ep.host_port(9222)``: a ``(host, port)`` for code that builds
      ``http://{host}:{port}/...`` itself (upstream controllers). Locally that
      is the published port; when the service URL has a path prefix (Fleet
      signed URLs) it is a loopback :class:`~cua_bench.adapters.bridge.PathBridge`
      to it, closed by ``await ep.aclose()``.

    Sessions attached to a legacy computer-server by URL (no cua-sandbox
    sandbox) resolve ports on the same host.
    """

    def __init__(self, session: Any, server: Optional[ServerSpec] = None) -> None:
        self.session = session
        self.server = server
        self._bridges: dict = {}

    @property
    def sandbox(self) -> Any:
        return getattr(self.session, "sandbox", None)

    async def url(self, target: "int | str" = "server") -> str:
        sandbox = self.sandbox
        if isinstance(target, str):
            if sandbox is None:
                if target == "server" and self.server is not None:
                    return self._host_url(self.server.port)
                raise LookupError(f"no sandbox to resolve service {target!r}")
            return await sandbox.service(target).url()
        port = int(target)
        if sandbox is None:
            return self._host_url(port)
        if self.server is not None and port == self.server.port:
            return await sandbox.service("server").url()
        mapped = (getattr(sandbox, "exposed_ports", None) or {}).get(port)
        if mapped:
            return f"http://127.0.0.1:{mapped}"
        return await sandbox.service(f"port-{port}").url()

    def _host_url(self, port: int) -> str:
        host = getattr(self.session, "_api_host", None) or "127.0.0.1"
        scheme = self.server.protocol if self.server and port == self.server.port else "http"
        return f"{scheme}://{host}:{port}"

    async def host_port(self, target: "int | str" = "server") -> tuple[str, int]:
        """``(host, port)`` reaching ``target`` with a plain ``http://host:port`` URL."""
        from urllib.parse import urlsplit

        url = await self.url(target)
        u = urlsplit(url)
        if u.scheme == "http" and u.hostname and u.port and u.path in ("", "/") and not u.query:
            return u.hostname, int(u.port)
        bridge = self._bridges.get(target)
        if bridge is None or bridge.url != url:
            from .bridge import PathBridge

            if bridge is not None:
                await bridge.aclose()
            bridge = self._bridges[target] = PathBridge(url)
        return await bridge.start()

    async def aclose(self) -> None:
        """Stop the bridges :meth:`host_port` started."""
        bridges, self._bridges = list(self._bridges.values()), {}
        for bridge in bridges:
            await bridge.aclose()

    async def request(self, service: str, method: str, path: str, **kwargs: Any) -> Any:
        sandbox = self.sandbox
        if sandbox is None:
            import httpx

            async with httpx.AsyncClient(timeout=kwargs.pop("timeout", 60)) as client:
                return await client.request(method, (await self.url(service)) + path, **kwargs)
        return await sandbox.service(service).request(method, path, **kwargs)


#: Requirement words ``cb run`` checks before any sandbox starts.
REQUIREMENT_ALIASES = {"openai": "env:OPENAI_API_KEY", "hf-gated": "env:HF_TOKEN"}


class BenchAdapter:
    """Base class for benchmark adapters (see the module docstring)."""

    #: Short benchmark id and the upstream version this adapter follows.
    id: str = ""
    version: str = ""
    #: "native": each task runs in a sandbox of ``image``. "dataset": no
    #: environment (static datasets such as grounding sets): the task's
    #: session is a :class:`~cua_bench.computers.dataset.DatasetSession`.
    provider: Literal["native", "dataset"] = "native"
    #: The image index (``ghcr.io/trycua/bench-<id>@sha256:...``); the resolver
    #: picks the rootfs or containerDisk variant. None: the canonical image.
    image: Optional[str] = None
    os_type: str = "linux"
    #: Kinds the benchmark supports (a requirement): ("vm",) for Windows,
    #: macOS or anything that needs a kernel.
    kinds: tuple[str, ...] = ("container", "vm")
    #: kvm (local VMs need hardware virtualization), egress, openai, hf-gated,
    #: env:NAME (an environment variable that must be set).
    requires: frozenset = frozenset()
    server: Optional[ServerSpec] = None
    #: Other guest ports the adapter reaches (exposed with the server port).
    ports: tuple[int, ...] = ()
    #: How a task starts clean: a fresh sandbox (default), the benchmark's own
    #: reset endpoint, or a snapshot restore inside the guest.
    reset: Literal["fresh-claim", "server", "snapshot"] = "fresh-claim"
    #: driver: agents act through cua-driver (sb.mouse/keyboard); native: the
    #: adapter translates cb actions into the benchmark's API (``act``).
    action_mode: Literal["driver", "native", "both"] = "driver"
    width: int = 1920
    height: int = 1080
    #: Seconds per phase (None: no limit). Override the whole mapping to change it.
    timeouts: Mapping[str, Optional[float]] = {
        "setup": 1800.0,
        "evaluate": 1800.0,
        "oracle": 3600.0,
    }

    # ── what an adapter implements ────────────────────────────────────────

    def load_tasks(self, split: str) -> list[cb.Task]:
        """The benchmark's tasks for ``split`` (``computer`` is filled in)."""
        raise NotImplementedError

    async def setup(self, task: cb.Task, session: Any, ep: Endpoints) -> None:
        """Bring the guest to the task's start state."""

    async def evaluate(self, task: cb.Task, session: Any, ep: Endpoints) -> Score:
        """Score the task (a number in [0, 1], or a list/dict of them)."""
        raise NotImplementedError

    async def act(self, action: Any, session: Any, ep: Endpoints) -> None:
        """``action_mode="native"``: execute a cb action through the benchmark's API."""
        raise NotImplementedError(f"{self.id} has no native action mode")

    #: Override to provide a reference solution (``--oracle``). Leave as None
    #: when the benchmark has none: cb run then evaluates the untouched state.
    oracle: Any = None

    # ── provided ──────────────────────────────────────────────────────────

    def environment(self) -> dict:
        """The ``computer`` declaration every task of this adapter carries."""
        setup: dict[str, Any] = {
            "os_type": self.os_type,
            "width": self.width,
            "height": self.height,
            "requires": sorted(self.requires),
        }
        if self.provider == "dataset":
            return {"provider": "dataset", "setup_config": setup}
        setup["kinds"] = list(self.kinds)
        if self.image:
            setup["image"] = self.image
        if self.server is not None:
            setup["server_port"] = self.server.port
        if self.ports:
            setup["ports"] = list(self.ports)
        return {"provider": "native", "setup_config": setup}

    def tasks(self, split: str = "train") -> list[cb.Task]:
        env = self.environment()
        out = []
        for index, task in enumerate(self.load_tasks(split)):
            computer = dict(env)
            declared = dict(getattr(task, "computer", None) or {})
            setup = {**env["setup_config"], **dict(declared.get("setup_config") or {})}
            # The adapter owns the environment; a task may only narrow it.
            setup["os_type"] = self.os_type
            if self.provider != "dataset":
                setup["kinds"] = list(self.kinds)
            computer["setup_config"] = setup
            task.computer = computer
            metadata = dict(task.metadata or {})
            metadata.setdefault("benchmark", {"id": self.id, "version": self.version})
            task.metadata = metadata
            if task.task_id is None:
                task.task_id = f"{self.id}-{index}"
            out.append(task)
        return out

    def endpoints(self, session: Any) -> Endpoints:
        """The session's Endpoints (one per session, so bridges are reused
        from setup to evaluate; closed after evaluate)."""
        ep = getattr(session, "_cb_endpoints", None)
        if ep is None or ep.server is not self.server:
            ep = Endpoints(session, self.server)
            try:
                session._cb_endpoints = ep
            except Exception:  # noqa: BLE001 - sessions without attributes
                pass
        return ep

    #: Seconds to wait for the benchmark server's health path before setup.
    #: Sandbox readiness only proves the port accepts connections; the
    #: server may still be starting what it fronts (a browser, a desktop).
    health_timeout: float = 300.0

    async def _wait_healthy(self, ep: "Endpoints") -> None:
        """Poll ``server.health`` until it answers 2xx (bounded)."""
        server = self.server
        sandbox = getattr(ep, "sandbox", None)
        # Only sandboxes with the service API (cua-sandbox); attached legacy
        # sessions and test fakes have none to probe.
        if server is None or server.protocol == "tcp" or not callable(getattr(sandbox, "service", None)):
            return
        method, _, path = server.health.partition(" ") if " " in server.health else ("GET", "", server.health)
        deadline = asyncio.get_running_loop().time() + self.health_timeout
        last: Any = None
        while True:
            try:
                r = await ep.request("server", method or "GET", path or "/", timeout=20)
                last = getattr(r, "status_code", None)
                if last is not None and 200 <= last < 300:
                    return
            except Exception as error:  # noqa: BLE001 - still starting
                last = repr(error)[:200]
            if asyncio.get_running_loop().time() > deadline:
                raise TimeoutError(
                    f"{self.id}: server :{server.port}{path} not healthy after {self.health_timeout:.0f}s ({last})"
                )
            await asyncio.sleep(2)

    async def _bounded(self, phase: str, coro: Any) -> Any:
        limit = self.timeouts.get(phase)
        return await (asyncio.wait_for(coro, limit) if limit else coro)

    def register(self, namespace: dict, *, split: str = "train") -> "BenchAdapter":
        """Define the cua-bench task functions in a task module's ``globals()``."""
        adapter = self

        @cb.tasks_config(split=split)
        def load():
            return adapter.tasks(split)

        @cb.setup_task(split=split)
        async def start(task_cfg, session):
            ep = adapter.endpoints(session)
            await adapter._bounded("setup", adapter._wait_healthy(ep))
            await adapter._bounded("setup", adapter.setup(task_cfg, session, ep))

        @cb.evaluate_task(split=split)
        async def evaluate(task_cfg, session):
            ep = adapter.endpoints(session)
            try:
                return await adapter._bounded("evaluate", adapter.evaluate(task_cfg, session, ep))
            finally:
                await ep.aclose()

        namespace.update(load=load, start=start, evaluate=evaluate)
        if adapter.oracle is not None and inspect.iscoroutinefunction(adapter.oracle):

            @cb.solve_task(split=split)
            async def solve(task_cfg, session):
                await adapter._bounded(
                    "oracle", adapter.oracle(task_cfg, session, adapter.endpoints(session))
                )

            namespace["solve"] = solve
        namespace["ADAPTER"] = adapter
        return adapter


def unmet_requirements(
    requires: "list[str] | tuple[str, ...] | frozenset",
    *,
    cloud: bool,
    environ: Mapping[str, str],
    has_kvm: bool,
) -> list[str]:
    """Human-readable problems for ``requires`` on a target (pure; facts injected)."""
    problems = []
    for word in sorted(requires or ()):
        word = REQUIREMENT_ALIASES.get(word, word)
        if word == "kvm" and not cloud and not has_kvm:
            problems.append(
                "needs KVM for local VMs (/dev/kvm is missing): run on a Linux host with "
                "virtualization enabled, or use --on cloud"
            )
        elif word.startswith("env:"):
            name = word[4:]
            if not str(environ.get(name, "")).strip():
                problems.append(f"needs {name} in the environment")
    return problems
