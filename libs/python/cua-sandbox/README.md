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

async with Sandbox.ephemeral(
    Image.from_registry("registry.example/desktop-workspace@sha256:...")
) as sb:
    await sb.shell.run("uname -a")
    await sb.screenshot()
```

## Persistent sandbox

Provision a new sandbox that stays alive after your script exits.

```python
from cua_sandbox import Sandbox, Image

sb = await Sandbox.create(
    Image.from_registry("registry.example/desktop-workspace@sha256:...")
)
await sb.shell.run("uname -a")
print(sb.claim_name)  # Fleet lifecycle identifier; save this to reconnect later
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

Fleet sandboxes can also create time-limited, revocable public URLs for an
exposed service. Treat each URL as a bearer credential and revoke it as soon as
the recipient no longer needs access.

```python
signed_url = await sb.services.create_signed_url(
    "mcp",
    label="Customer demo",
    expires_in_seconds=3600,
)
print(signed_url.url)

active_urls = await sb.services.list_signed_urls()
await sb.services.revoke_signed_url(signed_url)
```

## Fleet pools and durable claims

For production workloads, claim from an existing pool. Supplying `pool=` never changes its configuration; `name=` names the claim, while `sb.name` is the separately bound sandbox resource.

```python
from cua_sandbox import Sandbox

sb = await Sandbox.create(
    pool="workspace",
    name="workflow-123",
    service="mcp",
    keep_alive_minutes=30,
)

reference = sb.to_dict()
await sb.disconnect()  # claim remains held

# A later process or Temporal activity re-resolves the live claim.
sb = await Sandbox.from_dict(reference)
await sb.keep_alive(minutes=30)
await sb.close()  # idempotently releases the claim
```

Fleet pool names are globally unique across accounts, so `Sandbox.create` requires an explicitly named pool for registry images: apply one with `Pool.apply(image, name=...)` and pass it as `pool=`. `Sandbox.ephemeral(image)` instead creates an isolated temporary pool under a random name and deletes it after releasing the claim, preserving teardown-by-default semantics. If a chosen pool name is already owned by another account, Fleet refuses it and the SDK raises `PoolAccessDeniedError` — pick a different name.

```python
from cua_sandbox import Image, Sandbox

image = Image.from_registry("registry.example/desktop-workspace@sha256:...")

async with Sandbox.ephemeral(
    image,
    name="job-123",
    cpu=4,
    memory_mb=4096,
    server_port=5000,
) as sb:
    await sb.shell.run("uname -a")
```

To deliberately retain warm capacity for later calls, opt in with `keep_pool=True`. It requires `name=` so later runs can find the kept pool:

```python
async with Sandbox.ephemeral(image, name="shared-pool", keep_pool=True) as sb:
    await sb.shell.run("uname -a")
```

The equivalent lower-level reusable-pool API is:

```python
from cua_sandbox import Image, Pool

pool = await Pool.apply(
    Image.from_registry("registry.example/desktop-workspace@sha256:..."),
    name="desktop-workspace",
    replicas=1,
    cpu=4,
    memory_mb=4096,
    services={"server": 8000, "mcp": 3000},
)

sb = await pool.claim(name="job-123", service="mcp")
await sb.close()
```

`Pool.claim()` is both awaitable and an async context manager, so existing scoped usage remains valid:

```python
async with pool.claim(name="job-123") as sb:
    await sb.shell.run("echo hello")
```

Instead of a static `replicas` count, a pool can scale with claim demand by
passing `autoscaling=`. The pool then grows toward `max_pool_size` while claims
are pending and shrinks back to `min_pool_size` as they are released;
`initial_pool_size` seeds a one-time warm head start at creation:

```python
from cua_sandbox import Image, Pool, WarmPoolAutoscaling

pool = await Pool.apply(
    Image.from_registry("registry.example/desktop-workspace@sha256:..."),
    name="desktop-workspace",
    cpu=4,
    memory_mb=4096,
    autoscaling=WarmPoolAutoscaling(
        min_pool_size=0,
        initial_pool_size=2,
        max_pool_size=10,
    ),
)
```

`Pool.reconcile(CreatePoolRequest(...))` and `Template.reconcile(CreateTemplateRequest(...))` remain available for advanced generated-schema configuration. The public generated builders should be used instead of constructing builder-enabled Fleet records directly.

The image must run the CUA computer-server `/cmd` API on the configured `server_port`.
Windows computer-server images continue to use the default port `8000`.

Fleet currently supports registry images, CPU, memory, replica count, claim-demand autoscaling, and named TCP services. Local image builds, layers, injected files or environment, snapshots, custom disks, unsupported regions, and provider-crossing serialization raise `NotImplementedError`.
