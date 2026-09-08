# Periodic Cua Sandbox Live Fleet E2E Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a 15-minute live Fleet smoke that independently certifies repository `main` and the latest published `cua-sandbox`, verifies explicit port `8000`, screen and shell access, detects claim leaks, and alerts Alertmanager on failure.

**Architecture:** A public-API-only Python support module owns Fleet inspection and diagnostic-only leak detection; a thin opt-in pytest test owns the live scenario. One trigger-aware GitHub Actions workflow selects source and published lanes, while a repository-side contract test locks down schedule, security, cleanup, and alerting behavior.

**Tech Stack:** Python 3.12, pytest/pytest-asyncio, `cua-sandbox`, `fleet_sdk`, httpx, GitHub Actions, Alertmanager v2 API.

## Global Constraints

- Schedule must remain `7/15 * * * *`, running at `:07`, `:22`, `:37`, and `:52` UTC.
- Scheduled runs execute both `main-source` and `published-package`; relevant pushes to `main` execute only `main-source`.
- Manual dispatch accepts `both`, `main-source`, or `published-package`; only manual dispatch may set `force_failure=true`.
- Use image `public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2`.
- Provision with `cpu=4`, `memory_mb=4096`, `server_port=8000`, `time_to_start=900`, `request_timeout=60`, and `telemetry_enabled=False`.
- Authenticate only with `CUA_CLIENT_ID`, `CUA_CLIENT_SECRET`, `CUA_FLEET_BASE_URL=https://run.cua.ai`, and the default Cyclops token endpoint.
- Do not use `CUA_API_KEY`, legacy `/api/keys`, namespace-scoped key creation, repository-private SDK helpers, or mutable image tags.
- A leaked claim is a test failure; collect sanitized diagnostics without explicit deletion.
- Upload diagnostics only on failure and never include credentials, tokens, or authorization headers.
- Pin `actions/checkout`, `actions/setup-python`, and `actions/upload-artifact` by full commit SHA.
- No nightly suite and no required pull request live check.

---

### Task 1: Add Offline Tests For Live Support Logic

**Files:**
- Create: `libs/python/cua-sandbox/tests/live/__init__.py`
- Create: `libs/python/cua-sandbox/tests/live/fleet_e2e_support.py`
- Create: `libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py`

**Interfaces:**
- Consumes: public `fleet_sdk` records and exceptions; no `cua_sandbox` private modules.
- Produces: `HttpxFleetClient`, `build_fleet_client()`, `build_namespace_name()`, `wait_claims_absent()`, `collect_resource_inventory()`, `assert_template_contract()`, and `write_summary()` for diagnostic-only live pytest cleanup verification.

- [ ] **Step 1: Create the live test package marker**

Create an empty `libs/python/cua-sandbox/tests/live/__init__.py` so support code can be imported consistently by pytest.

- [ ] **Step 2: Write failing unit tests for naming, template assertions, and cleanup polling**

Create `libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py` with fakes that do not contact live infrastructure:

```python
from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_namespace_name,
    collect_resource_inventory,
    wait_claims_absent,
)


def test_build_namespace_name_is_stable_for_each_lane_and_event_class() -> None:
    name = build_namespace_name("published-package", "workflow_dispatch")
    assert name == "cua-live-published-package-manual"
    assert len(name) <= 63


def test_assert_template_contract_accepts_server_port_8000() -> None:
    probes = SimpleNamespace(
        to_json=lambda: json.dumps(
            {"readinessProbe": {"tcpSocket": {"port": 8000}}}
        )
    )
    template = SimpleNamespace(
        spec=SimpleNamespace(
            vm_template=SimpleNamespace(
                services=[SimpleNamespace(name="server", target_port=8000)],
                probes=probes,
            )
        )
    )
    assert_template_contract(template, expected_port=8000)


def test_assert_template_contract_rejects_wrong_service_port() -> None:
    probes = SimpleNamespace(
        to_json=lambda: json.dumps(
            {"readinessProbe": {"tcpSocket": {"port": 8000}}}
        )
    )
    template = SimpleNamespace(
        spec=SimpleNamespace(
            vm_template=SimpleNamespace(
                services=[SimpleNamespace(name="server", target_port=5000)],
                probes=probes,
            )
        )
    )
    with pytest.raises(AssertionError, match="target_port"):
        assert_template_contract(template, expected_port=8000)


@pytest.mark.asyncio
async def test_collect_resource_inventory_lists_owned_resources() -> None:
    def resource(name: str):
        return SimpleNamespace(metadata=SimpleNamespace(name=name))

    class FakeClient:
        async def get_namespace(self, name: str):
            return SimpleNamespace(name=name)

        async def list_templates(self, name: str):
            return [resource("template-a")]

        async def list_pools(self, name: str):
            return [resource("pool-a")]

        async def list_claims(self, name: str):
            return [resource("claim-a")]

    assert await collect_resource_inventory(FakeClient(), "demo") == {
        "templates": ["template-a"],
        "pools": ["pool-a"],
        "claims": ["claim-a"],
    }


@pytest.mark.asyncio
async def test_wait_claims_absent_polls_until_claims_are_gone(monkeypatch) -> None:
    class StatusError(Exception):
        def __init__(self, status: int) -> None:
            self.status = status

    calls = 0

    class FakeClient:
        async def get_namespace(self, name: str):
            nonlocal calls
            calls += 1
            if calls == 1:
                return SimpleNamespace(name=name)
            raise StatusError(404)

    monkeypatch.setattr(
        "tests.live.fleet_e2e_support.is_not_found_error",
        lambda error: getattr(error, "status", None) == 404,
    )
    monkeypatch.setattr("asyncio.sleep", lambda _: _completed())

    assert await wait_claims_absent(FakeClient(), "demo", timeout=1, interval=0)
    assert calls == 2


async def _completed() -> None:
    return None
```

- [ ] **Step 3: Run the focused test and verify it fails**

Run:

```bash
cd libs/python/cua-sandbox
uv sync --group dev
.venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py
```

Expected: FAIL during collection because `tests.live.fleet_e2e_support` does not yet export the required helpers.

- [ ] **Step 4: Implement the minimal public Fleet support module**

Create `libs/python/cua-sandbox/tests/live/fleet_e2e_support.py` with these concrete interfaces:

```python
from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import re
import time
from typing import Any

import httpx
from fleet_sdk import (
    CyclopsClient,
    CyclopsConfiguration,
    CyclopsCredentials,
    HttpClient,
    HttpError,
    HttpHeader,
    HttpRequest,
    HttpResponse,
    SdkError,
)

DEFAULT_BASE_URL = "https://run.cua.ai"
DEFAULT_TOKEN_URL = (
    "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token"
)


class HttpxFleetClient(HttpClient):
    def __init__(self) -> None:
        self._client = httpx.AsyncClient(timeout=60.0)

    async def execute(self, request: HttpRequest) -> HttpResponse:
        try:
            response = await self._client.request(
                request.method,
                request.url,
                headers={header.name: header.value for header in request.headers},
                content=request.body,
            )
        except httpx.TransportError as error:
            raise HttpError.Transport(str(error)) from error
        return HttpResponse(
            status=response.status_code,
            headers=[
                HttpHeader(name=name, value=value)
                for name, value in response.headers.multi_items()
            ],
            body=response.content,
        )

    async def aclose(self) -> None:
        await self._client.aclose()


def build_namespace_name(lane: str, event_name: str) -> str:
    event_class = {"schedule": "schedule", "push": "push", "workflow_dispatch": "manual"}.get(
        event_name, "manual"
    )
    raw = f"cua-live-{lane}-{event_class}".lower()
    normalized = re.sub(r"[^a-z0-9-]+", "-", raw).strip("-")
    return normalized[:63].rstrip("-")


def build_fleet_client() -> tuple[CyclopsClient, HttpxFleetClient]:
    client_id = os.environ["CUA_CLIENT_ID"]
    client_secret = os.environ["CUA_CLIENT_SECRET"]
    http_client = HttpxFleetClient()
    configuration = CyclopsConfiguration(
        base_url=os.environ.get("CUA_FLEET_BASE_URL", DEFAULT_BASE_URL),
        token_url=os.environ.get("CUA_TOKEN_URL", DEFAULT_TOKEN_URL),
        credentials=CyclopsCredentials(client_id, client_secret),
        pool_poll_interval_ms=2000,
        pool_poll_limit=300,
        claim_poll_interval_ms=2000,
        claim_poll_limit=300,
    )
    return CyclopsClient.connect(configuration, http_client), http_client


async def wait_claims_absent(
    client: CyclopsClient,
    name: str,
    *,
    timeout: float = 180.0,
    interval: float = 5.0,
) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not await client.list_claims(name):
            return True
        await asyncio.sleep(interval)
    return not await client.list_claims(name)


async def collect_resource_inventory(
    client: CyclopsClient, name: str
) -> dict[str, list[str]]:
    templates = await client.list_templates(name)
    pools = await client.list_pools(name)
    claims = await client.list_claims(name)
    return {
        "templates": [item.metadata.name for item in templates],
        "pools": [item.metadata.name for item in pools],
        "claims": [item.metadata.name for item in claims],
    }


def assert_template_contract(template: Any, expected_port: int) -> None:
    vm_template = template.spec.vm_template
    server = next(service for service in vm_template.services if service.name == "server")
    assert server.target_port == expected_port, (
        f"server target_port={server.target_port}, expected {expected_port}"
    )
    probes = json.loads(vm_template.probes.to_json())
    observed = probes["readinessProbe"]["tcpSocket"]["port"]
    assert observed == expected_port, (
        f"readiness probe port={observed}, expected {expected_port}"
    )


def write_summary(path: Path, summary: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
```

- [ ] **Step 5: Run the focused test and verify it passes**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q tests/test_live_fleet_e2e_support.py
```

Expected: `5 passed`.

- [ ] **Step 6: Commit the offline support layer**

```bash
git add \
  libs/python/cua-sandbox/tests/live/__init__.py \
  libs/python/cua-sandbox/tests/live/fleet_e2e_support.py \
  libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py
git commit -m "test(cua-sandbox): add live Fleet E2E support"
```

---

### Task 2: Add The Opt-In Live Fleet Pytest

**Files:**
- Create: `libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py`
- Modify: `libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py`

**Interfaces:**
- Consumes: Task 1 support functions, public `cua_sandbox.Image`, `cua_sandbox.Sandbox`, and public `fleet_sdk` client methods.
- Produces: one credential-gated pytest scenario and a sanitized JSON result at `CUA_LIVE_E2E_ARTIFACT_DIR/summary.json`.

- [ ] **Step 1: Add a failing offline test for credential gating**

Append this test to `tests/test_live_fleet_e2e_support.py`:

```python
def test_live_test_requires_both_oauth_values(monkeypatch) -> None:
    monkeypatch.delenv("CUA_CLIENT_ID", raising=False)
    monkeypatch.delenv("CUA_CLIENT_SECRET", raising=False)
    from tests.live.test_fleet_ephemeral import has_oauth_credentials

    assert not has_oauth_credentials()
    monkeypatch.setenv("CUA_CLIENT_ID", "client")
    assert not has_oauth_credentials()
    monkeypatch.setenv("CUA_CLIENT_SECRET", "secret")
    assert has_oauth_credentials()
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q \
  tests/test_live_fleet_e2e_support.py::test_live_test_requires_both_oauth_values
```

Expected: FAIL because `tests.live.test_fleet_ephemeral` does not exist.

- [ ] **Step 3: Implement the live scenario**

Create `libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py`:

```python
from __future__ import annotations

from importlib.metadata import version
import os
from pathlib import Path
import time

import pytest
from cua_sandbox import Image, Sandbox

from tests.live.fleet_e2e_support import (
    assert_template_contract,
    build_fleet_client,
    build_namespace_name,
    collect_resource_inventory,
    wait_claims_absent,
    write_summary,
)

IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2"
)


def has_oauth_credentials() -> bool:
    return bool(os.environ.get("CUA_CLIENT_ID") and os.environ.get("CUA_CLIENT_SECRET"))


pytestmark = [
    pytest.mark.asyncio,
    pytest.mark.skipif(not has_oauth_credentials(), reason="Fleet OAuth credentials not set"),
]


async def test_fleet_ephemeral_live() -> None:
    lane = os.environ.get("CUA_LIVE_E2E_LANE", "local")
    namespace = os.environ.get("CUA_LIVE_E2E_NAMESPACE") or build_namespace_name(
        lane,
        os.environ.get("CUA_LIVE_E2E_EVENT", "manual"),
    )
    artifact_dir = Path(
        os.environ.get("CUA_LIVE_E2E_ARTIFACT_DIR", "/tmp/cua-live-e2e")
    )
    summary = {
        "lane": lane,
        "namespace": namespace,
        "image": IMAGE,
        "source_sha": os.environ.get("GITHUB_SHA"),
        "packages": {
            "cua-sandbox": version("cua-sandbox"),
            "cua-fleet": version("cua-fleet"),
        },
    }
    fleet, http_client = build_fleet_client()
    primary_error: BaseException | None = None

    try:
        started = time.monotonic()
        async with Sandbox.ephemeral(
            Image.from_registry(IMAGE),
            name=namespace,
            cpu=4,
            memory_mb=4096,
            server_port=8000,
            time_to_start=900,
            request_timeout=60,
            telemetry_enabled=False,
        ) as sandbox:
            summary["provision_seconds"] = time.monotonic() - started
            summary["sandbox_name"] = sandbox.name
            template = await fleet.get_template(namespace, namespace)
            assert_template_contract(template, expected_port=8000)

            width, height = await sandbox.screen.size()
            summary["screen"] = {"width": width, "height": height}
            assert (width, height) == (1024, 768)

            screenshot = await sandbox.screenshot()
            artifact_dir.mkdir(parents=True, exist_ok=True)
            (artifact_dir / "screen.png").write_bytes(screenshot)
            assert screenshot.startswith(b"\x89PNG\r\n\x1a\n")
            assert len(screenshot) > 1000

            result = await sandbox.shell.run("uname -s")
            summary["shell"] = {
                "success": result.success,
                "stdout": result.stdout.strip(),
                "stderr": result.stderr.strip(),
            }
            assert result.success
            assert result.stdout.strip() == "Linux"
    except BaseException as error:
        primary_error = error
        summary["error"] = {"type": type(error).__name__}
        raise
    finally:
        try:
            cleanup_started = time.monotonic()
            claims_absent = await wait_claims_absent(fleet, namespace)
            inventory = await collect_resource_inventory(fleet, namespace)
            expected_inventory = {"templates": [namespace], "pools": [namespace], "claims": []}
            summary["claims_absent"] = claims_absent
            summary["persistent_resources"] = inventory
            if not claims_absent:
                summary["claim_leak"] = True
                pytest.fail(f"claims remain in namespace {namespace} after Sandbox.ephemeral()")
            if inventory != expected_inventory:
                summary["unexpected_inventory"] = True
                pytest.fail(f"unexpected reconciled resource inventory: {inventory}")
            summary["cleanup_seconds"] = time.monotonic() - cleanup_started
        finally:
            write_summary(artifact_dir / "summary.json", summary)
            await http_client.aclose()
```

- [ ] **Step 4: Verify credential-free collection skips the live test**

Run:

```bash
cd libs/python/cua-sandbox
env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET \
  .venv/bin/python -m pytest -q tests/live/test_fleet_ephemeral.py
```

Expected: `1 skipped` and no network request.

- [ ] **Step 5: Run all offline live-support tests**

Run:

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q \
  tests/test_live_fleet_e2e_support.py \
  tests/live/test_fleet_ephemeral.py
```

Expected without credentials: `6 passed, 1 skipped`.

- [ ] **Step 6: Commit the live scenario**

```bash
git add \
  libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py \
  libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py
git commit -m "test(cua-sandbox): add live Fleet ephemeral smoke"
```

---

### Task 3: Add And Lock The Trigger-Aware Periodic Workflow

**Files:**
- Create: `.github/scripts/tests/test_periodic_cua_sandbox_live.py`
- Create: `.github/workflows/periodic-cua-sandbox-live.yml`
- Modify: `.github/workflows/ci-test-scripts.yml`
- Modify: `libs/python/cua-sandbox/tests/live/test_fleet_ephemeral.py`
- Test: `.github/scripts/tests/test_periodic_cua_sandbox_live.py`

**Implementation contract:** The workflow runs every 15 minutes, has one source
lane for relevant `main` pushes, and uses the selected manual lane or both
lanes. It never explicitly deletes a namespace: `Sandbox.ephemeral()` cleanup
is verified with diagnostic-only leak detection.

```yaml
on:
  schedule:
    - cron: "7/15 * * * *"
  push:
    branches: [main]
    paths:
      - "libs/python/cua-sandbox/**"
      - "libs/python/cua-fleet/**"
      - ".github/workflows/periodic-cua-sandbox-live.yml"
      - ".github/scripts/tests/test_periodic_cua_sandbox_live.py"
```

Both the `prepare` and `live` jobs carry
`if: github.repository == 'trycua/cua'`, so a fork that syncs `main` or
enables the schedule cannot run the live smoke or post a fork-originated
alert to Alertmanager.

The `prepare` job emits `main-source` for every `push`, both lanes for
`schedule`, and the requested `workflow_dispatch` lane. The contract test
extracts this shell script from parsed YAML, runs it with a temporary
`GITHUB_OUTPUT`, parses its JSON matrix, and covers push, schedule, and every
manual selection. This makes a push-to-both or ignored manual selection fail
CI.

```yaml
concurrency:
  group: periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}
  cancel-in-progress: ${{ github.event_name == 'schedule' }}
```

The event-and-lane group lets a new schedule cancel only an older schedule for
the same lane. Push and manual evidence use separate groups and are never
cancelled by a schedule.

```yaml
steps:
  - name: Check Fleet OAuth credentials
    run: |
      if [[ -z "$CUA_CLIENT_ID" || -z "$CUA_CLIENT_SECRET" ]]; then
        exit 1
      fi
  - name: Prepare isolated live test suite
    run: |
      suite_root="$(mktemp -d /tmp/cua-live-e2e-suite.XXXXXX)"
      mkdir -p "$suite_root/tests/live"
      cp libs/python/cua-sandbox/tests/__init__.py "$suite_root/tests/"
      cp libs/python/cua-sandbox/tests/live/*.py "$suite_root/tests/live/"
      echo "CUA_LIVE_E2E_TEST_ROOT=$suite_root" >> "$GITHUB_ENV"
  - name: Record installed versions
    run: |
      python - <<'PY' | tee -a "$GITHUB_OUTPUT"
      from importlib.metadata import version
      print("sandbox=" + version("cua-sandbox"))
      print("fleet=" + version("cua-fleet"))
      PY
  - name: Write controlled failure diagnostics
    if: github.event_name == 'workflow_dispatch' && inputs.force_failure
    run: |
      python - <<'PY'
      import json
      import os
      from pathlib import Path

      artifact_dir = Path(os.environ["CUA_LIVE_E2E_ARTIFACT_DIR"])
      artifact_dir.mkdir(parents=True, exist_ok=True)
      (artifact_dir / "summary.json").write_text(
          json.dumps(
              {
                  "lane": os.environ["CUA_LIVE_E2E_LANE"],
                  "namespace": os.environ["CUA_LIVE_E2E_NAMESPACE"],
                  "source_sha": os.environ.get("GITHUB_SHA"),
                  "error": {"type": "ControlledFailure"},
              },
              indent=2,
              sort_keys=True,
          )
          + "\n"
      )
      PY
  - name: Run live Fleet smoke
    run: PYTHONPATH="$CUA_LIVE_E2E_TEST_ROOT" python -m pytest -q -s "$CUA_LIVE_E2E_TEST_ROOT/tests/live/test_fleet_ephemeral.py"
```

Both lanes install their respective SDK package before this copied suite runs.
The source lane resolves `cua_sandbox` through its editable install; the
published lane resolves it from site-packages because the checkout package root
is absent from `PYTHONPATH`. The live summary records the resolved
`cua_sandbox` module origin. Forced failures create a sanitized `summary.json`
before the failure-only artifact upload; the versions step writes outputs for
the later Alertmanager payload without reading `steps.versions.outputs` inside
its own step.

The repository-side contract parses YAML with `yaml.BaseLoader`, rejects
unpinned actions and any `cleanup_namespace` or `delete_namespace` workflow
reference, executes the matrix preparation script, and verifies credentials,
isolation, diagnostics, and lane-specific Alertmanager payloads. Scripts CI
installs `pyyaml` and path-filters both the contract and workflow.

- [ ] **Validation**

```bash
python -m pytest -q .github/scripts/tests/test_periodic_cua_sandbox_live.py
python -m pytest -q .github/scripts/tests
python - <<'PY'
from pathlib import Path
import yaml
yaml.load(
    Path('.github/workflows/periodic-cua-sandbox-live.yml').read_text(),
    Loader=yaml.BaseLoader,
)
PY
actionlint .github/workflows/periodic-cua-sandbox-live.yml
git diff --check
```

Expected: the workflow contracts, scripts tests, YAML parse, actionlint, and
whitespace check pass without contacting live infrastructure.

---

### Task 4: Run Focused And Repository Validation

**Files:**
- Modify only if validation reveals task-scoped defects.

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: fresh offline evidence that the live test is safely skipped without credentials and workflow contracts are enforced by CI.

- [ ] **Step 1: Run sandbox live-support tests without credentials**

```bash
cd libs/python/cua-sandbox
env -u CUA_CLIENT_ID -u CUA_CLIENT_SECRET \
  .venv/bin/python -m pytest -q \
  tests/test_live_fleet_e2e_support.py \
  tests/live/test_fleet_ephemeral.py
```

Expected: `6 passed, 1 skipped`.

- [ ] **Step 2: Run existing Fleet regression tests**

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m pytest -q \
  tests/test_fleet_cloud_transport.py \
  tests/test_vm_cleanup.py \
  -k 'server_port or custom_services'
```

Expected: `38 passed, 32 deselected`.

- [ ] **Step 3: Run scripts CI tests**

```bash
python -m pytest -q .github/scripts/tests
```

Expected: all tests pass with no failures.

- [ ] **Step 4: Run lint and diff validation**

```bash
cd libs/python/cua-sandbox
.venv/bin/python -m ruff check \
  tests/live/fleet_e2e_support.py \
  tests/live/test_fleet_ephemeral.py \
  tests/test_live_fleet_e2e_support.py
.venv/bin/python -m ruff format --check \
  tests/live/fleet_e2e_support.py \
  tests/live/test_fleet_ephemeral.py \
  tests/test_live_fleet_e2e_support.py
cd ../../..
git diff --check
```

Expected: all commands exit `0`.

- [ ] **Step 5: Commit any validation-only corrections**

If Task 4 required scoped corrections:

```bash
git add \
  .github/scripts/tests/test_periodic_cua_sandbox_live.py \
  .github/workflows/ci-test-scripts.yml \
  .github/workflows/periodic-cua-sandbox-live.yml \
  libs/python/cua-sandbox/tests/live \
  libs/python/cua-sandbox/tests/test_live_fleet_e2e_support.py
git commit -m "test(cua-sandbox): harden periodic live E2E"
```

If no corrections were needed, do not create an empty commit.

---

### Task 5: Certify Both Live Lanes And Roll Out The Schedule

**Files:**
- No code changes unless live evidence exposes a task-scoped defect.

**Interfaces:**
- Consumes: repository secrets, the exact candidate SHA, and Tasks 1-4.
- Produces: live source and published-package evidence, cleanup confirmation, Alertmanager verification, and an enabled periodic monitor on `main`.

- [ ] **Step 1: Verify the published package contains `server_port` before enabling the published lane**

Run in an isolated environment:

```bash
CHECK_ROOT=$(mktemp -d /tmp/cua-published-check.XXXXXX)
trap 'rm -rf "$CHECK_ROOT"' EXIT
uv venv --python 3.12 "$CHECK_ROOT/venv"
uv pip install \
  --python "$CHECK_ROOT/venv/bin/python" \
  --index https://wheels.cua.ai/simple \
  --default-index https://pypi.org/simple \
  --upgrade cua-sandbox
"$CHECK_ROOT/venv/bin/python" - <<'PY'
import inspect
from cua_sandbox import Sandbox
assert "server_port" in inspect.signature(Sandbox.ephemeral).parameters
PY
```

Expected: exit `0`. If it fails, publish the `cua-sandbox` release containing PR #2997 before enabling scheduled alerts; do not weaken the live assertion.

- [ ] **Step 2: Run the source lane against live Fleet on the exact candidate SHA**

Set the OAuth secrets in the environment and run:

```bash
export CUA_FLEET_BASE_URL=https://run.cua.ai
export CUA_LIVE_E2E_LANE=main-source
export CUA_LIVE_E2E_NAMESPACE="cua-live-main-source-manual"
export CUA_LIVE_E2E_ARTIFACT_DIR=/tmp/cua-live-main-source
LIVE_TEST_ROOT=$(mktemp -d /tmp/cua-live-e2e-suite.XXXXXX)
mkdir -p "$LIVE_TEST_ROOT/tests/live"
cp libs/python/cua-sandbox/tests/__init__.py "$LIVE_TEST_ROOT/tests/"
cp libs/python/cua-sandbox/tests/live/*.py "$LIVE_TEST_ROOT/tests/live/"
PYTHONPATH="$LIVE_TEST_ROOT" \
  libs/python/cua-sandbox/.venv/bin/python -m pytest -q -s \
  "$LIVE_TEST_ROOT/tests/live/test_fleet_ephemeral.py"
```

Expected: PASS; summary reports `1024x768`, `Linux`, port `8000`, the editable `cua_sandbox` module origin, and claim-only cleanup.

- [ ] **Step 3: Run the published lane in an isolated environment**

Run from the repository root with the OAuth secrets already exported:

```bash
PUBLISHED_ROOT=$(mktemp -d /tmp/cua-published-live.XXXXXX)
trap 'rm -rf "$PUBLISHED_ROOT"' EXIT
uv venv --python 3.12 "$PUBLISHED_ROOT/venv"
uv pip install \
  --python "$PUBLISHED_ROOT/venv/bin/python" \
  --index https://wheels.cua.ai/simple \
  --default-index https://pypi.org/simple \
  --upgrade cua-sandbox pytest pytest-asyncio
export CUA_FLEET_BASE_URL=https://run.cua.ai
export CUA_LIVE_E2E_LANE=published-package
export CUA_LIVE_E2E_NAMESPACE="cua-live-published-package-manual"
export CUA_LIVE_E2E_ARTIFACT_DIR=/tmp/cua-live-published-package
LIVE_TEST_ROOT=$(mktemp -d /tmp/cua-live-e2e-suite.XXXXXX)
mkdir -p "$LIVE_TEST_ROOT/tests/live"
cp libs/python/cua-sandbox/tests/__init__.py "$LIVE_TEST_ROOT/tests/"
cp libs/python/cua-sandbox/tests/live/*.py "$LIVE_TEST_ROOT/tests/live/"
PYTHONPATH="$LIVE_TEST_ROOT" \
  "$PUBLISHED_ROOT/venv/bin/python" -m pytest -q -s \
  "$LIVE_TEST_ROOT/tests/live/test_fleet_ephemeral.py"
```

Expected: PASS with the installed package version in `summary.json` and the expected persistent pool/template and no claims.

- [ ] **Step 4: Push the branch and open the implementation PR**

```bash
git push -u origin codex/periodic-live-sandbox-e2e
cat > /tmp/periodic-live-e2e-pr.md <<'EOF'
## What changed

- restore a Fleet-backed live `cua-sandbox` smoke every 15 minutes
- test repository `main` and the latest published package independently
- verify port `8000`, screen dimensions, screenshot, shell, and claim-only cleanup
- notify Alertmanager with lane-specific failure context

## Verification

- offline support and workflow contract tests pass
- focused Fleet regression tests pass
- source and published live lanes pass against the pinned Duo image
EOF
gh pr create \
  --repo trycua/cua \
  --title "ci(cua-sandbox): restore periodic live Fleet E2E" \
  --body-file /tmp/periodic-live-e2e-pr.md
```

The PR body must include the candidate SHA, both live lane results, package versions, namespace names, and cleanup confirmation.

- [ ] **Step 5: Merge the implementation PR after review and green CI**

```bash
PR_NUMBER=$(gh pr view --repo trycua/cua --json number --jq '.number')
gh pr checks --repo trycua/cua "$PR_NUMBER" --watch
gh pr merge --repo trycua/cua "$PR_NUMBER" --squash
```

Expected: the PR is merged and `.github/workflows/periodic-cua-sandbox-live.yml` exists on `main`.

- [ ] **Step 6: Verify the Alertmanager path with a controlled failure**

After the workflow exists on `main`, dispatch the manual-only controlled failure:

```bash
gh workflow run periodic-cua-sandbox-live.yml \
  --repo trycua/cua \
  --ref main \
  -f lane=main-source \
  -f force_failure=true
RUN_ID=$(gh run list \
  --repo trycua/cua \
  --workflow periodic-cua-sandbox-live.yml \
  --event workflow_dispatch \
  --limit 1 \
  --json databaseId \
  --jq '.[0].databaseId')
gh run watch "$RUN_ID" --repo trycua/cua --exit-status || true
curl -fsS --get https://am.cua.ai/api/v2/alerts \
  --data-urlencode 'filter=alertname="PeriodicCuaSandboxLiveE2EFailed"' \
  | jq -e '.[] | select(.labels.service == "cua-sandbox" and .labels.lane == "main-source")'
```

Expected: the workflow fails intentionally and Alertmanager returns one matching lane-specific alert. Because `force_failure` is restricted to `workflow_dispatch`, scheduled and push runs cannot activate it.

- [ ] **Step 7: Observe two consecutive scheduled intervals**

After merge, inspect two consecutive `:07/:22/:37/:52` runs. Confirm both lanes pass, a newer schedule cancels only an older scheduled run of the same lane, artifacts are absent on success, and each dedicated namespace retains only its named pool/template with no claims.

- [ ] **Step 8: Record rollout evidence**

Update the merged PR or implementation issue with workflow run links, observed versions, cleanup evidence, and the Alertmanager test. The rollout is complete only after two consecutive scheduled runs pass for both lanes.

## Live Evidence Remediation

The monitor uses reusable namespaces for scheduled and push runs, while manual ephemeral runs use per-run namespaces to avoid stale ownership collisions.
Each lane has one DNS-safe namespace for each event class:

- `cua-live-<lane>-schedule` for scheduled runs
- `cua-live-<lane>-push` for pushes
- `cua-live-<lane>-<run-id>` for `workflow_dispatch`

The event-and-lane concurrency group serializes reusable scheduled and push claims;
only scheduled runs cancel an older scheduled run in the same lane. Fleet
reconciliation preserves the namespace, pool, and template, all named after the
namespace. `Sandbox.ephemeral()` is verified with claim-only cleanup: after
exit the monitor polls until claims are absent, records persistent reconciled
resources, and requires exactly the named pool/template with zero claims. The
read-only poll and inventory run after every provisioning attempt, including one
that fails before yielding a sandbox. It never explicitly deletes a namespace,
pool, or template.


Workflow namespace expression: `cua-live-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && github.run_id || github.event_name }}`.

persistent reconciled resources are intentionally retained between runs; only deterministic claims are ephemeral.

The workflow uses step-scoped OAuth credentials only for credential preflight and the
live pytest step. After checkout, `git rev-parse HEAD` is exported as
`CUA_LIVE_E2E_SOURCE_SHA` and is the source SHA recorded by live, controlled-failure,
and Alertmanager evidence.

## Persistent Pool Suite

The workflow later gained a second suite that claims from persistent,
pre-provisioned Fleet pools instead of creating and destroying a pool per run.
The prepare job emits a lane-and-suite matrix: pushes stay on the `ephemeral`
suite only, while scheduled runs cross both lanes with both suites and manual
dispatch selects lane and suite combinations. The concurrency group is
`periodic-cua-sandbox-live-${{ github.event_name }}-${{ matrix.lane }}-${{ matrix.suite }}`,
so a newer schedule cancels only an older scheduled run of the same lane and
suite.

The suite's step, `Run live Fleet pool smoke`, executes
`tests/live/test_fleet_pool_persistent.py` from the same isolated copied suite
with step-scoped OAuth credentials. Two pool namespaces per lane and event
class are set by the workflow:

- `cua-live-pool-warm-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}`
- `cua-live-pool-cold-${{ matrix.lane }}-${{ github.event_name == 'workflow_dispatch' && 'manual' || github.event_name }}`

The warm pool keeps `replicas=1` so claims bind to pre-provisioned capacity;
the cold pool expresses scale-to-zero with
`WarmPoolAutoscaling(min_pool_size=0, initial_pool_size=0, max_pool_size=1)`
because pool reconciliation rejects `replicas` below one. Each run records
`pool_pre_existed` and replica counts from `Pool.get`, treating both 403 and
404 as not-pre-existed (`is_pool_missing_error`) because Fleet evaluates
authorization before existence for namespaces that have not been created
yet, reconciles the pinned configuration idempotently with `Pool.apply`,
claims through
`Sandbox.ephemeral(pool=..., name=...)` with the claim name fixed to the
namespace, and exits with a claim-only release. After release the monitor
polls until claims are absent and requires the reconciled inventory to contain
exactly the named pool and template with zero claims; the pool and template
deliberately persist between runs. The warm mode asserts a claim-acquisition
bound only when the pool pre-existed with a ready replica. Failure artifacts
and Alertmanager labels carry the suite alongside the lane.
