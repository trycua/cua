"""OSWorld-Verified on cua-bench, built on ``BenchAdapter``.

369 tasks (xlang-ai/OSWorld at a pinned commit, Apache-2.0) on the
``bench-osworld`` image: the upstream Ubuntu 22.04 disk with cua-spacesd, as
one index with a VM variant (QEMU / Fleet KubeVirt) and a container variant
(docker / gVisor; supervisord instead of systemd). The OSWorld server on
:5000 does setup and feeds the evaluators exactly as upstream; the agent
acts through cua-driver (``sb.mouse`` / ``sb.keyboard``), like on every
other benchmark. ``action_mode="native"`` runs pyautogui commands through
the server's ``/execute`` instead.

::

    cb run dataset libs/cua-bench/tasks/osworld --agent <agent> --max-variants 5
    cb run dataset libs/cua-bench/tasks/osworld --kind vm               # QEMU / KubeVirt
    cb run dataset libs/cua-bench/tasks/osworld --on cloud --kind container
    CUA_BENCH_OSWORLD_SPLIT=parity cb run dataset libs/cua-bench/tasks/osworld --oracle

Splits (``CUA_BENCH_OSWORLD_SPLIT``): test_all (default), test_small,
test_nogdrive, test_infeasible, parity (the tasks in
``libs/images/bench/osworld/parity.txt`` with scripted oracles).

Needs ``pip install "cua-bench[osworld]"``. Setup and evaluation download
task files on this machine and upload them (``download`` steps), so the
host needs internet; tasks that browse need egress in the guest too. The 8
Google Drive tasks need OSWorld's OAuth settings (upstream SETUP_GUIDELINE).

Container variant fidelity: tasks that need systemd, logind, GNOME settings
daemons, timedatectl, audio or Bluetooth hardware, snaps or user management
(mostly the ``os`` domain) carry ``container_fidelity: "degraded"``
(:func:`container_fidelity`) and are excluded from container parity claims.
"""

from __future__ import annotations

import asyncio
import json
import os
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import cua_bench as cb
from cua_bench import images
from cua_bench.adapters import BenchAdapter, ServerSpec

_HERE = Path(__file__).resolve().parent
if str(_HERE.parent) not in sys.path:
    sys.path.insert(0, str(_HERE.parent))

from osworld import oracles, upstream  # noqa: E402

SPLIT_ENV = "CUA_BENCH_OSWORLD_SPLIT"
CHROMIUM_PORT = 9222
VLC_PORT = 8080
CLIENT_PASSWORD = "password"


#: What a container (supervisord, no systemd/logind, no audio or Bluetooth
#: hardware, snaps removed) cannot reproduce: a task mentioning one of these
#: carries ``container_fidelity: "degraded"``.
_DEGRADED_ANY = ("gsettings", "dconf", "timedatectl", "systemctl", "loginctl", "hostnamectl",
                 "pactl", "amixer", "bluetooth")
_DEGRADED_OS = ("useradd", "adduser", "passwd", "snap ", "apt-get", "install", "time zone",
                "switch to the user", "lock", "battery")


def container_fidelity(task: dict, domain: str) -> str:
    """``degraded`` when the task needs what the container variant lacks, else ``full``."""
    text = json.dumps(task).lower()
    words = _DEGRADED_ANY + (_DEGRADED_OS if domain == "os" else ())
    return "degraded" if any(w in text for w in words) else "full"


class OSWorldEnv:
    """What ``DesktopEnv.evaluate`` / ``_set_task_info`` read, without a provider.

    Built per task; controllers point at ``host:port`` pairs that reach the
    guest (published ports locally, loopback bridges on Fleet).
    """

    def __init__(self, de: Any, server: tuple, chromium: tuple, vlc: tuple, cache_dir: str,
                 width: int, height: int) -> None:
        self.vm_ip, self.server_port = server
        # Upstream builds every URL from vm_ip, so all three ports must share
        # a host (loopback in both cases).
        if chromium[0] != self.vm_ip or vlc[0] != self.vm_ip:
            raise RuntimeError("OSWorld needs the server, CDP and VLC ports on one host")
        self.chromium_port = chromium[1]
        self.vlc_port = vlc[1]
        self.cache_dir_base = cache_dir
        self.cache_dir = cache_dir
        self.client_password = CLIENT_PASSWORD
        self.screen_width, self.screen_height = width, height
        self.enable_proxy = False
        self.current_use_proxy = False
        self.is_environment_used = False
        self.action_history: list = []
        self.controller = de.python.PythonController(vm_ip=self.vm_ip, server_port=self.server_port)
        self.setup_controller = de.setup.SetupController(
            vm_ip=self.vm_ip, server_port=self.server_port, chromium_port=self.chromium_port,
            vlc_port=self.vlc_port, cache_dir=cache_dir, client_password=CLIENT_PASSWORD,
            screen_width=width, screen_height=height,
        )
        self._de = de

    def __getattr__(self, name: str) -> Any:
        # DesktopEnv's own helpers (_set_evaluator_info, ...) bound to this
        # object, so its methods run unchanged.
        de = self.__dict__.get("_de")
        func = getattr(de.DesktopEnv, name, None) if de is not None else None
        if callable(func) and not isinstance(func, property):
            return func.__get__(self, type(self))
        raise AttributeError(name)

    # DesktopEnv exposes these as properties over the controller.
    @property
    def vm_platform(self) -> str:
        return self.controller.get_vm_platform()

    @property
    def vm_machine(self) -> str:
        return self.controller.get_vm_machine()

    @property
    def vm_screen_size(self) -> Any:
        return self.controller.get_vm_screen_size()

    def set_task(self, task: dict) -> None:
        self._de.DesktopEnv._set_task_info(self, task)
        self.setup_controller.reset_cache_dir(self.cache_dir)

    def evaluate(self) -> float:
        return self._de.DesktopEnv.evaluate(self)


class OSWorldAdapter(BenchAdapter):
    """OSWorld task JSON -> cb.Task; upstream SetupController and evaluators."""

    id = "osworld"
    version = f"verified@{upstream.OSWORLD_COMMIT[:7]}"
    kinds = ("container", "vm")
    requires = frozenset({"egress"})
    server = ServerSpec(port=5000, health="/screenshot")
    ports = (CHROMIUM_PORT, VLC_PORT)
    reset = "fresh-claim"
    action_mode = "driver"
    width, height = 1920, 1080

    def __init__(self, split: str | None = None, root: str | None = None) -> None:
        self.image = images.image("BENCH_OSWORLD")
        self.split = split or os.environ.get(SPLIT_ENV) or "test_all"
        self._root_arg = root
        self._root: Path | None = None

    @property
    def root(self) -> Path:
        if self._root is None:
            self._root = upstream.ensure_tree(self._root_arg)
        return self._root

    def load_tasks(self, split: str) -> list[cb.Task]:
        out = []
        for domain, task_id in upstream.task_index(self.root, self.split, oracles.PARITY):
            task = upstream.load_task(self.root, domain, task_id)
            out.append(
                cb.Task(
                    description=task["instruction"],
                    task_id=f"osworld-{task_id}",
                    metadata={
                        "osworld_id": task_id,
                        "domain": domain,
                        "osworld": task,
                        "container_fidelity": container_fidelity(task, domain),
                        "has_oracle": task_id in oracles.ORACLES,
                    },
                )
            )
        return out

    async def _env(self, task: cb.Task, ep: Any) -> OSWorldEnv:
        de = upstream.import_desktop_env(self.root)
        server = await ep.host_port("server")
        chromium = await ep.host_port(CHROMIUM_PORT)
        vlc = await ep.host_port(VLC_PORT)
        cache = os.path.join(tempfile.gettempdir(), "cua-bench-osworld-cache")
        env = OSWorldEnv(de, server, chromium, vlc, cache, self.width, self.height)
        env.set_task(task.metadata["osworld"])
        return env

    async def setup(self, task: cb.Task, session: Any, ep: Any) -> None:
        env = await self._env(task, ep)
        ok = await asyncio.to_thread(env.setup_controller.setup, env.config, False)
        if ok is False:
            raise RuntimeError(f"OSWorld setup failed for {task.metadata['osworld_id']}")

    async def evaluate(self, task: cb.Task, session: Any, ep: Any) -> float:
        env = await self._env(task, ep)
        # An agent that declared the task infeasible (session.infeasible is
        # True) ends with OSWorld's FAIL action, as upstream agents do.
        if getattr(session, "infeasible", False) is True:
            env.action_history.append("FAIL")
        return float(await asyncio.to_thread(env.evaluate))

    async def act(self, action: Any, session: Any, ep: Any) -> None:
        """``action_mode="native"``: a pyautogui command string through /execute."""
        command = action if isinstance(action, str) else getattr(action, "command", None)
        if not command:
            raise NotImplementedError("OSWorld native actions are pyautogui command strings")
        env = SimpleNamespace()
        host, port = await ep.host_port("server")
        de = upstream.import_desktop_env(self.root)
        env.controller = de.python.PythonController(vm_ip=host, server_port=port)
        await asyncio.to_thread(env.controller.execute_python_command, command)

    async def oracle(self, task: cb.Task, session: Any, ep: Any) -> None:
        """Scripted solutions for the parity tasks (no model)."""
        await oracles.solve(task.metadata["osworld_id"], ep)


ADAPTER = OSWorldAdapter().register(globals())
