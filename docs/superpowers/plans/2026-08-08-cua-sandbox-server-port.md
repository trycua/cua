# Cua Sandbox Fleet Server Port Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `server_port` to the public asynchronous sandbox creation APIs so Fleet services, readiness probes, and server tunnels can target Linux port `5000` while preserving the existing Windows default of `8000`.

**Architecture:** The public `Sandbox.create()` and `Sandbox.ephemeral()` methods forward one validated integer through `Sandbox._create()` into `FleetCloudTransport`. The transport owns all Fleet-specific use of the value: template service generation, readiness-probe generation, exposed-port deduplication, and server tunnel routing. Existing-pool connections remain unchanged because they consume the pool's stored template.

**Tech Stack:** Python 3.11-3.13, `pytest`, `pytest-asyncio`, `ruff`, `cua-sandbox`, `cua-fleet`/UniFFI Fleet bindings.

## Global Constraints

- `server_port` defaults to exactly `8000`.
- Valid values are integers from `1` through `65535`; booleans are invalid.
- The option applies to Fleet-backed cloud creation and must not change legacy API-key or local runtime routing.
- The selected port drives the Fleet `server` Service, readiness probe, and `forward_tunnel(server_port)` behavior.
- `Image.expose(server_port)` must not create a duplicate `port-<server_port>` Service.
- Do not infer ports from image names or registry references.
- Preserve existing behavior for callers that omit `server_port`.

---

### Task 1: Make Fleet Transport Port-Aware

**Files:**
- Modify: `libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py`

**Interfaces:**
- Consumes: `FleetCloudTransport(..., server_port: int = 8000)`.
- Produces: `self._server_port`, port-aware `_template_request()`, and port-aware `forward_tunnel()`.

- [ ] **Step 1: Write failing template and validation tests**

Add these focused tests near `test_registry_image_becomes_typed_template_request`:

```python
import json


def test_custom_server_port_configures_service_probe_and_exposed_port_deduplication():
    request = FleetCloudTransport(
        image=Image.from_registry("registry.example/workspace@sha256:abc")
        .expose(5000)
        .expose(3000),
        name="demo",
        server_port=5000,
    )._template_request()

    vm_template = request.spec.vm_template
    assert [(service.name, service.target_port) for service in vm_template.services] == [
        ("server", 5000),
        ("port-3000", 3000),
    ]
    assert json.loads(vm_template.probes.to_json()) == {
        "readinessProbe": {"tcpSocket": {"port": 5000}}
    }


@pytest.mark.parametrize("server_port", [True, False, 0, -1, 65536, 5000.0, "5000"])
def test_rejects_invalid_server_port(server_port):
    with pytest.raises(ValueError, match="server_port must be an integer between 1 and 65535"):
        FleetCloudTransport(
            image=Image.from_registry("registry.example/workspace:latest"),
            name="demo",
            server_port=server_port,
        )
```

If the generated `PreservedJson` wrapper does not expose `to_json()`, use its existing public JSON accessor established by the installed `cua-fleet==0.1.8` binding; do not inspect private UniFFI handles.

- [ ] **Step 2: Write the failing tunnel-routing test**

Add beside `test_forward_tunnel_uses_named_service_url`:

```python
@pytest.mark.asyncio
async def test_forward_tunnel_uses_server_service_for_custom_server_port():
    transport = FleetCloudTransport(
        image=Image.from_registry("example:latest"), name="demo", server_port=5000
    )
    transport._provisioned = True
    transport._bound = Sandbox(
        namespace="demo", claim="claim", name="sandbox", services=["server"]
    )

    class Client:
        def service_url(self, sandbox, service):
            assert service == "server"
            return "https://run.cua.ai/api/svc/demo/sandbox-server/"

    transport._sdk = Client()
    tunnel = await transport.forward_tunnel(5000)
    assert tunnel.url == "https://run.cua.ai/api/svc/demo/sandbox-server/"
```

- [ ] **Step 3: Run the focused tests and verify RED**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q \
  tests/test_fleet_cloud_transport.py::test_custom_server_port_configures_service_probe_and_exposed_port_deduplication \
  tests/test_fleet_cloud_transport.py::test_rejects_invalid_server_port \
  tests/test_fleet_cloud_transport.py::test_forward_tunnel_uses_server_service_for_custom_server_port
```

Expected: failures because `FleetCloudTransport.__init__()` does not accept `server_port`.

- [ ] **Step 4: Implement minimal transport support**

Update the constructor and template/tunnel code:

```python
def __init__(
    self,
    *,
    image: Optional[Image],
    name: str,
    # existing arguments...
    server_port: int = 8000,
    replicas: int = 1,
    services: Mapping[str, int] | None = None,
) -> None:
    if (
        isinstance(server_port, bool)
        or not isinstance(server_port, int)
        or server_port < 1
        or server_port > 65535
    ):
        raise ValueError("server_port must be an integer between 1 and 65535")
    # existing validation...
    self._server_port = server_port
```

Use the stored value everywhere the hardcoded server port currently appears:

```python
service = "server" if sandbox_port == self._server_port else f"port-{sandbox_port}"

service_ports = self._services or {
    "server": self._server_port,
    **{
        f"port-{port}": port
        for port in self._image._ports
        if port != self._server_port
    },
}

PreservedJson.from_json(
    json.dumps({"readinessProbe": {"tcpSocket": {"port": self._server_port}}})
)
```

Do not change the named service passed to `wait_service_ready()` or `FleetTransport`; it remains `"server"`.

- [ ] **Step 5: Run the Fleet transport tests and verify GREEN**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q tests/test_fleet_cloud_transport.py
```

Expected: all Fleet transport tests pass.

- [ ] **Step 6: Commit the transport behavior**

```bash
git add \
  libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py \
  libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py
git commit -m "fix(cua-sandbox): configure Fleet server port"
```

---

### Task 2: Expose the Public Sandbox Argument

**Files:**
- Modify: `libs/python/cua-sandbox/tests/test_vm_cleanup.py`
- Modify: `libs/python/cua-sandbox/cua_sandbox/sandbox.py`

**Interfaces:**
- Consumes: transport signature `FleetCloudTransport(..., server_port: int = 8000)` from Task 1.
- Produces: `Sandbox.create(..., server_port: int = 8000)` and `Sandbox.ephemeral(..., server_port: int = 8000)`.

- [ ] **Step 1: Write failing public forwarding tests**

Add a focused test class before the existing ephemeral cleanup section. Patch `Sandbox._create()` so the tests exercise only the public wrappers:

```python
class TestFleetServerPortForwarding:
    async def test_create_forwards_server_port(self):
        sandbox = object()
        with patch.object(Sandbox, "_create", AsyncMock(return_value=sandbox)) as create:
            result = await Sandbox.create(
                Image.from_registry("registry.example/workspace:latest"),
                server_port=5000,
                telemetry_enabled=False,
            )

        assert result is sandbox
        assert create.await_args.kwargs["server_port"] == 5000

    async def test_ephemeral_forwards_server_port(self):
        sandbox = _make_sandbox(_make_cloud_transport(name="port-e2e"), ephemeral=True)
        sandbox.destroy = AsyncMock()
        with patch.object(Sandbox, "_create", AsyncMock(return_value=sandbox)) as create:
            async with Sandbox.ephemeral(
                Image.from_registry("registry.example/workspace:latest"),
                server_port=5000,
                telemetry_enabled=False,
            ):
                pass

        assert create.await_args.kwargs["server_port"] == 5000
```

Also add a `_create()` seam test that patches `cua_sandbox.sandbox.FleetCloudTransport`, returns a fake connected transport, and asserts its constructor receives `server_port=5000`. This proves the value crosses the internal factory boundary rather than stopping at the public wrapper.

- [ ] **Step 2: Run the forwarding tests and verify RED**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q tests/test_vm_cleanup.py -k server_port
```

Expected: failures because the public methods and `_create()` do not accept or forward `server_port`.

- [ ] **Step 3: Add the public and internal parameters**

Add `server_port: int = 8000` to `Sandbox.create()`, `Sandbox.ephemeral()`, and `Sandbox._create()`. Forward it at each boundary:

```python
return await cls._create(
    # existing arguments...
    server_port=server_port,
)
```

```python
transport = FleetCloudTransport(
    image=image,
    name=name or _random_name(),
    # existing arguments...
    server_port=server_port,
)
```

Validate at the start of `_create()` so invalid values fail before local, legacy-cloud, or Fleet provisioning can begin:

```python
if (
    isinstance(server_port, bool)
    or not isinstance(server_port, int)
    or server_port < 1
    or server_port > 65535
):
    raise ValueError("server_port must be an integer between 1 and 65535")
```

The value remains unused by local runtimes and legacy API-key cloud transports after validation.

- [ ] **Step 4: Update constructor docstrings**

Add this argument description to both public methods:

```text
server_port: Guest computer-server TCP port for Fleet cloud sandboxes.
    Defaults to 8000. Use 5000 for the canonical Linux desktop-workspace image.
```

Update each method's example when useful, but do not change local-runtime examples to include a Fleet-only option.

- [ ] **Step 5: Run public factory and transport tests**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q \
  tests/test_vm_cleanup.py \
  tests/test_fleet_cloud_transport.py
```

Expected: all tests pass.

- [ ] **Step 6: Commit the public API**

```bash
git add \
  libs/python/cua-sandbox/cua_sandbox/sandbox.py \
  libs/python/cua-sandbox/tests/test_vm_cleanup.py
git commit -m "feat(cua-sandbox): expose Fleet server port"
```

---

### Task 3: Document and Certify the Fix

**Files:**
- Modify: `libs/python/cua-sandbox/README.md`
- Verify: `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py`
- Verify: `libs/python/cua-sandbox/cua_sandbox/sandbox.py`

**Interfaces:**
- Consumes: `Sandbox.create(..., server_port=5000)` and `Sandbox.ephemeral(..., server_port=5000)` from Task 2.
- Produces: documented canonical Linux usage and release-ready E2E evidence.

- [ ] **Step 1: Add the canonical Linux README example**

Update the Fleet cloud example to show the explicit image contract:

```python
async with Sandbox.ephemeral(
    Image.from_registry(
        "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace@sha256:b9e74dbff4cc727c33ff4b8483bffa0860bb99213041c88f11260ac31db7628f"
    ).expose(3000),
    server_port=5000,
) as sb:
    await sb.shell.run("uname -a")
```

Add one sentence explaining that Windows computer-server images continue to use the default port `8000`.

- [ ] **Step 2: Run formatting and focused package tests**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m ruff check cua_sandbox tests
.venv/bin/python -m ruff format --check cua_sandbox tests
.venv/bin/python -m pytest -q \
  tests/test_fleet_cloud_transport.py \
  tests/test_vm_cleanup.py \
  tests/test_fleet_builder_usage.py \
  tests/test_fleet_sdk_packaging.py
```

Expected: zero lint errors, zero formatting diffs, all selected tests pass.

- [ ] **Step 3: Run the complete cua-sandbox unit suite**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q tests
```

Expected: all non-environment-gated tests pass; report any documented skips separately.

- [ ] **Step 4: Commit documentation**

```bash
git add libs/python/cua-sandbox/README.md
git commit -m "docs(cua-sandbox): document Linux Fleet server port"
```

- [ ] **Step 5: Run one live empty-namespace Linux E2E**

Using a namespace name unique to the run and a user-scoped Fleet credential, run this public API shape from the candidate commit:

```python
async with Sandbox.ephemeral(
    Image.from_registry(
        "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace@sha256:b9e74dbff4cc727c33ff4b8483bffa0860bb99213041c88f11260ac31db7628f"
    ),
    name="pr-server-port-e2e-<timestamp>",
    cpu=4,
    memory_mb=4096,
    server_port=5000,
    time_to_start=900,
    request_timeout=60,
    telemetry_enabled=False,
) as sandbox:
    width, height = await sandbox.screen.size()
    result = await sandbox.shell.run("uname -s")
    assert width > 0 and height > 0
    assert result.success
```

Verify with `kubectl` that namespace, template, pool, claim, sandbox, VMI, and pod are created in order. After context exit, verify the owned namespace and every child resource are gone. Do not revoke or alter any pre-existing managed credential; revoke only a temporary key minted for this E2E.

- [ ] **Step 6: Review the final diff and PR metadata**

Run:

```bash
git diff origin/main...HEAD --check
git status --short
git log --oneline origin/main..HEAD
```

Use PR title:

```text
feat(cua-sandbox): configure Fleet computer-server port
```

The PR body must link merged PR `#2979`, describe the live `server_port=5000` E2E, list exact test commands, and call out that the default remains `8000`.
