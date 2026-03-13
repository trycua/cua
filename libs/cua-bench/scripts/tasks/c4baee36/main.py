"""Auto-generated cb task for KiCad submission c4baee36-ea0a-4451-81f0-fd540d47326c."""
from __future__ import annotations

import asyncio
from pathlib import Path

import cua_bench as cb

_SUBMISSION_ID = "c4baee36-ea0a-4451-81f0-fd540d47326c"
_REMOTE_PROJECT_DIR = "/home/cua/kicad_project"
_HARNESS_DIR = Path(__file__).parent


@cb.tasks_config
def tasks() -> list[cb.Task]:
    return [
        cb.Task(
            description='Design a KiCad-style schematic of a switching, step-down voltage regulator to convert 48 Vdc to 3.3 Vdc. Use the LM2576HVS-3.3 from Texas Instruments.  \n\n\t• Pin 1 is the input and should be connected to both the input of 48 Vdc and a polarized capacitor (C1) of 100 uF. C1 has its negative terminal connected to ground.  \n\t• Pin 2 is connected to both a catch diode 1N5822 cathode and an inductor of 100 uH.  The other end of the diode is connected to ground. The other end of the inductor is connected to both Pin 4 and the positive end of polarized capacitor (C2) of 1000 uF. The other end of C2 is grounded.\nPins 3 and 5 should be connected directly to ground.',
            metadata={"difficulty": 'easy', "submission_id": _SUBMISSION_ID},
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
        remote_dir = f"{_REMOTE_PROJECT_DIR}/{rel.parent.as_posix()}"
        await session.run_command(f"mkdir -p '{remote_dir}'", check=False)
        await session.write_bytes(remote_path, local_path.read_bytes())

    try:
        await session.apps.kicad.launch(project_path=None)
    except Exception:
        pass
    await asyncio.sleep(5)


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession) -> None:
    """Oracle solver: upload reference.net directly to the expected output path."""
    await session.run_command(f"mkdir -p '{_REMOTE_PROJECT_DIR}'", check=False)
    await session.write_bytes(
        f"{_REMOTE_PROJECT_DIR}/output.net",
        (_HARNESS_DIR / "reference.net").read_bytes(),
    )


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> float:
    from cua_bench.netlist_compare import (
        compare_kicad_netlists,
        load_reference_netlist,
    )

    reference = load_reference_netlist(_HARNESS_DIR / "reference.net", _HARNESS_DIR)

    # Try common netlist output locations first
    candidate_paths = [
        f"{_REMOTE_PROJECT_DIR}/{_SUBMISSION_ID}.net",
        f"{_REMOTE_PROJECT_DIR}/output.net",
    ]
    for path in candidate_paths:
        result = await session.run_command(f"cat '{path}'", check=False)
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

    # Fall back: agent edited schematic without exporting — use kicad-cli to generate netlist
    result = await session.run_command(
        f"find {_REMOTE_PROJECT_DIR} -name '*.kicad_sch' | head -1", check=False
    )
    sch_path = result.get("stdout", "").strip()
    if sch_path:
        out_net = "/tmp/kicad_eval_output.net"
        await session.run_command(
            f"kicad-cli sch export netlist '{sch_path}' -o '{out_net}'",
            check=False,
        )
        result = await session.run_command(f"cat '{out_net}'", check=False)
        candidate = result.get("stdout", "")
        if candidate.strip():
            return compare_kicad_netlists(candidate, reference)

    return 0.0
