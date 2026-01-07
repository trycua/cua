# CLAUDE quick reference for cua-bench environments

cua-bench is a toolkit for building computer-use RL environments and benchmarks. A task is a small Python module that defines:

- tasks_config: generates a list of Task objects (what to run)
- setup_task: creates the sandbox and UI per task
- solve_task (optional): an automated baseline to complete the task
- evaluate_task: computes the reward(s)

This file is auto-loaded by Claude Code to give you immediate context when you work inside this template.

## Decorator cheat sheet
- @cb.tasks_config(split="train")
  - Returns list[cb.Task]
  - Each Task can include: description: str, metadata: dict
  - Typical steps: simply return an array of tasks
- @cb.setup_task(split="train")
  - Called once per task instance
  - Typical steps: create sandbox, launch window(s), preload data
- @cb.solve_task(split="train") (optional)
  - Provide a baseline/heuristic solution using env.bot helpers or manually calling env.step(action)
- @cb.evaluate_task(split="train")
  - Return list[float] rewards, e.g. [1.0] if success else [0.0]

## Common env APIs (minimal)
- env.create_sandbox(provider, setup_config)
  - Example: provider="native" or "simulated"
  - setup_config keys: os_type (e.g. "linux", "win11", "macos"), width, height, background
- env.launch_window(html=..., title=..., width=..., height=...)
  - Returns a window id (pid) used for later operations
  - HTML without <html> tags will be rendered in the body of a tailwind HTML template
- env.execute_javascript(pid, js_str)
  - Evaluate custom JS in the launched webview; returns the value
- env.bot.click_element(pid, css_selector)
  - Minimal automation helper; see env.bot for more actions

## OS targets
- Desktop examples: linux, win11, win10, macos
- Mobile examples (when using provider="simulated" with mobile profiles): ios, android

## Starter pattern
- main.py: entry point with cua-bench decorators
  - tasks_config varies OS and button_text metadata
  - setup_task creates a sandbox + small webview
  - solve_task clicks a button
  - evaluate_task checks a JS flag window.__submitted
- gui/index.html: minimalist page for the example task

## Tips for modifying this starter
- Keep tasks_config fast and deterministic (pure function)
- Put runtime work in setup_task/solve_task/evaluate_task
- Use metadata to parameterize per-task variants (e.g. os_type, ids)
- Avoid global mutable state except for small handles (e.g. window ids)
- Prefer per-task state over global state using env.execute_javascript and window variables
- Prefer normalized, minimal outputs and clear rewards

## Working with Claude
- Claude can quickly transform this starter into your target prompt (e.g., “Modify this starter to be a 2048 game”).
- Provide: target behavior, UI changes (HTML/CSS/JS), evaluation logic, and any required assets.
- Use concise diffs and keep main.py small; push large data or UI assets into gui/.

## Code style
- Python ≥ 3.11, type hints preferred
- Keep functions small and focused
- No heavy comments required here; favor readable code and simple names
