# cua-fleet

Fleet support distribution used by the high-level
[`cua-sandbox`](../cua-sandbox/README.md) SDK.

For customer sandbox workflows, install `cua-sandbox` from the Cua wheel index:

```bash
pip install --extra-index-url https://wheels.cua.ai/simple cua-sandbox
```

```python
from cua_sandbox import Image, Sandbox

async with Sandbox.ephemeral(Image.linux()) as sandbox:
    await sandbox.shell.run("uname -a")
```
