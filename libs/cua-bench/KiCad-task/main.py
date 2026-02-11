"""KiCad circuit-creation tasks for cua-bench.

Tasks ask the agent to build a circuit in KiCad's schematic editor and export
a SPICE netlist.  Evaluation checks component counts and, when PySpice is
available, verifies node voltages via simulation.
"""

import cua_bench as cb

NETLIST_PATH = "~/output.cir"


@cb.tasks_config(split="train")
def load():
    """Define KiCad task variants."""
    tasks = [
        {
            "task_type": "create_circuit",
            "circuit_name": "VoltageDivider",
            "description": (
                "KiCad is already open. Using the Schematic Editor, create a voltage "
                "divider circuit with a 5 V DC source (V1), two 10 kΩ resistors "
                "(R1, R2) in series from the source to ground, and a net label "
                "'mid' on the node between the two resistors. Save the schematic, "
                f"then export a SPICE netlist to {NETLIST_PATH}"
            ),
            "expected_components": {"R": 2, "V": 1},
            "check_node": "mid",
            "expected_voltage": 2.5,
            "tolerance": 0.1,
        },
        {
            "task_type": "create_circuit",
            "circuit_name": "LEDCircuit",
            "description": (
                "KiCad is already open. Using the Schematic Editor, create a simple "
                "LED circuit: a 5 V DC source (V1), a 330 Ω current-limiting "
                "resistor (R1), and a diode (D1) in series to ground. "
                "Save the schematic, then export a SPICE netlist to "
                f"{NETLIST_PATH}"
            ),
            "expected_components": {"R": 1, "V": 1, "D": 1},
            "check_node": None,
            "expected_voltage": None,
            "tolerance": None,
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
    """Evaluate the agent's circuit by parsing and optionally simulating the netlist."""
    scores: list[float] = []

    # -- 1. Netlist must exist and be non-empty --------------------------------
    netlist = await session.apps.kicad.read_netlist(netlist_path=NETLIST_PATH)
    if not netlist.strip():
        return [0.0]

    # -- 2. Component counts must match ----------------------------------------
    components = await session.apps.kicad.get_components(netlist_path=NETLIST_PATH)
    expected = task_cfg.metadata.get("expected_components", {})
    for prefix, count in expected.items():
        actual = sum(1 for c in components if c["ref"].upper().startswith(prefix))
        scores.append(1.0 if actual == count else 0.0)

    # -- 3. Simulate and verify node voltage (if specified) --------------------
    check_node = task_cfg.metadata.get("check_node")
    expected_v = task_cfg.metadata.get("expected_voltage")
    tolerance = task_cfg.metadata.get("tolerance", 0.1)

    if check_node and expected_v is not None:
        try:
            voltages = await session.apps.kicad.simulate_operating_point(
                netlist_path=NETLIST_PATH, ground=0,
            )
            actual_v = voltages.get(check_node)
            if actual_v is not None and abs(actual_v - expected_v) <= tolerance:
                scores.append(1.0)
            else:
                scores.append(0.0)
        except Exception:
            scores.append(0.0)

    return [sum(scores) / len(scores)] if scores else [0.0]


@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    """Oracle not implemented — KiCad requires GUI interaction."""
    pass


if __name__ == "__main__":
    cb.interact(__file__)
