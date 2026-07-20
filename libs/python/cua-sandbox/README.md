# cua-sandbox

Sandboxed VM environments with a unified Python API. Cloud by default.

```bash
pip install cua-sandbox
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

## Fleet provider

Provision directly through the Cyclops Fleet SDK. Each Sandbox owns one
`replicas=1` pool and one claim; the bound sandbox's named `api` service routes
the existing computer-server protocol.

```python
from cyclops_sdk import connect
from cua_sandbox import FleetProvider, Image, Sandbox

sdk = connect({
    "base_url": "https://run.cua.ai",
    "oauth": {
        "token_url": "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
        "client_id": "...",
        "client_secret": "...",
    },
})
provider = FleetProvider(
    sdk=sdk,
    templates={
        "linux": {
            "containerDiskImage": "registry.example/desktop-workspace-duo@sha256:...",
            "imagePullSecret": "registry-credentials",
            "cpuCores": 4,
            "memory": "4Gi",
        },
    },
)

try:
    async with Sandbox.ephemeral(
        Image.linux(),
        name="fleet-demo",
        provider=provider,
    ) as sb:
        await sb.shell.run("uname -a")
        await sb.screenshot()
finally:
    sdk.close()
```

The caller owns the Cyclops SDK lifetime. `Sandbox.destroy()` and ephemeral
cleanup delete the claim before the pool. Fleet templates must be supplied
explicitly; `Image` install layers, snapshots, custom disk sizes, and non-default
regions are not yet supported by `FleetProvider`.
