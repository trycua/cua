from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
GENERATOR_PATH = ROOT / "scripts/docs-generators/generate_sandbox_sdk_reference_griffe.py"


def load_generator():
    spec = importlib.util.spec_from_file_location("sandbox_sdk_reference_griffe", GENERATOR_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_rendered_reference_covers_public_api_and_behavior() -> None:
    generator = load_generator()

    rendered = generator.render_reference()

    assert "The supported public surface is defined by `cua_sandbox.__all__`." in rendered
    for heading in (
        "## `Sandbox`",
        "## `Image`",
        "## Configuration and authentication",
        "## Lifecycle and service access",
        "## Tunnels and exported interfaces",
    ):
        assert heading in rendered
    for symbol in (
        "`Sandbox.create`",
        "`Sandbox.connect`",
        "`Image.linux`",
        "`configure`",
        "`Tunnel.forward`",
        "`Shell.run`",
    ):
        assert symbol in rendered
    assert "async def" in rendered
    assert "Raises:" in rendered
    assert "Supports both `await` and `async with`" in rendered


def test_reference_excludes_internal_and_raw_surfaces() -> None:
    generator = load_generator()

    rendered = generator.render_reference()

    for forbidden in (
        "cyclops_sdk",
        "FleetTransport",
        "FleetCloudTransport",
        "raw_request",
        "_TunnelContext",
        "_ConnectResult",
    ):
        assert forbidden not in rendered


def test_check_detects_stale_output(tmp_path: Path) -> None:
    generator = load_generator()
    output = tmp_path / "index.mdx"
    output.write_text("stale\n")

    with pytest.raises(generator.StaleReferenceError):
        generator.check_reference(output)

    generator.write_reference(output)
    generator.check_reference(output)
