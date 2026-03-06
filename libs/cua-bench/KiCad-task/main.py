"""KiCad circuit-creation tasks for cua-bench.

Tasks ask the agent to build a circuit in KiCad's schematic editor and export
a netlist in KiCad format (.net). Evaluation compares the agent's .net output
against a reference netlist (structural comparison) when one is provided, or
falls back to component-count checking when only ``expected_components`` is set.

Adding a new task variant:
    1. Add a dict to TASK_VARIANTS with at minimum a ``description`` key.
    2. Optionally include ``reference_netlist`` (path relative to this file's
       directory) for full structural comparison via netlist_compare.py.
    3. Optionally include ``expected_components`` (dict of ref-prefix → count)
       as a lighter-weight fallback when no reference netlist is available.
    4. Optionally include ``initial_circuit_file`` (path relative to this
       file's directory) to seed the agent with a starting schematic.
"""

from pathlib import Path

import cua_bench as cb
from netlist_compare import compare_kicad_netlists, load_reference_netlist

NETLIST_PATH = "/home/cua/output.net"
_TASK_DIR = Path(__file__).resolve().parent

# ---------------------------------------------------------------------------
# Task variants — extend this list to add new tasks.
# Each dict becomes one Task; only ``description`` is required.
# ---------------------------------------------------------------------------
TASK_VARIANTS = [
    {
        "circuit_name": "LEDCircuit",
        "description": (
            "KiCad is already open. Using the Schematic Editor, "
            "draw a clean schematic in KiCad for a 9V battery driving "
            "two infrared LN271 LEDs. The battery positive terminal is first "
            "connected to a 100 ohm current limiting resistor followed by the "
            "two LEDs. Save the schematic, then export the netlist in KiCad "
            f"format to {NETLIST_PATH}."
        ),
        # Full structural comparison against the bundled reference netlist.
        "reference_netlist": "kicad_twoleds_netlist.net",
    },
    {
        "circuit_name": "EmptyNetlistExport",
        "description": (
            "KiCad is already open. Create an empty project (new schematic, "
            "no components required). Then export the netlist in KiCad format "
            f"to {NETLIST_PATH} using all default settings. "
            "The goal is to produce a valid KiCad .net file."
        ),
        # No reference netlist and no expected components: any valid .net passes.
        "netlist_export_only": True,
    },
]


@cb.tasks_config(split="train")
def load():
    """Build Task objects from TASK_VARIANTS."""
    return [
        cb.Task(
            description=variant["description"],
            metadata=variant,
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "linux",
                    "width": 1920,
                    "height": 1080,
                },
            },
        )
        for variant in TASK_VARIANTS
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Install KiCad, optionally seed with an initial circuit file, then launch."""
    await session.apps.kicad.install(with_shortcut=True)

    initial_file = task_cfg.metadata.get("initial_circuit_file")
    if initial_file:
        await session.apps.kicad.launch(project_file=str(_TASK_DIR / initial_file))
    else:
        await session.apps.kicad.launch()


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Evaluate the agent's output netlist.

    Evaluation priority:
    1. ``reference_netlist`` in metadata → full structural comparison
       (component set + net topology) via compare_kicad_netlists().
    2. ``expected_components`` in metadata → lightweight component-count check.
    3. ``netlist_export_only`` or neither → any valid .net file scores 1.0.
    """
    netlist = await session.apps.kicad.read_netlist(netlist_path=NETLIST_PATH)
    if not netlist.strip():
        print(f"Evaluation: netlist missing or empty at {NETLIST_PATH}")
        return [0.0]

    meta = task_cfg.metadata

    # --- Path 1: full structural comparison ---
    ref_path = meta.get("reference_netlist")
    if ref_path:
        reference = load_reference_netlist(ref_path, _TASK_DIR)
        score = compare_kicad_netlists(netlist, reference)
        print(f"Evaluation (structural): score={score:.3f}")
        return [score]

    # --- Path 2: component-count fallback ---
    expected = meta.get("expected_components", {})
    if expected:
        components = await session.apps.kicad.get_components(netlist_path=NETLIST_PATH)
        scores = [
            1.0 if sum(1 for c in components if c["ref"].upper().startswith(prefix)) == count else 0.0
            for prefix, count in expected.items()
        ]
        score = sum(scores) / len(scores)
        print(f"Evaluation (component counts): score={score:.3f}")
        return [score]

    # --- Path 3: valid .net file is sufficient ---
    print("Evaluation: valid netlist, no further checks → 1.0")
    return [1.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle not implemented — KiCad requires GUI interaction."""
    pass


if __name__ == "__main__":
    cb.interact(__file__)
