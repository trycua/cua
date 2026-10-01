# Sandbox Fleet Builder API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate the Python sandbox SDK and its tests from direct construction of builder-enabled Fleet records to the generated Fleet builder API while preserving all existing public sandbox APIs.

**Architecture:** Re-export the generated builder companions from `cua_sandbox`, then construct Fleet template and pool request graphs with immutable fluent builders at their existing boundaries. Fleet records that do not expose generated builders remain constructor-based. A focused AST contract test prevents regressions to direct constructors for builder-enabled records.

**Tech Stack:** Python 3.11-3.13, pytest, Ruff, generated UniFFI `fleet_sdk` Python bindings.

## Global Constraints

- Preserve all existing `cua_sandbox` public names, signatures, and returned Fleet record types.
- Add builder exports without removing legacy record exports or constructor compatibility.
- Pin the sandbox SDK to the builder-enabled `cua-fleet==0.1.7` release.
- Migrate all direct calls to builder-enabled Fleet records under `libs/python/cua-sandbox/cua_sandbox` and `libs/python/cua-sandbox/tests`.
- Leave `CreateClaimRequest`, `ClaimSpec`, `HttpHeader`, `HttpRequest`, `HttpResponse`, `CyclopsConfiguration`, `CyclopsCredentials`, and Fleet `Sandbox` constructor calls unchanged because the generated SDK does not provide builders for them.
- Do not edit generated files under `libs/fleet/sdk-bindings`.
- Do not create commits unless the user explicitly requests them.

---

### Task 1: Lock the Builder Usage Contract

**Files:**
- Create: `libs/python/cua-sandbox/tests/test_fleet_builder_usage.py`

**Interfaces:**
- Consumes: Python source trees `libs/python/cua-sandbox/cua_sandbox` and `libs/python/cua-sandbox/tests`.
- Produces: `test_builder_enabled_fleet_records_use_generated_builders()`, which rejects direct calls to the seven builder-enabled Fleet record classes. The detector must resolve `from ... import ... as ...` aliases and `import fleet_sdk as ...` or `import cua_sandbox as ...` attribute calls, with focused snippet tests for each import style.

- [ ] **Step 1: Write the failing source contract test**

```python
from __future__ import annotations

import ast
from pathlib import Path


PACKAGE_ROOT = Path(__file__).parents[1]
BUILDER_ENABLED_RECORDS = {
    "CreatePoolRequest",
    "CreateTemplateRequest",
    "OsGymSandboxTemplateSpec",
    "OsGymSandboxWarmPoolSpec",
    "SandboxService",
    "SandboxTemplateRef",
    "VmTemplate",
}


def test_builder_enabled_fleet_records_use_generated_builders() -> None:
    violations: list[str] = []
    source_roots = (PACKAGE_ROOT / "cua_sandbox", PACKAGE_ROOT / "tests")

    for source_root in source_roots:
        for path in source_root.rglob("*.py"):
            tree = ast.parse(path.read_text(), filename=str(path))
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
                    continue
                if node.func.id in BUILDER_ENABLED_RECORDS:
                    relative_path = path.relative_to(PACKAGE_ROOT)
                    violations.append(f"{relative_path}:{node.lineno}: {node.func.id}")

    assert violations == [], "Direct Fleet record constructors remain:\n" + "\n".join(violations)
```

- [ ] **Step 2: Run the contract test and verify RED**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_fleet_builder_usage.py -q`

Expected: FAIL listing direct constructor calls in `cua_sandbox/transport/fleet_cloud.py` and `tests/test_pool.py`.

---

### Task 2: Export Fleet Builders Through Cua Sandbox

**Files:**
- Modify: `libs/python/cua-sandbox/pyproject.toml`
- Modify: `libs/python/cua-sandbox/uv.lock`
- Modify: `libs/python/cua-sandbox/cua_sandbox/__init__.py`
- Modify: `libs/python/cua-sandbox/tests/test_pool.py`
- Modify: `libs/python/cua-sandbox/tests/test_fleet_sdk_packaging.py`
- Modify: `libs/python/cua-sandbox/tests/test_fleet_sdk_distribution.py`

**Interfaces:**
- Consumes: generated `fleet_sdk` builder classes from `cua-fleet==0.1.7`.
- Produces: public imports named `CreatePoolRequestBuilder`, `CreateTemplateRequestBuilder`, `OsGymSandboxTemplateSpecBuilder`, `OsGymSandboxWarmPoolSpecBuilder`, `SandboxServiceBuilder`, `SandboxTemplateRefBuilder`, and `VmTemplateBuilder` from `cua_sandbox`.

- [ ] **Step 1: Add a failing public export test**

Extend the `from cua_sandbox import (...)` list in `tests/test_pool.py` with all seven builder names, then add:

```python
def test_public_pool_schema_exports_generated_builders() -> None:
    service = SandboxServiceBuilder().name("server").target_port(8000).build()
    vm_template = (
        VmTemplateBuilder()
        .container_disk_image("registry.example/workspace:latest")
        .services([service])
        .build()
    )
    template_request = (
        CreateTemplateRequestBuilder()
        .namespace("default")
        .name("workspace")
        .spec(OsGymSandboxTemplateSpecBuilder().vm_template(vm_template).build())
        .build()
    )
    pool_request = (
        CreatePoolRequestBuilder()
        .namespace("default")
        .spec(
            OsGymSandboxWarmPoolSpecBuilder()
            .replicas(1)
            .sandbox_template_ref(SandboxTemplateRefBuilder().name("workspace").build())
            .build()
        )
        .build()
    )

    assert template_request.spec.vm_template.services == [service]
    assert pool_request.spec.sandbox_template_ref.name == "workspace"
```

- [ ] **Step 2: Run the export test and verify RED**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_pool.py::test_public_pool_schema_exports_generated_builders -q`

Expected: collection ERROR because the builder names are not exported from `cua_sandbox`.

- [ ] **Step 3: Pin the builder-enabled Fleet wheel**

Change `cua-fleet==0.0.10` to `cua-fleet==0.1.7` in `pyproject.toml`, then run:

Run: `uv lock --project libs/python/cua-sandbox --upgrade-package cua-fleet`

Expected: `libs/python/cua-sandbox/uv.lock` resolves `cua-fleet` 0.1.7. Remove the unrelated hard-coded `cua-sandbox` project-version assertion from the Fleet packaging test so releases do not require incidental edits. Run the packaging test before updating its assertions to observe RED, then update its expected version to `0.1.7` and expected registry to `https://wheels.cua.ai/simple`, and rerun it to GREEN. Run `test_fleet_sdk_distribution.py` before updating its version assertion to observe RED, then change the expected distribution version from `0.0.7` to `0.1.7` and rerun it to GREEN.

- [ ] **Step 4: Re-export all seven builders**

Add these names to the existing `from fleet_sdk import (...)` blocks in `cua_sandbox/__init__.py`, and add the same names as strings adjacent to their record names in `__all__`:

```python
CreatePoolRequestBuilder
CreateTemplateRequestBuilder
OsGymSandboxTemplateSpecBuilder
OsGymSandboxWarmPoolSpecBuilder
SandboxServiceBuilder
SandboxTemplateRefBuilder
VmTemplateBuilder
```

- [ ] **Step 5: Run the export test and verify GREEN**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_pool.py::test_public_pool_schema_exports_generated_builders -q`

Expected: PASS.

---

### Task 3: Build Fleet Cloud Requests With Builders

**Files:**
- Modify: `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py`

**Interfaces:**
- Consumes: the seven generated builder classes and existing `PreservedJson`/`ServiceProtocol` values.
- Produces: `_FleetCloudTransport._template_request() -> CreateTemplateRequest` and `_FleetCloudTransport._pool_request() -> CreatePoolRequest` with unchanged record values.

- [ ] **Step 1: Replace builder-enabled imports**

Keep request record types used in annotations and replace the other builder-enabled record imports with:

```python
CreatePoolRequestBuilder
CreateTemplateRequestBuilder
OsGymSandboxTemplateSpecBuilder
OsGymSandboxWarmPoolSpecBuilder
SandboxServiceBuilder
SandboxTemplateRefBuilder
VmTemplateBuilder
```

- [ ] **Step 2: Build service records fluently**

```python
services = [
    SandboxServiceBuilder()
    .name(name)
    .target_port(port)
    .protocol(ServiceProtocol.TCP)
    .build()
    for name, port in service_ports.items()
]
```

- [ ] **Step 3: Build the VM, template spec, and template request**

```python
vm_template_builder = (
    VmTemplateBuilder()
    .container_disk_image(self._image._registry)
    .image_pull_secret("ecr-credentials")
    .probes(
        PreservedJson.from_json(
            json.dumps({"readinessProbe": {"tcpSocket": {"port": 8000}}})
        )
    )
    .services(services)
)
if self._cpu is not None:
    vm_template_builder = vm_template_builder.cpu_cores(self._cpu)
if self._memory_mb is not None:
    vm_template_builder = vm_template_builder.memory(f"{self._memory_mb}Mi")

template_spec = OsGymSandboxTemplateSpecBuilder().vm_template(vm_template_builder.build()).build()
return (
    CreateTemplateRequestBuilder()
    .namespace(self._name)
    .name(self._name)
    .spec(template_spec)
    .build()
)
```

Omit optional setters when the existing value is `None`; generated records still contain `None` for those fields.

- [ ] **Step 4: Build the warm-pool spec and request**

```python
def _pool_request(self) -> CreatePoolRequest:
    template_ref = SandboxTemplateRefBuilder().name(self._name).build()
    pool_spec = (
        OsGymSandboxWarmPoolSpecBuilder()
        .replicas(self._replicas)
        .sandbox_template_ref(template_ref)
        .build()
    )
    return CreatePoolRequestBuilder().namespace(self._name).spec(pool_spec).build()
```

- [ ] **Step 5: Run focused Fleet cloud tests**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_fleet_cloud_client.py libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py -q`

Expected: PASS with request fields unchanged.

---

### Task 4: Migrate Sandbox Test Fixtures to Builders

**Files:**
- Modify: `libs/python/cua-sandbox/tests/test_pool.py`

**Interfaces:**
- Consumes: builder exports added to `cua_sandbox` in Task 2.
- Produces: `pool_request()` and `template_request()` fixtures plus the custom VM fixture using builders while preserving each assertion's record identity and values.

- [ ] **Step 1: Convert `pool_request()`**

```python
def pool_request(
    *,
    name: str = "foo",
    template_name: str | None = None,
    replicas: int = 1,
) -> CreatePoolRequest:
    template_ref = SandboxTemplateRefBuilder().name(template_name or name).build()
    spec = (
        OsGymSandboxWarmPoolSpecBuilder()
        .replicas(replicas)
        .sandbox_template_ref(template_ref)
        .build()
    )
    return CreatePoolRequestBuilder().namespace(name).spec(spec).build()
```

- [ ] **Step 2: Convert `template_request()`**

```python
def template_request(
    *,
    name: str = "foo",
    image: str = "example:latest",
    services: dict[str, int] | None = None,
    vm_template: VmTemplate | None = None,
) -> CreateTemplateRequest:
    if vm_template is None:
        built_services = [
            SandboxServiceBuilder()
            .name(service_name)
            .target_port(port)
            .protocol(ServiceProtocol.TCP)
            .build()
            for service_name, port in (services or {"server": 8000}).items()
        ]
        vm_template = (
            VmTemplateBuilder()
            .container_disk_image(image)
            .image_pull_secret("ecr-credentials")
            .services(built_services)
            .build()
        )

    spec = OsGymSandboxTemplateSpecBuilder().vm_template(vm_template).build()
    return CreateTemplateRequestBuilder().namespace(name).name(name).spec(spec).build()
```

- [ ] **Step 3: Convert the custom VM fixture**

```python
service = (
    SandboxServiceBuilder()
    .name("server")
    .target_port(8000)
    .protocol(ServiceProtocol.TCP)
    .build()
)
vm_template = (
    VmTemplateBuilder()
    .container_disk_image("registry.example/workspace:latest")
    .runtime(RuntimeKind.KUBEVIRT)
    .image_pull_secret("workspace-pull")
    .cpu_cores(10)
    .memory("20Gi")
    .firmware(Firmware.EFI)
    .services([service])
    .build()
)
request = template_request(vm_template=vm_template)
```

- [ ] **Step 4: Run the builder usage contract and pool tests**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_fleet_builder_usage.py libs/python/cua-sandbox/tests/test_pool.py -q`

Expected: PASS with no direct builder-enabled Fleet constructors reported.

---

### Task 5: Verify Packaging and Code Quality

**Files:**
- Verify: `libs/python/cua-sandbox/cua_sandbox/__init__.py`
- Verify: `libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py`
- Verify: `libs/python/cua-sandbox/tests/test_fleet_builder_usage.py`
- Verify: `libs/python/cua-sandbox/tests/test_pool.py`

**Interfaces:**
- Consumes: completed builder migration.
- Produces: evidence that the builder-enabled Fleet distribution is importable, sandbox packaging remains valid, and edited Python passes lint.

- [ ] **Step 1: Run Fleet SDK distribution and packaging tests**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_fleet_sdk_distribution.py libs/python/cua-sandbox/tests/test_fleet_sdk_packaging.py -q`

Expected: PASS.

- [ ] **Step 2: Run the focused sandbox Fleet suite**

Run: `uv run --project libs/python/cua-sandbox pytest libs/python/cua-sandbox/tests/test_fleet_builder_usage.py libs/python/cua-sandbox/tests/test_pool.py libs/python/cua-sandbox/tests/test_fleet_cloud_client.py libs/python/cua-sandbox/tests/test_fleet_cloud_transport.py libs/python/cua-sandbox/tests/test_fleet_transport.py -q`

Expected: PASS.

- [ ] **Step 3: Run Ruff on edited files**

Run: `uv run --project libs/python/cua-sandbox --extra dev ruff check libs/python/cua-sandbox/cua_sandbox/__init__.py libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py libs/python/cua-sandbox/tests/test_fleet_builder_usage.py libs/python/cua-sandbox/tests/test_pool.py`

Expected: PASS with no diagnostics.

- [ ] **Step 4: Smoke-test the release build and public dependency resolution**

In a temporary copy of `libs/python/cua-sandbox`, run `uvx --from pdm==2.20.1 pdm lock` and `uvx --from pdm==2.20.1 pdm build`. Create a clean virtual environment, install the built wheel using the default PyPI index, and verify `fleet_sdk.CreatePoolRequestBuilder` imports successfully.

Expected: PDM locks and builds successfully; the clean install resolves `cua-fleet==0.1.7` from PyPI and exposes the builder API.

- [ ] **Step 5: Review the final diff**

Run: `git diff --check && git diff -- libs/python/cua-sandbox/cua_sandbox/__init__.py libs/python/cua-sandbox/cua_sandbox/transport/fleet_cloud.py libs/python/cua-sandbox/tests/test_fleet_builder_usage.py libs/python/cua-sandbox/tests/test_pool.py`

Expected: `git diff --check` exits 0; the diff contains only builder exports, builder construction, the source contract, and fixture migration.
