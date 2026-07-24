"""Regression tests for the generated public cua-sandbox API reference."""

from __future__ import annotations

import ast
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
GENERATOR = ROOT / "scripts/docs-generators/generate_sandbox_sdk_reference.py"
REFERENCE = ROOT / "docs/content/docs/reference/sandbox-sdk/index.mdx"
PACKAGE_INIT = ROOT / "libs/python/cua-sandbox/cua_sandbox/__init__.py"


def _public_exports() -> list[str]:
    tree = ast.parse(PACKAGE_INIT.read_text())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "__all__" for target in node.targets
        ):
            return ast.literal_eval(node.value)
    raise AssertionError("cua_sandbox.__all__ is missing")


def _run_generator(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(GENERATOR), *args],
        cwd=ROOT,
        check=False,
        text=True,
        capture_output=True,
    )


def test_sdk_reference_is_current_and_covers_root_public_exports() -> None:
    result = _run_generator("--check")

    assert result.returncode == 0, result.stdout + result.stderr
    reference = REFERENCE.read_text()
    for export in _public_exports():
        assert f"`{export}`" in reference


def test_sdk_reference_preserves_public_signatures_and_excludes_internal_surface() -> None:
    reference = REFERENCE.read_text()

    expected_signatures = [
        "async def create(cls, image: Image, *, name: Optional[str] = None",
        "async def ephemeral(cls, image: Image, *, name: Optional[str] = None",
        "def from_registry(cls, ref: str) -> Image",
        "def expose(self, port: int) -> Image",
        "async def disconnect(self) -> None",
        "async def destroy(self) -> None",
        "def configure(*, api_key: Optional[str] = None",
    ]
    for signature in expected_signatures:
        assert signature in reference

    forbidden_symbols = [
        "FleetTransport",
        "cyclops_sdk",
        "_ConnectResult",
        "_create",
        "raw_operation",
    ]
    for symbol in forbidden_symbols:
        assert symbol not in reference
