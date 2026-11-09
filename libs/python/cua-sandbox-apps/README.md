# cua-sandbox-apps

> [!WARNING]
> **Deprecated:** `cua-sandbox-apps` is no longer maintained by Cua and will not receive updates or fixes. Its publishing is switched off. Existing releases stay on PyPI. For sandboxes use the [`cua` SDK and CLI](https://cua.ai/docs/cua-sdk/quickstart). See [Migrate from deprecated packages](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages).


Universal app catalog and installer pipeline for CUA sandboxes.
Environment generation: discover software, create
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
