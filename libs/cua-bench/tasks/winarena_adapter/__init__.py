"""Windows Arena benchmark adapter for cua-bench.

This adapter integrates the Windows Arena benchmark (154 tasks across 12 Windows
application domains) into cua-bench.

Usage:
    # Check status
    python -m tasks.winarena_adapter status

    # Prepare golden image (first time only)
    python -m tasks.winarena_adapter prepare-image

    # List tasks
    python -m tasks.winarena_adapter tasks --verbose

    # Run with cua-bench
    cb run tasks/winarena_adapter --task-id <id> --agent cua
"""

from .evaluator import WAAEvaluator
from .main import SetupStatus, check_setup, run_setup
from .setup_controller import WAASetupController
from .task_loader import load_waa_tasks

__all__ = [
    "load_waa_tasks",
    "WAAEvaluator",
    "WAASetupController",
    "check_setup",
    "run_setup",
    "SetupStatus",
]
