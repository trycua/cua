"""KiCad circuit-creation tasks for cua-bench.

Tasks ask the agent to build a circuit in KiCad's schematic editor and export
a netlist in KiCad format (.net). Evaluation parses the .net file and
verifies component counts (and optional reference comparison).
"""

import cua_bench as cb

NETLIST_PATH = "/home/cua/output.net"


@cb.tasks_config(split="train")
def load():
    """Define KiCad task variants."""
    tasks = [
        {
            "task_type": "create_circuit",
            "circuit_name": "LEDCircuit",
            "description": (
                "KiCad is already open. Using the Schematic Editor, "
                "draw a clean schematic in KiCAD for a 9V battery driving "
                "two infrared LN271 LEDs. The battery positive terminal is first "
                "connected to a 100 ohm current limiting resistor followed by the "
                "two LEDs. Save the schematic, then export the netlist in KiCad "
                f"format to {NETLIST_PATH}"
            ),
            "expected_components": {"R": 1, "V": 1, "D": 2},
        },
        {
            "task_type": "create_circuit",
            "circuit_name": "EmptyNetlistExport",
            "description": (
                "KiCad is already open. Create an empty project (new schematic, "
                "no components required). Then export the "
                f"netlist in KiCad format to {NETLIST_PATH} using all default "
                "settings. The goal is to produce a valid KiCad .net file."
            ),
            "expected_components": {},
            "netlist_export_only": True,
        },
    ]

    return [
        cb.Task(
            description=task["description"],
            metadata=task,
            computer={
                "provider": "native",
                "setup_config": {
                    "os_type": "linux",
                    "width": 1920,
                    "height": 1080,
                },
            },
        )
        for task in tasks
    ]


@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    """Install KiCad and launch it so the window is visible."""
    await session.apps.kicad.install(with_shortcut=True)
    await session.apps.kicad.launch()


@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    """Evaluate by parsing the KiCad .net netlist and checking component counts."""
    scores: list[float] = []

    # Netlist must exist and be non-empty
    netlist = await session.apps.kicad.read_netlist(netlist_path=NETLIST_PATH)
    if not netlist.strip():
        print("Evaluation: netlist missing or empty at " + NETLIST_PATH)
        return [0.0]

    # Component counts must match (parsed from KiCad .net format); if no expected components, valid .net = 1.0
    components = await session.apps.kicad.get_components(netlist_path=NETLIST_PATH)
    expected = task_cfg.metadata.get("expected_components", {})
    if not expected:
        print("Evaluation: valid netlist, no component check -> 1.0")
        return [1.0]
    for prefix, count in expected.items():
        actual = sum(1 for c in components if c["ref"].upper().startswith(prefix))
        scores.append(1.0 if actual == count else 0.0)
    return [sum(scores) / len(scores)] if scores else [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle not implemented — KiCad requires GUI interaction."""
    pass


if __name__ == "__main__":
    cb.interact(__file__)
