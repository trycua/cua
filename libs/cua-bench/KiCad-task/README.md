# KiCad Task

Simple cua-bench task that installs the open-source [KiCad](https://www.kicad.org/) EDA suite and verifies that the agent can create and save a new project.

## Task

- **Setup**: KiCad is launched (use env image `cua-xfce-kicad:latest` so KiCad is pre-installed).
- **Goal**: Create schematics and export netlists in KiCad format to `~/output.net`. Variant 0: specific circuit; variant 1: empty project.
- **Verification**: Parses the .net file: variant 0 checks component counts; variant 1 passes if a valid KiCad .net file was produced.

## Variants

| Variant | Name               | Description |
|--------|--------------------|-------------|
| 0      | LEDCircuit         | 9V battery, 100 Ω resistor, two LN271 LEDs; export netlist to `~/output.net`. |
| 1      | EmptyNetlistExport | Create an empty project, save, and export netlist to `~/output.net` with default settings. Verifier passes if the file is valid KiCad .net. |

## Running

Requires native provider (Docker/QEMU) with `os_type: "linux"` (or `"windows"` if you adjust the task config).

```bash
# Interactive preview (from cua-bench repo root)
cb interact KiCad-task --variant-id 0

# Run with oracle (completes successfully; solve is a no-op, so reward is 0.0)
cb run task KiCad-task --variant-id 0 --oracle

# Run with agent
cb run task KiCad-task --variant-id 0 --agent cua-agent --model <model>
```

**Note:** With `--oracle`, the run completes (setup installs KiCad, solve is a no-op, evaluate runs). Reward is 0.0 unless an agent or human creates the project. Setup may take several minutes while KiCad is installed in the environment.

## Files

- `main.py` – Task definition, setup (install KiCad), evaluation (check project file), and solve stub.
