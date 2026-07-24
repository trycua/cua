from __future__ import annotations
import subprocess
import sys
from pathlib import Path
from mkdocstrings_handlers.python import PythonConfig, PythonHandler

ROOT = Path(__file__).resolve().parents[3]
GENERATOR = ROOT / "scripts/docs-generators/generate_mkdocstrings_sandbox_reference.py"
REFERENCE = ROOT / "docs/content/docs/reference/sandbox-sdk/index.mdx"


def exports() -> list[str]:
    handler = PythonHandler(
        PythonConfig(paths=[str(ROOT / "libs/python/cua-sandbox")]),
        ROOT,
        theme="material",
        custom_templates=None,
        mdx=[],
        mdx_config={},
    )
    return list(
        handler.collect("cua_sandbox", handler.get_options({"allow_inspection": False})).exports
    )


def test_reference_is_current_and_covers_root_public_exports() -> None:
    result = subprocess.run(
        [sys.executable, str(GENERATOR), "--check"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    for export in exports():
        assert f"`{export}`" in REFERENCE.read_text()


def test_reference_preserves_public_contract_and_excludes_private_operations() -> None:
    reference = REFERENCE.read_text()
    for fragment in [
        "async def create(cls, image: Image",
        "def from_registry(cls, ref: str) -> Image",
        "async def destroy(self) -> None",
        "def configure(*, api_key: Optional[str]",
        "def forward(self, *ports: int | str)",
        "async def __aenter__(self)",
        "ValueError",
    ]:
        assert fragment in reference
    for symbol in ["FleetTransport", "cyclops_sdk", "_ConnectResult", "_create", "raw_operation"]:
        assert symbol not in reference
