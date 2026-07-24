from __future__ import annotations

import html
import re
import subprocess
import sys
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
GENERATOR = ROOT / "scripts/docs-generators/sphinx_cua_sandbox.py"
CONF = ROOT / "docs/sphinx/cua-sandbox/conf.py"
ARTIFACT = ROOT / "docs/public/reference/cua-sandbox-sphinx/index.html"


def load_generator():
    spec = spec_from_file_location("sphinx_cua_sandbox", GENERATOR)
    assert spec and spec.loader
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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
    artifact_text = " ".join(html.unescape(re.sub(r"<[^>]+>", "", artifact)).split())
    for expected in (
        "cua_sandbox.__all__",
        "Sandbox",
        "Sandbox.create",
        "Sandbox.connect",
        "Sandbox.ephemeral",
        "Sandbox.list",
        "Sandbox.get_info",
        "Sandbox.suspend",
        "Sandbox.resume",
        "Sandbox.restart",
        "Sandbox.snapshot",
        "Sandbox.get_display_url",
        "Image",
        "configure",
        "api_key",
        "base_url",
        "share",
        "login",
        "whoami",
        "Apps",
        "Clipboard",
        "FileEntry",
        "Files",
        "Keyboard",
        "Mobile",
        "Mouse",
        "Screen",
        "Shell",
        "Terminal",
        "Tunnel",
        "TunnelInfo",
        "Window",
        "__aenter__",
        "__aexit__",
        "NotImplementedError",
        "CloudTransport",
    ):
        assert expected in artifact
    assert "async with" in artifact_text
    for forbidden in (
        "cyclops_sdk",
        "_make_transport",
        "HTTPTransport.request",
        "cua_sandbox.transport.",
        "Fleet",
        "fleet_base_url",
        "raw VM",
    ):
        assert forbidden not in artifact


def test_public_autosummary_is_derived_from_literal_all() -> None:
    generator = load_generator()

    exports = generator.public_exports()

    assert exports == (
        "configure",
        "login",
        "whoami",
        "Image",
        "Sandbox",
        "SandboxInfo",
        "sandbox",
        "Localhost",
        "localhost",
        "CloudTransport",
        "RuntimeSupport",
        "check_local_support",
        "skip_if_unsupported",
    )
    rendered = generator.render_public_exports(exports)
    assert all(f"cua_sandbox.{name}" in rendered for name in exports)
    assert not (ROOT / "docs/sphinx/cua-sandbox/public-exports.rst").exists()


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
    assert "socket.socket = _NoNetworkSocket" in config
    assert "autodoc-process-signature" in config
    assert "autodoc-process-docstring" in config
    assert "subprocess.Popen = _deny_runtime_operation" in config
    assert '"uv",' in generator
    assert '"-m",' in generator
    assert '"sphinx",' in generator
