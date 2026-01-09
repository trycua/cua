# cua-bench environment scaffolding guide

## Overview

cua-bench creates computer-use RL environments. Structure:

- `main.py`: Python decorators for task logic
- `gui/`: HTML/CSS/JS for UI (auto-includes Tailwind)

### @cb.tasks_config

Returns `list[cb.Task]` with description and metadata. The description is what the AI agent hears ("Play 2048", "Book a hotel"). Metadata contains task parameters (difficulty, game size, etc.).

```python
return [cb.Task(description="Play 2048", metadata={"size": 4, "os_type": "linux"})]
```

### @cb.setup_task

Creates sandbox and launches webviews. Keep minimal - just environment setup.

```python
global pid
env.create_sandbox(provider="computer", setup_config={"os_type": "linux", "width": 800, "height": 600})
pid = env.launch_window(html=html_content, title="Game", width=400, height=400) # create webview window
```

### gui/

All game/task logic goes here. The HTML will be rendered in a Tailwind + Iconify template within a desktop webview window (Do NOT use `<html>` or `<body>` tags).

**Key patterns:**

- **Use semantic HTML with ARIA descriptions** - Use proper semantic elements (`<main>`, `<section>`, `<button>`, `<nav>`, etc.) and include `aria-label`, `aria-describedby`, and `role` attributes for accessibility and better element identification
- **Responsive design with compact padding** - Use minimal padding/margins (`p-1`, `p-2`, `gap-1`, `gap-2`) and responsive layouts that work from popup size (300x200) to full desktop. Avoid fixed heights/widths, use `min-h-0`, `overflow-auto`, and ensure critical elements remain visible when viewport shrinks
- **Global state** - Store current score in `window.__score` (0.0-1.0 range for RL)
- **AI baseline** - Implement AI strategy in JavaScript, expose via `window.__next_move()`
- **Window size** - Fill the entire window using `class="flex h-full w-full"` on the root element, and prefer all elements to be compact and responsive (the HTML is typically rendered in a desktop webview window or mobile screen with small sizes)
- **Icons** - Use `<iconify-icon icon="prefix:name"></iconify-icon>` for icons

### @cb.solve_task

Gets next action from GUI AI and executes it using `env.step` or `env.bot`:

```python
global pid
action = env.execute_javascript(pid, "window.__next_move()")
while action is not None and action["type"] != "done":
    if not action or action["type"] == "wait":
        env.step(WaitAction(seconds=1.0))
    elif action["type"] == "click_element":
        env.bot.click_element(pid, f"#{action['element_id']}") # safest way to click an element
    elif action["type"] == "click_absolute":
        env.step(ClickAction(x=action["x"], y=action["y"])) # x,y must be in screen coordinates (requires offsetting by window.screenX and window.screenY)
    elif action["type"] == "type":
        env.step(TypeAction(text=action["text"]))
    action = env.execute_javascript(pid, "window.__next_move()")
env.step(DoneAction())
```

- You can only use `env.step` or `env.bot` to execute actions that solve the task.
- You can only use `env.execute_javascript` to execute helper functions that return information about the optimal action (e.g., the target element, or the output from an AI agent that implements the optimal strategy and exposes a `window.__next_move()` function).
- Any `window.__next_move()` function in the GUI should only return the next action to take if the task is not solved.
- The `window.__next_move()` function is usually called in a loop by the `@cb.solve_task` decorated function until the task is solved.
- The `window.__next_move()` function should not execute any actions or modify the environment / state - it should only return the next action to take, which will be executed by the `env.step` or `env.bot` functions.

### @cb.evaluate_task

Returns rewards from GUI state:

```python
global pid
score = env.execute_javascript(pid, "window.__score")
return [float(score)]  # 0.0-1.0 range preferred
```

## Action Types

Available action classes for `env.step()`:

**Mouse:**

- `ClickAction(x, y)`
- `RightClickAction(x, y)`
- `DoubleClickAction(x, y)`
- `DragAction(from_x, from_y, to_x, to_y, duration=1.0)`
- `ScrollAction(direction="up|down", amount=100)`

**Keyboard:**

- `TypeAction(text="hello")`
- `KeyAction(key="Enter")`
- `HotkeyAction(keys=["ctrl", "c"])`

**Control:**

- `DoneAction()`
- `WaitAction(seconds=1.0)`

## Icons

Use iconify-icon elements for scalable vector icons:

```html
<iconify-icon icon="eva:people-outline"></iconify-icon>
<iconify-icon icon="mingcute:ad-circle-line" width="24" height="24"></iconify-icon>
<iconify-icon icon="mdi:play" class="text-blue-500" style="font-size: 2rem;"></iconify-icon>
```

- Icons are automatically processed and replaced with inline SVG
- Supports all iconify icon sets (eva, mingcute, mdi, etc.)

## Screen Size

The screen size is specified in the `setup_config` parameter of `env.create_sandbox`. Here are some common screen sizes:

```python
# --- Screen size options for desktop environments ---
StandardScreenSize = Union[
    # Standard Desktop Resolutions
    tuple[Literal[1920], Literal[1080]],  # Full HD (current default)
    tuple[Literal[1366], Literal[768]],  # HD (laptop standard)
    tuple[Literal[2560], Literal[1440]],  # 2K/QHD
    tuple[Literal[3840], Literal[2160]],  # 4K/UHD
    tuple[Literal[1280], Literal[720]],  # HD Ready
    tuple[Literal[1600], Literal[900]],  # HD+
    tuple[Literal[1920], Literal[1200]],  # WUXGA
    tuple[Literal[2560], Literal[1600]],  # WQXGA
    tuple[Literal[3440], Literal[1440]],  # Ultrawide QHD
    tuple[Literal[5120], Literal[1440]],  # Super Ultrawide
    # Mobile/Tablet Resolutions
    tuple[Literal[1024], Literal[768]],  # iPad (portrait)
    tuple[Literal[768], Literal[1024]],  # iPad (landscape)
    tuple[Literal[360], Literal[640]],  # Mobile portrait
    tuple[Literal[640], Literal[360]],  # Mobile landscape
    # Legacy Resolutions
    tuple[Literal[1024], Literal[600]],  # Netbook
    tuple[Literal[800], Literal[600]],  # SVGA
    tuple[Literal[640], Literal[480]],  # VGA
    # Additional Common Resolutions
    tuple[Literal[1440], Literal[900]],  # Custom laptop
    tuple[Literal[1680], Literal[1050]],  # WSXGA+
    tuple[Literal[1920], Literal[1440]],  # Custom 4:3 ratio
    tuple[Literal[2560], Literal[1080]],  # Ultrawide Full HD
    tuple[Literal[3440], Literal[1440]],  # Ultrawide QHD
    tuple[Literal[3840], Literal[1080]],  # Super Ultrawide Full HD
]
```

## Best Practices

- Keep `main.py` minimal - just decorators and basic logic (e.g., environment setup, task loading, etc.)
- Implement AI strategy in `gui/` JavaScript, expose via `window.__next_move()`
- Use `window.__score` for RL rewards (0.0-1.0 range)
- Parameterize variants via Task metadata (difficulty, size, OS, # of rounds)
- Avoid using `WaitAction` unless the task requires it (e.g., waiting for a page to load) or if you are waiting for the next action to be available. The `env.bot` helpers will automatically step the environment forward with actionability logic, including waiting for elements to become clickable.
- All `x,y` coordinates are in screen coordinates (0,0 is top-left of the screen). Use `window.screenX` and `window.screenY` to get the offset from the top-left of the browser viewport to the top-left of the screen.
- Pick a screen size that is appropriate for the task and environment.
