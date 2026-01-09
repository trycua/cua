# cua-bench Task Development Guide

## Overview

cua-bench creates RL environments for training computer-use agents. Each task defines:

- Task variants (different scenarios from one template)
- Setup logic (launch GUI or apps)
- Evaluation logic (check if task was completed)
- Oracle solution (reference implementation)

## Two Approaches to Creating Tasks

### Universal GUI (Recommended)

Create custom HTML/CSS/JS interfaces that work across all platforms. This is the **default approach** for most tasks.

**Use this when:**

- Building custom games, tools, or interfaces
- Creating simulated versions of real apps (e.g., "a calculator app", "a color picker")
- You want cross-platform compatibility (simulated, Docker, QEMU)
- You want to support lightweight, fast iteration

**Example:** Calculator app, Minesweeper game, form-filling task, simulated email client

### Native Apps (Advanced)

Use real applications (Firefox, LibreOffice, etc.) on actual operating systems via Docker/QEMU.

**Use this when:**

- Testing interactions with real applications
- The task explicitly requires a real app's specific behavior
- You need shell access for setup

**Important:** Native apps require Docker/QEMU and are slower. Always prefer Universal GUI unless you specifically need a real application.

**When asked "make a task using Firefox":** (or any other task involving a 'real app')
Ask the user to clarify:

1. Do you want a **simulated clone** using Universal GUI? (faster, recommended)
2. Do you want a task using **real Firefox** on a native OS? (requires Docker/QEMU)

## Task Structure

Every task has a `main.py` file with 4 components:

### 1. Task Configuration

```python
import cua_bench as cb
from pathlib import Path

@cb.tasks_config(split="train")
def load():
    return [
        cb.Task(
            description='Click the "Submit" button.',
            metadata={"button_text": "Submit"},
            computer={
                "provider": "native",  # or "simulated" for lightweight preview
                "setup_config": {
                    "os_type": "linux",  # linux, windows (native) or win11, macos (simulated)
                    "width": 1024,
                    "height": 768
                }
            }
        )
        # Add more variants with different metadata...
    ]
```

**Provider types:**

- `"native"`: Real OS environments via Docker/QEMU
  - OS types: `"linux"`, `"windows"`
- `"simulated"`: Lightweight browser-based desktop (no Docker needed)
  - Themes: `"win11"`, `"macos"`

### 2. Setup Task

Launch your GUI window or native app. **Must be async.**

```python
pid = None  # Store window handle globally

@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    # Launch a window with your HTML GUI
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text(),
        title="My App",
        width=800,
        height=600
    )

    # Initialize with JavaScript
    await session.execute_javascript(pid, "window.initApp()")
```

### 3. Evaluate Task

Check if the task was completed. **Must be async.**

```python
@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid

    if pid is None:
        return [0.0]

    # Read state from JavaScript
    submitted = await session.execute_javascript(pid, "window.__submitted")

    # Return 1.0 for success, 0.0 for failure, or partial credit (0.0-1.0)
    return [1.0] if submitted else [0.0]
```

### 4. Oracle Solution

Demonstrate how to solve the task. **Must be async.**

```python
@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid

    if pid is None:
        return

    # Click element by CSS selector
    await session.click_element(pid, "#submit-btn")
```

## Universal GUI (Your Default Tool)

Universal GUI is an Electron-like API for packaging custom HTML/CSS/JS interfaces with your tasks.

### Basic Structure

```
my_task/
├── main.py           # Task logic (decorators)
└── gui/
    └── index.html    # Your HTML/CSS/JS GUI
```

### HTML Template

```html
<!DOCTYPE html>
<html>
  <head>
    <style>
      body {
        font-family: system-ui;
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        height: 100vh;
        margin: 0;
        padding: 20px;
      }
      .button {
        padding: 10px 20px;
        font-size: 16px;
        cursor: pointer;
      }
    </style>
  </head>
  <body>
    <h1>My Task</h1>
    <button class="button" id="submit-btn">Submit</button>

    <script>
      // Store state for evaluation
      window.__submitted = false;

      // Handle interaction
      document.getElementById('submit-btn').addEventListener('click', function () {
        window.__submitted = true;
        this.textContent = 'Submitted!';
      });
    </script>
  </body>
</html>
```

### Key Patterns

**Global state**: Use `window.__varName` to expose state for evaluation

```javascript
window.__gameWon = false;
window.__currentScore = 0;
```

**Data attributes**: Use `data-*` for reliable element selection

```html
<button data-color="red" data-action="submit">Click me</button>
```

**CSS selectors**: Click elements in oracle solutions

```python
await session.click_element(pid, "#submit-btn")
await session.click_element(pid, "[data-color='red']")
await session.click_element(pid, ".submit-button")
```

### Session API Reference

The `session: cb.DesktopSession` object provides methods to interact with the environment.

#### Universal GUI Methods

```python
# Launch window - returns window PID (int | str)
pid = await session.launch_window(
    html=html_content,          # Optional[str]: HTML content to display
    url=None,                    # Optional[str]: URL to open
    folder=None,                 # Optional[str]: Local folder to serve
    title="Window Title",        # str: Window title
    x=100,                       # Optional[int]: X position
    y=100,                       # Optional[int]: Y position
    width=400,                   # int: Window width
    height=300,                  # int: Window height
    icon=None,                   # Optional[str]: Window icon path
    use_inner_size=False,        # bool: Size content area vs window
    title_bar_style="default"    # str: Title bar style
)
# Returns: int | str (window process ID)

# Execute JavaScript - returns the result
result = await session.execute_javascript(
    pid,                         # int | str: Window PID
    "window.__myVar"             # str: JavaScript code to execute
)
# Returns: Any (the JavaScript expression result)

# Click element by CSS selector - returns None
await session.click_element(
    pid,                         # int | str: Window PID
    "#button-id"                 # str: CSS selector
)
# Returns: None
```

#### Native App Methods

```python
# Run shell command - returns dict with command result
result = await session.run_command("cat ~/Desktop/file.txt")
# Returns: Dict[str, Any] with keys:
#   - "stdout": str (command output)
#   - "stderr": str (error output)
#   - "return_code": int (exit code)
#   - "success": bool (True if return_code == 0)

# Access command output
output = result["stdout"]
errors = result["stderr"]
exit_code = result["return_code"]
succeeded = result["success"]

# Read text file - returns file contents
content = await session.read_file("/path/to/file.txt")
# Returns: str

# Write text file - returns None
await session.write_file("/path/to/file.txt", "content")
# Returns: None

# Read binary file - returns bytes
data = await session.read_bytes("/path/to/file.bin")
# Returns: bytes

# Write binary file - returns None
await session.write_bytes("/path/to/file.bin", b"data")
# Returns: None

# Check if file exists - returns boolean
exists = await session.file_exists("/path/to/file.txt")
# Returns: bool

# Check if directory exists - returns boolean
exists = await session.directory_exists("/path/to/dir")
# Returns: bool

# List directory contents - returns list of filenames
files = await session.list_dir("/path/to/dir")
# Returns: list[str]
```

#### Common Patterns

```python
# Execute command and check output
result = await session.run_command("ls -la")
if result["success"]:
    print(result["stdout"])
else:
    print(f"Error: {result['stderr']}")

# Read JavaScript state
game_won = await session.execute_javascript(pid, "window.__gameWon")
score = await session.execute_javascript(pid, "window.__score")

# Click elements by different selectors
await session.click_element(pid, "#submit-btn")           # By ID
await session.click_element(pid, ".submit-button")        # By class
await session.click_element(pid, "[data-action='submit']") # By attribute
```

## Native Apps (Advanced)

For tasks requiring real applications like Firefox, LibreOffice, or OS-specific tools.

### Using Native Apps

```python
@cb.tasks_config(split="train")
def load():
    return [
        cb.Task(
            description='Open Firefox and navigate to example.com',
            computer={
                "provider": "native",  # Requires Docker/QEMU
                "setup_config": {
                    "os_type": "linux",  # or "windows"
                    "width": 1920,
                    "height": 1080
                }
            }
        )
    ]

@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    # Install application (Linux example)
    await session.run_command("sudo apt update && sudo apt install -y firefox")

    # Launch application
    await session.run_command("firefox &")
```

**Important limitations:**

- `session.run_command()` only works with native provider (not simulated)
- Requires Docker/QEMU setup
- Slower than Universal GUI
- Less deterministic than custom GUIs

**When to use:**

- WinArena-style tasks requiring real applications
- OS-specific behavior testing
- Tasks that cannot be simulated with HTML/CSS/JS

## Testing Your Tasks

### Interactive Preview

Launch a visible browser window to interact with your task:

```bash
# Preview first variant
cb interact tasks/my_task --variant-id 0

# Run with oracle solution
cb interact tasks/my_task --variant-id 0 --oracle

# Skip the interactive prompt (useful for testing)
cb interact tasks/my_task --variant-id 0 --oracle --no-wait
```

### Run with Agent

Evaluate your task with an AI agent:

```bash
# Run single task
cb run task tasks/my_task --variant-id 0 --agent cua-agent --model anthropic/claude-sonnet-4-20250514

# Run with oracle (no agent)
cb run task tasks/my_task --variant-id 0 --oracle

# Run entire dataset
cb run dataset datasets/my_dataset --agent cua-agent --model anthropic/claude-sonnet-4-20250514
```

## Complete Example: Color Picker

```python
# main.py
import cua_bench as cb
from pathlib import Path

pid = None

@cb.tasks_config(split="train")
def load():
    colors = ["red", "blue", "green", "yellow"]
    return [
        cb.Task(
            description=f'Select the {color} color from the palette.',
            metadata={"target_color": color},
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "win11",
                    "width": 512,
                    "height": 512
                }
            }
        )
        for color in colors
    ]

@cb.setup_task(split="train")
async def start(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid
    pid = await session.launch_window(
        html=(Path(__file__).parent / "gui/index.html").read_text(),
        title="Color Picker",
        width=400,
        height=300
    )

@cb.evaluate_task(split="train")
async def evaluate(task_cfg: cb.Task, session: cb.DesktopSession) -> list[float]:
    global pid
    if pid is None:
        return [0.0]

    selected = await session.execute_javascript(pid, "window.__selectedColor")
    target = task_cfg.metadata["target_color"]

    return [1.0] if selected == target else [0.0]

@cb.solve_task(split="train")
async def solve(task_cfg: cb.Task, session: cb.DesktopSession):
    global pid
    if pid is None:
        return

    target = task_cfg.metadata["target_color"]
    await session.click_element(pid, f'[data-color="{target}"]')
```

```html
<!-- gui/index.html -->
<!DOCTYPE html>
<html>
  <head>
    <style>
      body {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        height: 100vh;
        font-family: system-ui;
      }
      .palette {
        display: grid;
        grid-template-columns: repeat(2, 100px);
        gap: 10px;
      }
      .color-box {
        width: 100px;
        height: 100px;
        cursor: pointer;
        border: 3px solid #ccc;
        border-radius: 8px;
      }
      .color-box:hover {
        border-color: #666;
      }
      .color-box.selected {
        border-color: #000;
        border-width: 5px;
      }
    </style>
  </head>
  <body>
    <h1>Pick a Color</h1>
    <div class="palette">
      <div class="color-box" data-color="red" style="background: red;"></div>
      <div class="color-box" data-color="blue" style="background: blue;"></div>
      <div class="color-box" data-color="green" style="background: green;"></div>
      <div class="color-box" data-color="yellow" style="background: yellow;"></div>
    </div>

    <script>
      window.__selectedColor = null;

      document.querySelectorAll('.color-box').forEach((box) => {
        box.addEventListener('click', function () {
          document.querySelectorAll('.color-box').forEach((b) => b.classList.remove('selected'));
          this.classList.add('selected');
          window.__selectedColor = this.dataset.color;
        });
      });
    </script>
  </body>
</html>
```

## Tips & Best Practices

**Always use async**: All task functions must be `async def`

**Global window handles**: Store `pid` globally so all functions can access it

```python
pid = None  # At module level

@cb.setup_task(split="train")
async def start(...):
    global pid
    pid = await session.launch_window(...)
```

**Expose state via window globals**: Let evaluation read JavaScript state

```javascript
window.__gameWon = false;
window.__score = 0;
window.__submitted = true;
```

**Use data attributes**: Make oracle solutions robust

```html
<button data-action="submit" data-color="red">Submit</button>
```

**Prefer Universal GUI**: Start with custom HTML/CSS/JS. Only use native apps if absolutely necessary.

**Test incrementally**:

1. Start with `cb interact` to preview your GUI
2. Add `--oracle` to verify your solution works
3. Use `cb run` to evaluate with agents

**Simulated vs Native**:

- **Simulated**: Fast, no Docker, perfect for custom GUIs
- **Native**: Slower, requires Docker, needed for real apps

## Common Questions

**Q: How do I make a task using Firefox?**
A: Clarify the requirement first:

- Want a simulated browser clone? Use Universal GUI (recommended)
- Need real Firefox? Use native provider with `session.run_command()`

**Q: Can I use React/Vue/Svelte?**
A: Yes! Build your app and serve the `dist/` folder, or start a dev server and point `launch_window()` to `http://localhost:3000`

**Q: How do I debug my task?**
A: Use `cb interact tasks/my_task --variant-id 0` to open a visible browser and interact manually

**Q: What if my task needs to install packages?**
A: Use `session.run_command()` with native provider:

```python
await session.run_command("sudo apt update && sudo apt install -y <package>")
```

**Q: How do I create partial rewards?**
A: Return values between 0.0 and 1.0 from `evaluate()`:

```python
return [revealed_cells / total_cells]  # Progress-based reward
```
