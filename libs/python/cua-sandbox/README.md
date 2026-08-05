# cua-sandbox

Sandboxed VM environments with a unified Python API. Cloud by default.

```bash
pip install cua-sandbox
```

Fleet support is provided by the published `cua-fleet` wheel. It bundles the platform-specific `fleet_sdk` native binding.
Install from the Cua wheel index when resolving dependencies with pip:

```bash
pip install --extra-index-url https://wheels.cua.ai/simple cua-sandbox
```

## Ephemeral sandbox

Created on enter, destroyed on exit.

```python
from cua_sandbox import Sandbox, Image

async with Sandbox.ephemeral(Image.linux()) as sb:
    await sb.shell.run("uname -a")
    await sb.screenshot()
```

## Persistent sandbox

Provision a new sandbox that stays alive after your script exits.

```python
from cua_sandbox import Sandbox, Image

sb = await Sandbox.create(Image.linux())
await sb.shell.run("uname -a")
print(sb.name)  # save this to reconnect later
await sb.disconnect()
```

## Connect to existing sandbox

Attach to a sandbox that's already running. Works as a plain await or context manager.

```python
from cua_sandbox import Sandbox

# plain await
sb = await Sandbox.connect("my-sandbox")
await sb.shell.run("whoami")
await sb.disconnect()

# context manager — disconnects on exit, sandbox keeps running
async with Sandbox.connect("my-sandbox") as sb:
    await sb.shell.run("whoami")
```

## Destroy a sandbox

```python
await sb.destroy()  # disconnect + permanently delete
```

## Local VM

Spins up a local VM using QEMU or Lume, destroyed on exit.

```python
from cua_sandbox import Sandbox, Image
from cua_sandbox.runtime import QEMURuntime

async with Sandbox.ephemeral(Image.linux(), local=True, runtime=QEMURuntime()) as sb:
    await sb.shell.run("uname -a")
```

## Localhost (unsandboxed)

Direct host control — **not sandboxed**, use with caution.

```python
from cua_sandbox import Localhost

async with Localhost.connect() as host:
    await host.shell.run("echo hello")
    await host.screenshot()
```


## Cloud sandbox

Fleet is the OAuth cloud backend. Configure OAuth credentials once; Fleet uses `https://run.cua.ai` by default and can be overridden with `configure(fleet_base_url=...)` or `CUA_FLEET_BASE_URL`. The legacy API-key VM API continues to use `https://api.cua.ai`. Cloud images must use a registry reference; `expose()` declares additional Fleet services.

Fleet does not support snapshots or custom disks, and currently supports only `us-east-1`. `await sb.tunnel.forward(3000)` returns the authenticated Fleet service URL for an exposed port; it does not open a local SSH tunnel.

## Fleet templates and pools

Use a pool to keep reusable registry-image sandboxes warm. The VM shape — image, resources, exposed services — lives in a *template*; a pool only says how many replicas to keep warm and which template to use. Reconcile the template first, then the pool that references it. Reconciliation is idempotent for both: it creates the missing resource or updates the existing one with the same name. Each claim is released when the context exits, including when the block raises.

```python
from cua_sandbox import (
    CreatePoolRequest,
    CreateTemplateRequest,
    OsGymSandboxTemplateSpec,
    OsGymSandboxWarmPoolSpec,
    Pool,
    SandboxService,
    SandboxTemplateRef,
    ServiceProtocol,
    Template,
    VmTemplate,
)

await Template.reconcile(
    CreateTemplateRequest(
        namespace="foo",
        name="workspace",
        spec=OsGymSandboxTemplateSpec(
            vm_template=VmTemplate(
                container_disk_image="registry.example/workspace:latest",
                command=None,
                runtime=None,
                runtime_class_name=None,
                node_selector=None,
                tolerations=None,
                image_pull_policy=None,
                image_pull_secret="ecr-credentials",
                cpu_cores=4,
                memory="8Gi",
                firmware=None,
                probes=None,
                services=[
                    SandboxService(name="server", target_port=8000, protocol=ServiceProtocol.TCP),
                    SandboxService(name="mcp", target_port=3000, protocol=ServiceProtocol.TCP),
                ],
                oidc=None,
            ),
        ),
    )
)

pool = await Pool.reconcile(
    CreatePoolRequest(
        namespace="foo",
        spec=OsGymSandboxWarmPoolSpec(
            replicas=1,
            sandbox_template_ref=SandboxTemplateRef(name="workspace"),
            autoscaling=None,
        ),
    )
)

async with pool.claim() as sb:
    result = await sb.shell.run("echo hello")

    # Requests use the same authenticated Fleet claim.
    response = await sb.services.request(
        "mcp", method="POST", path="/mcp", json={"jsonrpc": "2.0", "method": "tools/list", "id": 1}
    )
    response.raise_for_status()
```

For scripts that use the synchronous facade:

```python
from cua_sandbox.sync import Pool, Template

Template.reconcile(template_request)  # same CreateTemplateRequest as above
pool = Pool.reconcile(pool_request)  # same CreatePoolRequest as above
with pool.claim() as sb:
    result = sb.shell.run("echo hello")
```

```python
import os

import cua_sandbox as cua
from cua_sandbox import Image, Sandbox

cua.configure(
    client_id=os.environ["CUA_CLIENT_ID"],
    client_secret=os.environ["CUA_CLIENT_SECRET"],
)

async with Sandbox.ephemeral(
    Image.from_registry("registry.example/desktop-workspace@sha256:...").expose(3000)
) as sb:
    await sb.shell.run("uname -a")
```
