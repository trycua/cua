# cua-sandbox-apps

Universal app catalog and installer pipeline for CUA sandboxes.
Gym-Anything-style environment generation: discover software, create
install/launch scripts, then verify with screenshots in a sandbox.

## Usage

```python
from cua_sandbox import Image, Sandbox

# Build an image with an app pre-installed
image = Image.linux().app_install("godot-engine")

async with Sandbox.ephemeral(image) as sb:
    await sb.apps.launch("godot-engine")
    screenshot = await sb.screenshot()
```
