"""Windows Agent Arena (WAA) adapter for cua-bench (a ``BenchAdapter``).

154 tasks across 12 Windows application domains.

Usage:
    # Run with cua-bench (Windows is VM-only; see main.py for the image)
    CUA_BENCH_WAA_IMAGE=<registry ref> cb run tasks/winarena_adapter --agent cua-agent

    # List tasks
    python -m tasks.winarena_adapter tasks --verbose
"""

from .evaluator import WAAEvaluator
from .setup_controller import WAASetupController
from .task_loader import load_waa_tasks

__all__ = [
    "load_waa_tasks",
    "WAAEvaluator",
    "WAASetupController",
]
