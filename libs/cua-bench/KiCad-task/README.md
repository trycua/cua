# KiCad Task

Simple cua-bench task that installs the open-source [KiCad](https://www.kicad.org/) EDA suite and verifies that the agent can create and save a new project.

## Task

- **Setup**: Installs KiCad via the cua-bench app registry (Linux: PPA + apt; Windows: winget; macOS: Homebrew).
- **Goal**: Create a new KiCad project with a given name and save it to `Desktop/KiCadProjects/<project_name>/`.
- **Verification**: Checks that the project folder exists and contains the expected `<project_name>.kicad_pro` file.

## Variants

| Variant | Project name   | Description |
|--------|----------------|-------------|
| 0      | MyFirstBoard   | Create and save project "MyFirstBoard" to Desktop/KiCadProjects. |
| 1      | BlinkyPCB      | Create and save project "BlinkyPCB" to Desktop/KiCadProjects. |

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
