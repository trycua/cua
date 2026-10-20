# cua-sandbox

Sandboxed VM and container environments with a unified Python API. A thin
wrapper over the [cua SDK](../../cua/README.md) (`cua>=0.2.0`, Rust core), which
handles local runtimes and the spacesd client.

```bash
pip install cua-sandbox
# or: pip install "cua[sandbox]"  (then: from cua import Sandbox, Image)
```

The data plane (spacesd and local runtimes) goes through the `cua` SDK.

| Backend | Implementation |
|---|---|
| Local containers (Docker/Podman, gVisor when available), QEMU, Lume | `cua` SDK (cua-vmm), zero pre-setup |
| Direct: any reachable cua-spacesd | `Sandbox.connect(url=..., token=...)` |
| Tart, Hyper-V, Android emulator, OSWorld | Legacy Python adapters |

For typed desktop control through `sb.driver.connect()`, install the optional
Driver SDK:

```bash
pip install 'cua-sandbox[driver]'
```

The `driver` extra pins `cua-driver==0.37.0`, which provides the typed-window API
and the remote channel bridge. It requires that version to be published for your
platform.

```python
from cua_driver import GetScreenSizeInput


async def observe_guest(sb):
    async with sb.driver.connect() as driver:
        # The generated cua_driver.CuaDriver, not an MCP facade.
        return await driver.get_screen_size(GetScreenSizeInput(session=None))
```

### Carriers

`sb.driver.connect()` rides **cua-spacesd**: the typed-envelope MCP extension
on the spacesd's `/mcp`, sent through the cua SDK's env client with the
sandbox's own endpoint and credentials. It works for direct
(`Sandbox.connect(url=...)`) and local sandboxes. Explicit form:
`connect(service="env", transport="mcp")`. Named Driver services were a Cua
Cloud feature and are gone.

The connection initializes the session and verifies
`capabilities.experimental["ai.cua.driver.envelopes"].version == 1` before
opening a receiver, so a tools-only endpoint fails before any desktop action.
The connection keeps the receiver's generation, host-selected permissions and
cancellation. It does not reconnect or replay actions. On exit the SDK attempts
bounded receiver and MCP-session cleanup; an unconfirmed cleanup only warns.
Shell, files, terminals and the other Sandbox interfaces are unaffected.

See the [wire contract](../../cua-driver/docs/mcp-envelope-carrier.md) for limits.

## Any image, locally

Cua Cloud has ended; use a local sandbox or your own computer. Three separate
choices, each optional:

- `on`: `"local"` (`on="cloud"` and `local=False` raise: Cua Cloud has ended).
- `kind`: `"auto"`, `"container"` or `"vm"`.
- `runtime`: the engine. `"auto"`, or locally `gvisor`/`runc` (containers)
  and `qemu`/`lume` (VMs). A combination
  that does not exist raises `InvalidPlacement`, listing the valid values.

Unset values come from `CUA_DEFAULT_ON`/`CUA_DEFAULT_KIND`/`CUA_DEFAULT_RUNTIME`,
then `~/.cua/config.toml`, then
`local`/`auto`/`auto`.

```python
from cua_sandbox import Image, Sandbox, http

async with Sandbox.ephemeral(
    Image.from_registry("python:3.12-slim"),
    command=["python", "-m", "http.server", "8000"],  # replaces the entrypoint
    env={"FOO": "bar"},
    services={"web": 8000},              # named guest ports
    wait_for=http("web", "/"),           # or tcp("web"), or a list
    on="local",                          # the default
) as sb:
    r = await sb.service("web").request("GET", "/")
    url = await sb.service("web").url()           # usable from this machine
    share = await sb.public_url("web", ttl=3600)  # shareable, expires
    async with sb.tunnel.forward(8000) as t:      # loopback port to the guest port
        print(t.url)
    info = await sb.info()  # status, location, kind, runtime, services, expires_at
    print(sb.id)            # what Sandbox.connect(id) and Sandbox.delete(id) take
```

- `public_url` is a loopback URL with its own token, served by the cua daemon
  (started on demand; `CUA_BIN` names the CLI). Revoke it with
  `await sb.revoke_public_url(share)`.
- Status is `provisioning`, `starting`, `ready` or `stopped`. Provider internals
  (such as the local backend) are in `info.provider_details`.

Guides: [Sandboxes](https://cua.ai/docs/cua-sdk),
[quickstart](https://cua.ai/docs/cua-sdk/quickstart),
[services](https://cua.ai/docs/cua-sdk/guides/services),
[lifecycle](https://cua.ai/docs/cua-sdk/guides/lifecycle).

## Images

`Image.linux()`, `Image.windows()` and `Image.macos()` are the canonical images
`ghcr.io/trycua/linux:24.04`, `ghcr.io/trycua/windows:2022` and
`ghcr.io/trycua/macos:26` (`Image.macos("15")` for Sequoia). The SDK picks the
variant each backend runs: the rootfs for containers, the `-disk`
containerDisk for VMs (`kind="vm"`), Lume on a Mac. Override one with
`CUA_IMAGE_LINUX`, `CUA_IMAGE_WINDOWS` or `CUA_IMAGE_MACOS`. The canonical
images ship cua-spacesd.

Any registry image works: `Image.from_registry("python:3.12-slim")`. A private
one takes credentials, used for the pull (never logged or saved in `~/.cua`):

```python
from cua_sandbox import Image, RegistrySecret

img = Image.from_registry("ghcr.io/acme/app:1", secret=RegistrySecret.from_env())
# or RegistrySecret("user", "token"), or RegistrySecret.aws_ecr(region="us-east-1")
```

Layers (`apt_install`, `pip_install`, `uv_install`, `run`, `copy`, `env`) apply
at boot, built on the registry image as base and cached by content. See
[Build an image](https://cua.ai/docs/cua-sdk/guides/images) and
[private registries](https://cua.ai/docs/cua-sdk/guides/private-registries).

## MCP servers

`sb.mcp(service)` connects the official MCP Python SDK to an MCP server the
sandbox serves, locally or in your own cloud, with no cua-spacesd:

```bash
pip install 'cua-sandbox[mcp]'
```

```python
async with sb.mcp("mcp") as client:
    tools = await client.list_tools()
    result = await client.call_tool("add", {"a": 2, "b": 3})

config = await sb.mcp_config("mcp")  # {"url": ..., "headers": {...}} for any MCP client
```

The headers can carry a short-lived bearer: fetch a fresh config per connection.
See [MCP](https://cua.ai/docs/cua-sdk/guides/mcp).

## Sidecars

Extra containers share the sandbox's network namespace, so the sandbox reaches
them on `localhost` and `services=` can name their ports:

```python
from cua_sandbox import Container, Image, Sandbox

async with Sandbox.ephemeral(
    Image.from_registry("python:3.12-slim"),
    command=["sleep", "infinity"],
    sidecars=[Container("redis:7-alpine", ports=[6379], name="db")],
    services={"db": 6379},
    on="local",
    runtime="runc",  # local sidecars need runc (gVisor containers cannot share a network namespace)
) as sb:
    await sb.shell.run("python -c \"import socket; socket.create_connection(('db', 6379))\"")
```

Sidecars are addressed by name: the sandbox reaches `db:6379` and a
sidecar reaches the sandbox at `main`. With sidecars, the service names `main`,
`sidecars` and `sc` are reserved. Local sidecars need `runtime="runc"`, and
local VM sandboxes refuse sidecars. See
[Sidecars](https://cua.ai/docs/cua-sdk/guides/services#sidecars).

## Ephemeral sandbox

Created on enter, destroyed on exit.

```python
from cua_sandbox import Image, Sandbox

async with Sandbox.ephemeral(Image.linux()) as sb:
    await sb.shell.run("uname -a")
    await sb.screenshot()
```

## Persistent sandbox

Provision a new sandbox that stays alive after your script exits.

```python
from cua_sandbox import Image, Sandbox

sb = await Sandbox.create(Image.linux())
await sb.shell.run("uname -a")
print(sb.id)  # save this to reconnect later: Sandbox.connect(sb.id)
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

# context manager: disconnects on exit, the sandbox keeps running
async with Sandbox.connect("my-sandbox") as sb:
    await sb.shell.run("whoami")
```

Attach to any reachable cua-spacesd with
`Sandbox.connect(url="http://host:3211", token=...)`.

## Destroy a sandbox

```python
await sb.destroy()  # disconnect + permanently delete
```

## Local VM

Spins up a local VM using QEMU or Lume, destroyed on exit.

```python
from cua_sandbox import Image, Sandbox

async with Sandbox.ephemeral(Image.linux(), on="local", kind="vm") as sb:
    await sb.shell.run("uname -a")
```

`cua runtime doctor` shows which local backends this host has.

## Local machine

cua-sandbox only controls sandboxes. To control the local machine, use cua-driver (its SDK or MCP server).

## Upgrading from 0.8

0.9 runs on the cua SDK (`cua>=0.2.0`). `Sandbox.create` now runs locally
unless you pass `local=False`. The `cua_sandbox.localhost` module, `Localhost`,
and the `computer_server`, `http`, `local` and `websocket` transports are
removed; control the local machine with cua-driver instead.

## Guest services and cua-spacesd

Sandboxes are daemon-agnostic: readiness is the provider's "running" plus your
`wait_for` probes, and nothing assumes a guest agent. The computer interfaces
(`screen`, `mouse`, `shell`, `files`, ...) use cua-spacesd (port 3211) when
the image has it and raise `SpacesdNotAvailable` otherwise. `await sb.spacesd()`
returns the SDK's typed `SpacesdClient` and is optional.

Cua Cloud has ended; see [Cua Cloud has ended](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended). `on="cloud"`, `local=False` and `pool=` raise that error.
