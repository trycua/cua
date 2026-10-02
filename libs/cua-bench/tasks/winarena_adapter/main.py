"""Windows Agent Arena (WAA) on cua-bench, built on ``BenchAdapter``.

173 tasks across 12 Windows application domains (Chrome, File Explorer,
LibreOffice Calc/Writer, VS Code, VLC, Edge, Settings, Clock, Calculator,
Paint, Notepad). WAA is no longer maintained upstream (microsoft/
WindowsAgentArena, MIT); its task JSON, setup operations and evaluators are
embedded here.

Windows is VM-only. WAA needs a Windows 11 image with its applications,
which Microsoft's license does not let us publish: build it yourself (see
README.md), push it to a registry as a VM image and point the adapter at it::

    CUA_BENCH_WAA_IMAGE=<registry>/waa-win11@sha256:... cb run tasks/winarena_adapter
    cb run tasks/winarena_adapter --on cloud          # Fleet KubeVirt
    CUA_BENCH_WAA_DIFFICULTY=hard cb run ...          # examples_noctxt

Without CUA_BENCH_WAA_IMAGE the canonical Windows image runs, which lacks
most of WAA's applications. Local runs need KVM (x86_64 Linux).
"""

import os
import sys
from pathlib import Path

import cua_bench as cb
from cua_bench.adapters import BenchAdapter

# Add parent directory to sys.path for imports when loaded as standalone module
_MODULE_DIR = Path(__file__).parent
if str(_MODULE_DIR.parent) not in sys.path:
    sys.path.insert(0, str(_MODULE_DIR.parent))

from winarena_adapter.evaluator import WAAEvaluator  # noqa: E402
from winarena_adapter.setup_controller import WAASetupController  # noqa: E402
from winarena_adapter.task_loader import load_waa_tasks  # noqa: E402

WAA_IMAGE_ENV = "CUA_BENCH_WAA_IMAGE"
WAA_DIFFICULTY_ENV = "CUA_BENCH_WAA_DIFFICULTY"


class WAAAdapter(BenchAdapter):
    """WAA: task JSON -> cb.Task, config[] setup, getter/metric evaluation."""

    id = "waa"
    version = "microsoft/WindowsAgentArena (embedded)"
    os_type = "windows"
    kinds = ("vm",)
    requires = frozenset({"kvm"})
    reset = "fresh-claim"
    action_mode = "driver"
    width, height = 1920, 1080

    def __init__(self) -> None:
        self.image = os.environ.get(WAA_IMAGE_ENV) or None

    def load_tasks(self, split: str) -> list[cb.Task]:
        difficulty = os.environ.get(WAA_DIFFICULTY_ENV, "normal")
        return load_waa_tasks(difficulty=difficulty)

    async def setup(self, task: cb.Task, session, ep) -> None:
        config = (task.metadata or {}).get("config") or []
        if config:
            await WAASetupController(session).setup(config)

    async def evaluate(self, task: cb.Task, session, ep) -> list[float]:
        evaluator_config = (task.metadata or {}).get("evaluator") or {}
        if not evaluator_config:
            return [0.0]
        return [await WAAEvaluator(session).evaluate(evaluator_config)]

    # WAA ships no reference solutions: `oracle` stays None, so `cb run`
    # without an agent evaluates the untouched start state.


ADAPTER = WAAAdapter().register(globals())


if __name__ == "__main__":
    cb.interact(__file__)
