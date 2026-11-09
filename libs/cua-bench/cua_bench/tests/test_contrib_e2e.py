"""A cua-bench task on a contrib sandbox provider, through the unchanged
cua-bench code path: ``CUA_BENCH_ON=daytona`` -> ``resolve_target`` ->
``open_sandbox`` -> ``cua_sandbox.Sandbox.ephemeral(image, on="daytona")`` ->
the cua SDK's Daytona provider -> the task's shell and file calls over
cua-spacesd. cua-bench has no provider-specific code; the location word goes
to the SDK as is.

Hermetic: the Daytona API is ``cua-contrib-fixtures`` (a schema mock from
Daytona's published OpenAPI spec, not recorded traffic) whose sandboxes'
port 3211 is a mock cua-spacesd on loopback. Nothing runs on the host.

Opt-in: it needs the real ``cua`` binding built with contrib providers and
the fixtures binary (``.github/workflows/ci-cua-contrib.yml`` builds both):

    CUA_CONTRIB_FIXTURES=<path to cua-contrib-fixtures> pytest cua_bench/tests/test_contrib_e2e.py

The live lane (``CUA_E2E_CONTRIB_LIVE=1`` plus a provider key) runs the same
task against the real provider; see ``tests/live/test_contrib_live.py``.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest

HELLO = Path(__file__).resolve().parents[2] / "example_tasks" / "hello_file_env"


def _contrib_binding() -> tuple[bool, str]:
    try:
        import cua  # noqa: F401
        from cua import _native as native
    except Exception as error:  # noqa: BLE001 - the binding is optional here
        return False, f"the cua binding is not importable ({error})"
    built = getattr(native, "contrib_providers_built", None)
    if built is None:
        return False, "the cua binding predates contrib providers"
    if "daytona" not in built():
        return False, "the cua binding was built without contrib providers (--features contrib)"
    return True, ""


@pytest.fixture
def daytona_fixture(monkeypatch, tmp_path):
    binary = os.environ.get("CUA_CONTRIB_FIXTURES")
    # The contrib CI lane sets CUA_CONTRIB_REQUIRE=1: a missing prerequisite
    # there is a failure, not a skip.
    missing = pytest.fail if os.environ.get("CUA_CONTRIB_REQUIRE") == "1" else pytest.skip
    if not binary:
        missing("CUA_CONTRIB_FIXTURES is not set (the contrib lane builds it)")
    ok, why = _contrib_binding()
    if not ok:
        missing(why)
    proc = subprocess.Popen(
        [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
    )
    try:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("cua-contrib-fixtures exited before printing endpoints")
        endpoints = json.loads(line)
        monkeypatch.setenv("DAYTONA_API_URL", endpoints["daytona_api"])
        monkeypatch.setenv("DAYTONA_API_KEY", endpoints["daytona_key"])
        # No registry in the hermetic lane: the image runs as tagged.
        monkeypatch.setenv("CUA_IMAGE_RESOLVE", "0")
        monkeypatch.setenv("CUA_HOME", str(tmp_path / "cua-home"))
        monkeypatch.setenv("CUA_DEFAULT_ON", "daytona")
        for var in ("CUA_DEFAULT_RUNTIME", "CUA_DEFAULT_KIND", "CUA_BENCH_IMAGE"):
            monkeypatch.delenv(var, raising=False)
        from cua_sandbox import sandbox_state

        monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path / "sandboxes")
        yield endpoints
    finally:
        # Closing stdin (communicate does) stops the fixture.
        try:
            _, stderr = proc.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            _, stderr = proc.communicate(timeout=10)
        # The fixture reports sandboxes a run leaked.
        assert "sandboxes left at exit" not in (stderr or ""), stderr


@pytest.mark.asyncio
async def test_a_bench_task_runs_on_a_contrib_provider(daytona_fixture):
    from cua_bench import run_single_task
    from cua_bench.targets import resolve_target

    assert resolve_target().on == "daytona"
    for index, word in enumerate(("hello", "bench")):
        result = await run_single_task(HELLO, task_index=index, oracle=True)
        assert result.success and result.reward == 1.0, (word, result)


def test_an_unknown_location_is_refused_by_name():
    from cua_bench.targets import TargetError, resolve_target

    with pytest.raises(TargetError, match="--on must be one of"):
        resolve_target(on="nosuchcloud", environ={})
    # Contrib words are accepted as locations (the SDK validates the build).
    assert resolve_target(on="e2b", environ={}).on == "e2b"
    assert resolve_target(on="daytona", environ={}).contrib
