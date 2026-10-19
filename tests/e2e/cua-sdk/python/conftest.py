"""pytest wiring for the cua SDK e2e suite.

Every test carries ``@pytest.mark.e2e(scenario, lane)``. Setup skips it when
the lane is not enabled here (with the reason), and the outcome is appended
to ``$CUA_E2E_RESULTS/py.jsonl`` for ``run.py``'s matrix.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
from pathlib import Path

import e2e
import pytest


def pytest_configure(config):
    config.addinivalue_line("markers", "e2e(scenario, lane): cua SDK e2e scenario and lane")
    config.addinivalue_line(
        "markers", "covers_docs(*block_ids): docs code blocks (page#id) this test exercises"
    )


def _mark(item):
    m = item.get_closest_marker("e2e")
    if m is None:
        raise pytest.UsageError(f"{item.nodeid} has no @pytest.mark.e2e(scenario, lane)")
    return m.args[0], m.args[1]


def pytest_runtest_setup(item):
    _, lane = _mark(item)
    why = e2e.lane_enabled(lane)
    if why:
        pytest.skip(why)


_started: dict[str, float] = {}


def pytest_runtest_logstart(nodeid, location):
    _started[nodeid] = time.monotonic()


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    rep = outcome.get_result()
    done = (
        (rep.when == "call")
        or (rep.when == "setup" and (rep.skipped or rep.failed))
        or (rep.when == "teardown" and rep.failed)
    )
    if not done:
        return
    scenario, lane = _mark(item)
    if hasattr(rep, "wasxfail"):
        status = "xfail" if rep.skipped else "xpass"
        reason = str(rep.wasxfail)[:400]
    elif rep.skipped:
        status = "skip"
        reason = rep.longrepr[2] if isinstance(rep.longrepr, tuple) else str(rep.longrepr)
        reason = reason.removeprefix("Skipped: ")
    elif rep.failed:
        status = "fail"
        reason = (rep.longreprtext or "").strip().splitlines()[-1:] or [""]
        reason = reason[0][:400]
    else:
        status, reason = "pass", ""
    record = {
        "scenario": scenario,
        "lang": "py",
        "lane": lane,
        "test": item.name,
        "status": status,
        "secs": round(time.monotonic() - _started.get(item.nodeid, time.monotonic()), 1),
        "reason": reason,
        "run": e2e.RUN,
    }
    out = os.environ.get("CUA_E2E_RESULTS")
    if out:
        Path(out).mkdir(parents=True, exist_ok=True)
        with open(Path(out) / "py.jsonl", "a") as f:
            f.write(json.dumps(record) + "\n")
        _record_docs_blocks(item, record, Path(out))


def _record_docs_blocks(item, record: dict, out: Path) -> None:
    """Coverage manifest: one line per docs block a test ran, keyed by the
    block's stable id, into docs-blocks.jsonl (joined by docs/coverage.py).

    A test covers blocks either through a ``block`` parameter (an
    ``extract.Block``) or by declaring ``@pytest.mark.covers_docs(<id>, ...)``.
    """
    params = getattr(getattr(item, "callspec", None), "params", {})
    blocks = [params["block"]] if getattr(params.get("block"), "guide", None) else []
    ids = [(b.id, b.lang, b.line) for b in blocks]
    for m in item.iter_markers("covers_docs"):
        ids += [(i, "", 0) for i in m.args]
    if not ids:
        return
    with open(out / "docs-blocks.jsonl", "a") as f:
        for block_id, lang, line in ids:
            row = {
                "block_id": block_id,
                "page": block_id.split("#", 1)[0] + ".mdx",
                "line": line,
                "lane": record["lane"],
                "lang": lang or record["lang"],
                "status": record["status"],
                "test": record["test"],
                "reason": record["reason"],
            }
            f.write(json.dumps(row) + "\n")


@pytest.fixture(scope="session")
def fixtures():
    """cua-test-fixtures: MockServer spacesd + fake Fleet on loopback."""
    binary = e2e.fixtures_binary()
    if binary is None:
        pytest.skip("cua-test-fixtures is not built")
    proc = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("cua-test-fixtures exited before printing endpoints")
        yield json.loads(line)
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)


@pytest.fixture
def fake_fleet(fixtures, tmp_path):
    """An embedded Cua whose Fleet is the fake API."""
    import cua

    return cua.embedded(
        state_dir=str(tmp_path / "state"),
        fleet_from_env=False,
        fleet=cua.FleetSettings(base_url=fixtures["fleet_base_url"], token=fixtures["fleet_token"]),
    )


@pytest.fixture
def live_fleet(tmp_path):
    import cua

    return cua.embedded(state_dir=str(tmp_path / "state"))


@pytest.fixture
def local_cua(tmp_path):
    import cua

    return cua.embedded(state_dir=str(tmp_path / "state"), fleet_from_env=False)
