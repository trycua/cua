# Cua Bench UI

> [!WARNING]
> **Deprecated:** `cua-bench-ui` is no longer updated and will not receive fixes. Its tests, builds and publishing are switched off. Existing releases stay on PyPI, so current installs keep working; [cua-bench](../../cua-bench/README.md) still uses the last release. See [Migrate from deprecated packages](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages).


Lightweight webUI window controller for Cua bench environments using pywebview

## Usage

```python
from bench_ui import launch_window, get_element_rect, execute_javascript

# Launch a window with inline HTML content
pid = launch_window(html="<html><body><h1>Hello</h1></body></html>")

# Get element rect in screen space
rect = get_element_rect(pid, "h1", space="screen")
print(rect)

# Execute arbitrary JavaScript
text = execute_javascript(pid, "document.querySelector('h1')?.textContent")
print(text)
```

## Installation

```bash
pip install cua-bench-ui
```
