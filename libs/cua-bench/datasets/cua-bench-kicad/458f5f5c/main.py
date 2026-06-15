"""Auto-generated cb task for KiCad submission 458f5f5c-1347-479c-a94b-9bf4748de65a."""
from __future__ import annotations

import asyncio
from pathlib import Path

import cua_bench as cb

_SUBMISSION_ID = "458f5f5c-1347-479c-a94b-9bf4748de65a"
_REMOTE_PROJECT_DIR = "/home/cua/kicad_project"
_HARNESS_DIR = Path(__file__).parent


@cb.tasks_config
def tasks() -> list[cb.Task]:
    return [
        cb.Task(
            description='''Design a microphone preamplifier that conditions a raw mic signal into a clean, ADC-safe input for a 3.3 V microcontroller.
* The circuit connects to the microphone using a 2-pole audio jack connector with both tip and shield switches. The connect detect terminals are not used and are simply shorted to each other.
* Use one 56k kΩ resistor to provide a power bias to the microphone.
* Use an LM741 op-amp powered from a +12 V single supply in a non-inverting configuration. Use two other 56 kΩ resistors to create a mid-rail bias reference for the opamp input.  A 4.7 µF electrolytic capacitor is used to AC-couple the microphone signal into the biased opamp input. 
* Configure adjustable gain using a 50 kΩ potentiometer (RV1) in the feedback path, and include a 1.1 kΩ resistor and a 4.7 µF capacitor connected to the inverting input as part of the gain-setting and low-frequency shaping network. 
* At the output, include a 1.1 kΩ load resistor and AC-couple the output using a 4.7 µF capacitor. 
* After the coupling capacitor, add an ADC interface that strongly attenuates the signal using an 18 kΩ / 1.1 kΩ divider and clamps the ADC node with two Schottky diodes to the 3.3 V rail and ground for input protection. 
* Tie all grounds together and generate the complete schematic.''',
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
