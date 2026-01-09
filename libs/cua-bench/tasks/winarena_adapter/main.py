"""Windows Arena benchmark adapter for cua-bench.

This adapter integrates the Windows Arena benchmark (173 tasks across 12 Windows
application domains) into cua-bench. Since WAA is no longer maintained, the essential
code has been embedded directly.

Domains:
- Chrome (17 tasks)
- File Explorer (19 tasks)
- LibreOffice Calc (24 tasks)
- LibreOffice Writer (19 tasks)
- VS Code (23 tasks)
- VLC Media Player (21 tasks)
- MS Edge (13 tasks)
- Settings (5 tasks)
- Clock (4 tasks)
- Windows Calculator (3 tasks)
- Microsoft Paint (3 tasks)
- Notepad (2 tasks)
"""

import os
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import cua_bench as cb

# Add parent directory to sys.path for imports when loaded as standalone module
_MODULE_DIR = Path(__file__).parent
if str(_MODULE_DIR.parent) not in sys.path:
    sys.path.insert(0, str(_MODULE_DIR.parent))

from winarena_adapter.evaluator import WAAEvaluator
from winarena_adapter.setup_controller import WAASetupController
from winarena_adapter.task_loader import load_waa_tasks

# ============================================================================
# Setup Check (for cb run --setup integration)
# ============================================================================


@dataclass
class SetupStatus:
    """Status of adapter setup requirements."""

    ready: bool
    message: str
    can_setup: bool = False
    setup_command: Optional[str] = None


def check_setup() -> SetupStatus:
    """Check if Windows Arena is ready to run locally.

    This function is called by `cb run` to check if the adapter
    needs first-time setup before running tasks.

    Returns:
        SetupStatus with ready=True if golden image exists,
        or ready=False with instructions if setup is needed.
    """
    # Check for KVM (required for local runs)
    has_kvm = os.path.exists("/dev/kvm")

    if not has_kvm:
        return SetupStatus(
            ready=False,
            message="Windows Arena requires KVM for local execution.\n"
            "KVM is only available on Linux with virtualization enabled.\n"
            "For cloud execution, use --provider with a pre-configured image.",
            can_setup=False,
        )

    # Check for base image
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    image_path = Path(xdg_data) / "cua-bench" / "images" / "windows-qemu"
    windows_boot = image_path / "windows.boot"

    if windows_boot.exists():
        return SetupStatus(
            ready=True,
            message=f"Windows image ready at: {image_path}",
        )

    return SetupStatus(
        ready=False,
        message="Windows Arena requires a base VM image (first-time setup).\n"
        "This requires a Windows 11 ISO file (~6GB download).\n\n"
        "Options:\n"
        "  1. Auto-download ISO:  cb run tasks/winarena_adapter --setup --download-iso\n"
        "  2. Use existing ISO:   cb run tasks/winarena_adapter --setup --iso /path/to/windows.iso",
        can_setup=True,
        setup_command="cb run tasks/winarena_adapter --setup --download-iso",
    )


def run_setup(iso_path: Optional[str] = None, download_iso: bool = False) -> bool:
    """Run the setup process to prepare the golden image.

    This function is called by `cb run --setup` to prepare the adapter.

    Args:
        iso_path: Path to Windows 11 ISO file (optional)
        download_iso: Whether to download the ISO from Microsoft

    Returns:
        True if setup succeeded, False otherwise.
    """
    from winarena_adapter.cli import cmd_prepare_image

    # Create a mock args object
    class PrepareArgs:
        force = False
        skip_pull = False
        no_kvm = False
        ram = "8G"
        cpus = "8"
        browser_port = "8006"
        rdp_port = "3390"

    args = PrepareArgs()
    args.iso = iso_path
    args.download_iso = download_iso

    return cmd_prepare_image(args) == 0


@cb.tasks_config(split="train")
def load():
    """Load all 173 WAA benchmark tasks."""
    return load_waa_tasks()


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Execute WAA task setup operations (config array)."""
    config = task_cfg.metadata.get("config", [])
    if config:
        controller = WAASetupController(session)
        await controller.setup(config)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Evaluate task completion using WAA evaluators."""
    evaluator_config = task_cfg.metadata.get("evaluator", {})

    if not evaluator_config:
        return [0.0]

    evaluator = WAAEvaluator(session)
    score = await evaluator.evaluate(evaluator_config)

    return [score]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """WAA does not provide oracle solutions."""
    raise NotImplementedError("Windows Arena benchmark does not include oracle solutions.")


if __name__ == "__main__":
    cb.interact(__file__)
