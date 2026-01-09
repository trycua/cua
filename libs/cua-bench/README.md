# cua-bench

A toolkit for verifiable cross-platform computer-use RL environments and benchmarks. Create custom GUI tasks with HTML/CSS/JS or run tasks on real Windows, Linux, macOS environments.

## Installation

```bash
pip install cua-bench
```

Or install from source:

```bash
git clone https://github.com/trycua/cua.git
cd cua/libs/cua-bench
pip install -e .
```

## Quick Start

### 1. Create a Base Image (Optional)

For tasks requiring native environments (real OS), create a base image:

```bash
# Quick setup: Linux container (no KVM required)
cb image create linux-docker

# Windows VM (requires KVM, ~1-2 hours)
cb image create windows-qemu --download-iso

# Check available images
cb image list
```

Most tasks use simulated environments (HTML/CSS/JS) and don't require base images.

### 2. Run a Task

```bash
export ANTHROPIC_API_KEY=sk-....

# Run a single task with an AI agent (async)
cb run task tasks/click_button --agent cua-agent --model anthropic/claude-sonnet-4-20250514

# Watch progress
cb run watch <run_id>

# View the trace
cb trace view <run_id>
```

### 3. Explore Interactively

```bash
# Open task in browser for manual testing
cb interact tasks/click_button --variant-id 0

# Run with oracle solution
cb interact tasks/click_button --oracle
```

## Running Tasks

### Single Task

Tasks run **asynchronously by default** and return immediately with a run ID:

```bash
# Run async (returns immediately)
cb run task tasks/my_task --agent cua-agent --model anthropic/claude-sonnet-4-20250514

# Monitor progress
cb run watch <run_id>

# Or wait for completion inline
cb run task tasks/my_task --agent cua-agent --model anthropic/claude-sonnet-4-20250514 --wait

# Run with oracle solution (reference implementation)
cb run task tasks/my_task --oracle --wait
```

### Run a Dataset

Run multiple tasks in parallel:

```bash
# Run entire dataset
cb run dataset datasets/cua-bench-basic \
    --agent cua-agent \
    --model anthropic/claude-haiku-4-5 \
    --max-parallel 8 \
    --max-steps 10

# Run with filtering
cb run dataset datasets/cua-bench-basic \
    --task-filter "click*" \
    --max-variants 1
```

### Managing Runs

```bash
# List all runs
cb run list

# Watch a run in real-time
cb run watch <run_id>

# Check run status
cb run info <run_id>

# View logs
cb run logs <run_id>

# Stop a run
cb run stop <run_id>
```

## Viewing Traces

```bash
# View traces from a run
cb trace view <run_id>

# Opens interactive viewer at http://127.0.0.1:55115/
```

## Creating Tasks

### Generate a Task with AI

```bash
# AI-assisted task generation
cb generate-task "2048 game" --output tasks/2048_game

# Create empty task scaffold
cb create-task tasks/my_task
```

### Task Structure

Every task has a `main.py` with four decorated functions:

```python
import cua_bench as cb
from pathlib import Path

pid = None

@cb.tasks_config(split="train")
def load():
    return [
        cb.Task(
            description='Click the "Submit" button.',
            metadata={"button_text": "Submit"},
            computer={
                "provider": "simulated",  # or "native"
                "setup_config": {
                    "os_type": "win11",  # win11, macos (simulated) or linux, windows (native)
                    "width": 1024,
                    "height": 768
                }
            }
        )
    ]

@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid
    # Launch a window with HTML GUI
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text(),
        title="My App",
        width=800,
        height=600
    )

@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid
    if pid is None:
        return [0.0]

    # Read state from JavaScript
    submitted = await session.execute_javascript(pid, "window.__submitted")
    return [1.0] if submitted else [0.0]

@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid
    if pid is None:
        return

    # Get element position and click it
    rect = await session.get_element_rect(pid, "#submit-btn")
    if rect:
        await session.click(rect["x"] + rect["width"] // 2, rect["y"] + rect["height"] // 2)

    # Or click via JavaScript
    # await session.execute_javascript(pid, 'document.querySelector("#submit-btn").click()')
```

## Programmatic Interface

```python
import cua_bench as cb

# Load an environment
env = cb.make("./tasks/my_task")

# Reset to start a task
screenshot, task_config = await env.reset(task_id=0)

# Get the session
session = env.session

# Take actions
await session.click(100, 200)
await session.type("Hello, world!")
await session.key("Enter")

# Take screenshot
screenshot_bytes = await session.screenshot()

# Run oracle solution
await env.solve()

# Evaluate
rewards = await env.evaluate()

# Clean up
await env.close()
```

### Session API

```python
# Mouse actions
await session.click(x, y)
await session.right_click(x, y)
await session.double_click(x, y)
await session.move_to(x, y)
await session.drag(from_x, from_y, to_x, to_y)
await session.scroll(direction="down", amount=100)

# Keyboard actions
await session.type("Hello, world!")
await session.key("Enter")
await session.hotkey(["ctrl", "c"])

# Screenshots
screenshot_bytes = await session.screenshot()  # PNG bytes

# Window management (for simulated tasks)
pid = await session.launch_window(
    html="<h1>Hello</h1>",
    title="My Window",
    width=800,
    height=600
)

# Execute JavaScript in window
result = await session.execute_javascript(pid, "document.title")

# Get element position
rect = await session.get_element_rect(pid, ".my-button")
# Returns: {"x": 10, "y": 20, "width": 100, "height": 30}

# Close all windows
await session.close_all_windows()
```

## Base Images

Base images are reproducible environments for running tasks on real operating systems.

### Create Images

```bash
# Linux container (fast, no KVM)
cb image create linux-docker

# Linux VM with KVM
cb image create linux-qemu

# Windows 11 VM (~1-2 hours)
cb image create windows-qemu --download-iso

# Android VM
cb image create android-qemu

# macOS VM (Apple Silicon only)
cb image create macos-lume
```

### Manage Images

```bash
# List all images
cb image list

# Show image details
cb image info windows-qemu

# Clone for customization
cb image clone windows-qemu windows-custom

# Interactive shell (read-only overlay)
cb image shell windows-qemu

# Interactive shell (modify golden image)
cb image shell windows-qemu --writable

# Delete an image
cb image delete windows-custom
```

## Export Training Data

Export agent trajectories to training datasets:

```bash
# Export with aguvis-stage-1 format (default)
cb dataset build <run-id> --save-dir datasets/

# Export with gui-r1 format
cb dataset build <run-id> --mode gui-r1 --save-dir datasets/

# Push to Hugging Face
cb dataset build <run-id> --push-to-hub --repo-id username/my-dataset
```

**Available formats:**

- **aguvis-stage-1** - Action augmentation dataset (AgUVis/smol2operator style)
- **gui-r1** - Low-level click instructions (GUI-R1 format)

## Platform Support

### Simulated Desktop (HTML/CSS/JS)

Fast, lightweight environments using Playwright. No Docker required.

**Best for:**

- Custom GUI tasks
- Web app testing
- Quick iteration
- Cross-platform compatibility

**Themes:**

- `win11` - Windows 11 desktop simulation
- `macos` - macOS desktop simulation

### Native Desktop (Docker/QEMU)

Real operating systems running in containers or VMs.

**Best for:**

- Real application testing
- OS-specific tasks
- Benchmarks requiring actual apps

**Platforms:**

- `linux-docker` - Linux container (XFCE)
- `linux-qemu` - Linux VM with KVM
- `windows-qemu` - Windows 11 VM
- `android-qemu` - Android VM
- `macos-lume` - macOS VM (Apple Silicon)

## CLI Reference

### Image Commands

```bash
cb image create <platform>     # Create base image
cb image list                   # List all images
cb image info <name>            # Show image details
cb image clone <src> <dst>      # Clone an image
cb image delete <name>          # Delete an image
cb image shell <name>           # Interactive shell (read-only overlay)
```

### Run Commands

```bash
cb run task <path>              # Run a single task
cb run dataset <path>           # Run all tasks in dataset
cb run list                     # List all runs
cb run watch <id>               # Watch run in real-time
cb run info <id>                # Show run details
cb run logs <id>                # View run logs
cb run stop <id>                # Stop a run
```

### Interact Commands

```bash
cb interact <task>              # Interactive mode (manual testing)
cb interact <task> --oracle     # Run oracle solution
cb interact <task> --variant-id <n>  # Specific task variant
```

### Trace Commands

```bash
cb trace view <run-id>          # View traces from a run
cb trace list                   # List all traces
```

### Dataset Commands

```bash
cb dataset list                 # List available datasets
cb dataset build <run-id>       # Export training data
```

### Platform Commands

```bash
cb platform list                # List available platforms
cb status                       # Show system status
```

## Documentation

- **Full Documentation**: https://cuabench.ai/
- **Getting Started**: https://cuabench.ai/cuabench/guide/getting-started
- **CLI Reference**: https://cuabench.ai/cuabench/reference/cli-reference
- **SDK Reference**: https://cuabench.ai/cuabench/reference/sdk-reference
- **Creating Tasks**: https://cuabench.ai/cuabench/guide/creating-tasks

## Examples

See the `tasks/` directory for example tasks and the documentation for detailed guides.

## License

Licensed under the Apache License 2.0. See [LICENSE](LICENSE) for details.
