"""Auto-generated cb task for KiCad submission 83490c3a-a8cb-4595-b795-5648c3d5bb0e."""
from __future__ import annotations

import asyncio
from pathlib import Path

import cb

_SUBMISSION_ID = "83490c3a-a8cb-4595-b795-5648c3d5bb0e"
_REMOTE_PROJECT_DIR = "/home/cua/kicad_project"
_HARNESS_DIR = Path(__file__).parent


@cb.tasks_config
def tasks() -> list[cb.Task]:
    return [
        cb.Task(
            description='Design a power tree in Kicad with the following attributes:  \n\t• Convert the input voltage of +12V to the backbone power of 3.3V by using the voltage switching module OKI-78SR-3.3/1.5-W36E-C.\n\t• From the 3.3V power, use LDO regulators from the LT1761 series to create voltages of 2.5V, 1.8V, and 1.2V.\n\t\t○ To reduce noise, connect a 0.01uF capacitor to the BYPASS pin of each LDO.\n\t• To reduce noise and increase stability for each voltage rail, add additional capacitors. For the switching module and each LDO, add for each\n\t\t○ An unpolarized input bypass capacitor of 1uF\n\t\t○ A polarized output capacitor of 10uF\n\t• Label the output voltages as 3p3V, 2p5V, 1p8V, and 1p2V respectively.',
            metadata={"difficulty": 'medium', "submission_id": _SUBMISSION_ID},
        )
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession) -> None:
    await session.apps.kicad.install(with_shortcut=True)

    initial_dir = _HARNESS_DIR / "initial"
    for local_path in sorted(initial_dir.rglob("*")):
        if not local_path.is_file():
            continue
        rel = local_path.relative_to(initial_dir)
        remote_path = f"{_REMOTE_PROJECT_DIR}/{rel.as_posix()}"
        await session.run_command(
            f"mkdir -p $(dirname '{remote_path}')", check=False
        )
        await session.write_bytes(remote_path, local_path.read_bytes())

    try:
        await session.apps.kicad.launch()
    except Exception:
        pass
    await asyncio.sleep(5)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> float:
    from cua_bench.netlist_compare import compare_kicad_netlists, load_reference_netlist

    reference = load_reference_netlist(_HARNESS_DIR / "reference.net")

    # Try common netlist output locations
    candidate_paths = [
        f"{_REMOTE_PROJECT_DIR}/{_SUBMISSION_ID}.net",
        f"{_REMOTE_PROJECT_DIR}/output.net",
    ]
    for path in candidate_paths:
        result = await session.run_command(
            f"cat '{path}'", check=False
        )
        candidate = result.get("stdout", "")
        if candidate.strip():
            return compare_kicad_netlists(candidate, reference)

    # Search for any .net file in the project dir
    result = await session.run_command(
        f"find {_REMOTE_PROJECT_DIR} -name '*.net' | head -1", check=False
    )
    net_path = result.get("stdout", "").strip()
    if net_path:
        result = await session.run_command(f"cat '{net_path}'", check=False)
        candidate = result.get("stdout", "")
        if candidate.strip():
            return compare_kicad_netlists(candidate, reference)

    return 0.0
