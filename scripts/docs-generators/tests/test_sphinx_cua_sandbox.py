from __future__ import annotations

import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
GENERATOR = ROOT / "scripts/docs-generators/sphinx_cua_sandbox.py"
CONF = ROOT / "docs/sphinx/cua-sandbox/conf.py"
ARTIFACT = ROOT / "docs/public/reference/cua-sandbox-sphinx/index.html"


def run_generator(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GENERATOR), *args],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
    )


def test_sphinx_artifact_is_current_and_public() -> None:
    result = run_generator("--check")

    assert result.returncode == 0, result.stderr
    artifact = ARTIFACT.read_text()
    for expected in (
        "cua_sandbox.__all__",
        "Sandbox",
        "Sandbox.create",
        "Image",
        "configure",
        "Tunnel",
        "__aenter__",
        "__aexit__",
        "CloudTransport",
    ):
        assert expected in artifact
    for forbidden in (
        "cyclops_sdk",
        "_make_transport",
        "HTTPTransport.request",
        "cua_sandbox.transport.",
    ):
        assert forbidden not in artifact


def test_check_does_not_modify_the_committed_artifact() -> None:
    before = ARTIFACT.read_bytes()

    result = run_generator("--check")

    assert result.returncode == 0, result.stderr
    assert ARTIFACT.read_bytes() == before


def test_generator_only_imports_the_sdk_in_the_sphinx_subprocess() -> None:
    generator = GENERATOR.read_text()
    config = CONF.read_text()

    assert "import cua_sandbox" not in generator
    assert '"cyclops_sdk"' in config
    assert '"uv",' in generator
    assert '"-m",' in generator
    assert '"sphinx",' in generator
