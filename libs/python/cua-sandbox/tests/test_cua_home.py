"""cua-sandbox keeps its state under ``$CUA_HOME`` like the Rust core, and
with a temp ``CUA_HOME`` nothing under the real ``~/.cua`` is read or
written.

``HOME`` points at a fake home holding a sentinel ``~/.cua`` tree; every
state path is exercised, then the sentinel tree must be byte-for-byte and
mtime-for-mtime unchanged.
"""

from __future__ import annotations

import importlib
import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

sandbox_state = importlib.import_module("cua_sandbox.sandbox_state")
_paths = importlib.import_module("cua_sandbox._paths")


def _snapshot(root: Path) -> dict:
    return {
        str(p.relative_to(root)): (p.stat().st_mtime_ns, p.read_bytes() if p.is_file() else b"")
        for p in sorted(root.rglob("*"))
    }


@pytest.fixture
def fake_homes(tmp_path, monkeypatch):
    home = tmp_path / "home"
    real_cua = home / ".cua"
    (real_cua / "sandboxes").mkdir(parents=True)
    (real_cua / "sandboxes" / "sentinel.json").write_text(
        json.dumps({"name": "sentinel", "runtime_type": "docker"})
    )
    (real_cua / "credentials").write_text("api_key=sentinel-key\n")
    cua_home = tmp_path / "cua-home"
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("USERPROFILE", str(home))
    monkeypatch.setenv("CUA_HOME", str(cua_home))
    before = _snapshot(real_cua)
    yield real_cua, cua_home
    assert _snapshot(real_cua) == before, "the real ~/.cua was modified"


def test_cua_home_rule_matches_the_rust_core(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("CUA_HOME", "")
    assert _paths.cua_home() == tmp_path / ".cua"
    monkeypatch.setenv("CUA_HOME", str(tmp_path / "x"))
    assert _paths.cua_home() == tmp_path / "x"


def test_state_goes_to_cua_home_never_the_real_home(fake_homes):
    real_cua, cua_home = fake_homes
    # CUA_HOME was set after import: the paths follow it at call time.
    assert sandbox_state.state_dir() == cua_home / "sandboxes"
    sandbox_state.save(
        "cua-e2e-home",
        runtime_type="docker",
        image={"os_type": "linux"},
        host="127.0.0.1",
        api_port=3211,
    )
    assert (cua_home / "sandboxes" / "cua-e2e-home.json").exists()
    names = [s.get("name") for s in sandbox_state.list_all()]
    assert "cua-e2e-home" in names and "sentinel" not in names
    sandbox_state.delete("cua-e2e-home")

    auth = importlib.import_module("cua_sandbox._auth")
    auth._save_credentials(api_key="test-key")
    assert (cua_home / "credentials").read_text() == "api_key=test-key\n"
    config = importlib.import_module("cua_sandbox._config")
    assert config._read_credentials_key() == "test-key"

    autopool = importlib.import_module("cua_sandbox._autopool")
    assert autopool.cua_dir() == cua_home
    image = importlib.import_module("cua_sandbox.image")
    assert image._image_cache() == cua_home / "cua-sandbox" / "image-cache"


def test_a_patched_state_dir_still_wins(fake_homes, tmp_path, monkeypatch):
    monkeypatch.setattr(sandbox_state, "SANDBOX_STATE_DIR", tmp_path / "patched")
    assert sandbox_state.state_dir() == tmp_path / "patched"


def test_no_module_path_points_at_the_real_home(tmp_path):
    """Imported with HOME=fake and CUA_HOME=temp, no module-level path of
    cua_sandbox lies under the fake ~/.cua."""
    home = tmp_path / "home"
    home.mkdir()
    cua_home = tmp_path / "cua-home"
    probe = textwrap.dedent(f"""
        import importlib, pkgutil, pathlib, sys
        import cua_sandbox
        bad = []
        for m in pkgutil.walk_packages(cua_sandbox.__path__, "cua_sandbox."):
            try:
                mod = importlib.import_module(m.name)
            except Exception:
                continue
            for k, v in vars(mod).items():
                if isinstance(v, pathlib.PurePath) and str(v).startswith({str(home / ".cua")!r}):
                    bad.append(f"{{m.name}}.{{k}}={{v}}")
        print("\\n".join(bad))
        sys.exit(1 if bad else 0)
        """)
    env = {**os.environ, "HOME": str(home), "USERPROFILE": str(home), "CUA_HOME": str(cua_home)}
    r = subprocess.run([sys.executable, "-c", probe], env=env, capture_output=True, text=True)
    assert r.returncode == 0, f"module paths under the real home:\n{r.stdout}{r.stderr[-2000:]}"
    assert not (home / ".cua").exists(), "importing cua_sandbox created ~/.cua"


def test_unit_tests_never_use_the_real_cua_home():
    """The autouse conftest fixture points CUA_HOME at a temp dir, so a unit
    test that forgets to patch the state dir cannot leak records into the
    developer's ~/.cua."""
    real = Path(os.path.expanduser("~")) / ".cua"
    assert os.environ.get("CUA_HOME")
    assert not str(sandbox_state.state_dir()).startswith(str(real))
