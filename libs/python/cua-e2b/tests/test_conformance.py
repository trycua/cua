"""cua-e2b against the goldens recorded from the real e2b (2.42.0 and 2.56.0)
and E2B.

- The API goldens (`conformance/golden/e2b-<version>-api.json`): every covered
  method accepts e2b's parameters with e2b's defaults, dataclasses have
  e2b's fields, exceptions e2b's bases; both releases are met at once (2.56
  spells three defaults `None` where 2.42 said `True`; `None` means `True`).
  Pure Python; runs everywhere.
- The behaviour goldens (`conformance/golden/e2b-<version>-behavior.json`): the
  conformance suite run through cua-e2b against a real cua-spacesd (the
  binding smoke fixture's in-process server core, `cua-test-fixtures`),
  compared case by case. Needs the fixture and the `cua` wheel; skipped
  otherwise. Cases that need a VM (pause, no network) run in the live lane
  (`.github/workflows/e2b-conformance.yml`).
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest
from conformance import api, suite

HERE = Path(__file__).resolve().parent
GOLDEN = HERE / "conformance" / "golden"
REPO = HERE.parents[3]

# Cases the fixture cannot run: it is a process on this host, not a VM.
NEEDS_A_VM = [
    "pause_connect",
    "no_internet",
    "pause_modes",
    "connect_reboot",
    "lifecycle_filesystem_timeout",
    "snapshot_roundtrip",
    "fork_running",
    "snapshot_pagination",
    "sbx_paused_ops",
    # update_network drops packets in the VM's network namespace
    "sbx_network_update",
]

# Cases that become another user or root: the fixture's guest user cannot.
# They run in the live lane (a VM) and against E2B.
try:  # TEMPORARY: see conformance/pending.py
    from conformance.pending import PENDING
except ImportError:
    PENDING = []

NEEDS_ROOT = [
    "cmd_run_as_root_and_env",
    "fs_mode_owner_root",
    "fs_permission_other_user",
    "fs_remove_other_user",
]

VERSIONS = sorted(p.name.split("-")[1] for p in GOLDEN.glob("e2b-*-api.json"))


def test_there_are_goldens_for_the_claimed_releases():
    assert {"2.42.0", "2.56.0", "2.58.0"} <= set(VERSIONS)


@pytest.mark.parametrize("version", VERSIONS)
def test_api_matches_e2b(version):
    golden = json.loads((GOLDEN / f"e2b-{version}-api.json").read_text())
    assert api.compare(golden, api.dump()) == []


def test_api_compare_flags_a_missing_parameter_even_with_kwargs():
    golden = {"callables": {"Sandbox.pause": [["mode", "KEYWORD_ONLY", "None"]]}}
    actual = {"callables": {"Sandbox.pause": [["opts", "VAR_KEYWORD", None]]}}
    for k in ("dataclasses", "exceptions", "enums"):
        golden[k] = actual[k] = {}
    assert api.compare(golden, actual) == ["Sandbox.pause: no parameter 'mode'"]


def test_compare_flags_drift_and_allows_documented_differences():
    golden = {"run_fg": {"ok": {"exit_code": 0}}, "get_host": {"ok": {"format": "a"}}}
    same = {"run_fg": {"ok": {"exit_code": 0}}, "get_host": {"ok": {"format": "b"}}}
    drifted = {"run_fg": {"ok": {"exit_code": 1}}, "get_host": {"ok": {"format": "a"}}}
    assert suite.compare(golden, same) == []
    assert suite.compare(golden, drifted) == ["run_fg.ok.exit_code: 0 != 1"]


def test_compare_ignores_what_a_release_lacks():
    golden = {"c": {"ok": {"since": "2.58"}}, "d": {"ok": {"a": 1, "x": {"since": "2.56"}}}}
    actual = {"c": {"ok": {"anything": 1}}, "d": {"ok": {"a": 1, "x": {"deep": 2}}}}
    assert suite.compare(golden, actual) == []
    assert suite.compare({"d": {"ok": {"a": 2}}}, {"d": {"ok": {"a": 1}}}) == ["d.ok.a: 2 != 1"]


def test_compare_allows_glob_patterns():
    golden = {"c": {"ok": {"k": {"text": "a"}, "other": 1}}}
    actual = {"c": {"ok": {"k": {"text": "b"}, "other": 2}}}
    assert suite.compare(golden, actual, allow=["c.k.*"]) == ["c.ok.other: 1 != 2"]


def test_every_allowed_difference_has_a_reason():
    for table in (suite.ALLOWED, suite.SINCE_2_56):
        assert all(isinstance(r, str) and len(r) > 20 for r in table.values())


def _registered_cases():
    suite._import_cases()
    return set(suite.CASES) | set(suite.FRESH) | {"sync." + n for n in suite.SYNC_CASES}


def test_the_goldens_cover_every_case_of_every_release():
    cases = _registered_cases()
    for path in sorted(GOLDEN.glob("e2b-*-behavior.json")):
        recorded = set(json.loads(path.read_text())["results"])
        assert cases <= recorded, f"{path.name} lacks {sorted(cases - recorded)}"


def test_the_vm_and_root_lists_name_real_cases():
    cases = _registered_cases()
    assert set(NEEDS_A_VM) <= cases and set(NEEDS_ROOT) <= cases


def _fixture_binary():
    explicit = os.environ.get("CUA_TEST_FIXTURES")
    if explicit:
        return Path(explicit)
    for profile in ("debug", "release"):
        p = REPO / "libs" / "cua" / "target" / profile / "cua-test-fixtures"
        if p.exists():
            return p
    return None


@pytest.fixture(scope="module")
def spaces_fixture():
    pytest.importorskip("cua")
    binary = _fixture_binary()
    if binary is None or sys.platform != "linux":
        # The fixture's Space runs this host's /bin/bash and /proc: Linux.
        pytest.skip("cua-test-fixtures is not built (libs/cua/scripts/build-test-fixtures.sh)")
    proc = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        line = proc.stdout.readline()
        if not line:
            pytest.fail("cua-test-fixtures exited before printing its endpoints")
        yield json.loads(line)
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)


def _free_port() -> int:
    import socket

    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


# The fixture's Space is this host: what one release's run leaves in its file
# system would make the next release's run see an existing directory or repo.
HOST_LEFTOVERS = ["/tmp/conf", "/tmp/conf-git"]


@pytest.mark.parametrize(
    "version", sorted(p.name.split("-")[1] for p in GOLDEN.glob("e2b-*-behavior.json"))
)
def test_behaviour_matches_e2b_on_cua_spacesd(version, spaces_fixture, tmp_path):
    import shutil

    for leftover in HOST_LEFTOVERS:
        shutil.rmtree(leftover, ignore_errors=True)
    golden = json.loads((GOLDEN / f"e2b-{version}-behavior.json").read_text())["results"]
    env = {
        **os.environ,
        "CUA_TEST": "1",
        "CUA_HOME": tempfile.mkdtemp(prefix="cua-e2b-conf-"),
        "CUA_E2B_MODE": "embedded",
        "CUA_E2B_ON": "direct:" + spaces_fixture["spaces_url"],
        "CUA_E2B_TOKEN": spaces_fixture["spaces_token"],
        "CONFORMANCE_SCHEME": "http",
        "CONFORMANCE_NO_ROOT": "1",
        "CONFORMANCE_SKIP": ",".join(NEEDS_A_VM + NEEDS_ROOT + PENDING),
        "CONFORMANCE_HTTP_PORT": str(_free_port()),
        "PYTHONPATH": str(HERE) + os.pathsep + os.environ.get("PYTHONPATH", ""),
    }
    out = subprocess.run(
        [sys.executable, "-m", "conformance.suite"],
        env=env,
        cwd=tmp_path,
        capture_output=True,
        text=True,
        timeout=600,
    )
    assert out.returncode == 0, out.stderr[-4000:]
    results = json.loads(out.stdout)["results"]
    # a release without a case (2.42 has no pause modes) is not held to it
    skip = NEEDS_A_VM + NEEDS_ROOT + PENDING + sorted(set(results) - set(golden))
    allow = suite.SINCE_2_56 if tuple(map(int, version.split("."))) < (2, 56) else ()
    assert suite.compare(golden, results, skip=skip, allow=allow) == []
