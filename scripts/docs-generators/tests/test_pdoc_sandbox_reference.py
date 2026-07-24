"""Regression tests for the pdoc-backed cua-sandbox reference generator."""

from __future__ import annotations

import importlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from pdoc.doc import Module

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/docs-generators"))

import generate_pdoc_sandbox_reference as generator


class PdocSandboxReferenceTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary_directory.cleanup)
        self.package_root = Path(self.temporary_directory.name) / "fixture_sdk"
        self.package_root.mkdir()
        (self.package_root / "__init__.py").write_text(
            '''"""Fixture SDK."""

from .api import Image, Lifecycle, Sandbox, configure, sandbox
from cyclops_sdk import RawOperation

__all__ = ["Sandbox", "Image", "Lifecycle", "configure", "sandbox"]
'''
        )
        (self.package_root / "api.py").write_text(
            '''"""Public fixture APIs."""

from contextlib import asynccontextmanager
from typing import AsyncIterator


class Sandbox:
    """A connected sandbox."""

    tunnel: str
    """Port-forwarding interface."""

    service_ports: dict[str, int]
    """Named service ports exposed by the sandbox."""

    async def disconnect(self) -> None:
        """Disconnect without deleting the sandbox."""

    async def destroy(self) -> None:
        """Delete the sandbox. Returns {"status": "deleted"}."""

    async def __aenter__(self) -> "Sandbox":
        """Connect when entering an async context."""
        return self

    async def __aexit__(self, *exc: object) -> None:
        """Disconnect when leaving an async context."""

    def _raw_operation(self) -> None:
        """Private raw operation."""

    async def send(self, action: str) -> None:
        """Dispatch a raw action."""


class Image:
    """Immutable image builder."""

    def expose(self, port: int) -> "Image":
        """Expose a service port."""
        if port < 1:
            raise ValueError("port must be positive")


class ConnectResult:
    """Awaitable async-context wrapper."""


class Lifecycle:
    def connect(self) -> ConnectResult:
        """Return a wrapper that supports await and async with."""

        async def factory() -> None:
            """Connect asynchronously after the wrapper is invoked."""

        return ConnectResult()


def configure(*, api_key: str | None = None) -> None:
    """Configure the SDK."""


@asynccontextmanager
async def sandbox(name: str) -> AsyncIterator[Sandbox]:
    """Create a sandbox context."""
    yield Sandbox()
'''
        )
        (self.package_root / "interfaces.py").write_text(
            '''"""Public interface handles."""


class Tunnel:
    """Port-forwarding interface."""

    async def open(self, port: int) -> str:
        """Open a tunnel for a service port."""


class TunnelInfo:
    """Information about an open tunnel."""


__all__ = ["Tunnel", "TunnelInfo"]
'''
        )
        cyclops = Path(self.temporary_directory.name) / "cyclops_sdk.py"
        cyclops.write_text("class RawOperation: pass\n")
        sys.path.insert(0, self.temporary_directory.name)
        self.addCleanup(sys.path.remove, self.temporary_directory.name)
        importlib.invalidate_caches()
        self.addCleanup(sys.modules.pop, "fixture_sdk", None)
        self.addCleanup(sys.modules.pop, "fixture_sdk.api", None)
        self.addCleanup(sys.modules.pop, "fixture_sdk.interfaces", None)
        self.addCleanup(sys.modules.pop, "cyclops_sdk", None)

    def module(self) -> Module:
        return Module.from_name("fixture_sdk")

    def test_uses_root_all_as_the_public_contract(self) -> None:
        names = [member.name for member in generator.public_members(self.module())]

        self.assertEqual(names, ["Sandbox", "Image", "Lifecycle", "configure", "sandbox"])

    def test_renders_signatures_async_context_and_public_members(self) -> None:
        rendered = generator.render_reference(self.module())

        self.assertIn("pdoc imports `cua_sandbox` source at generation time", rendered)
        self.assertIn("does not statically analyze", rendered)
        self.assertIn("class Sandbox", rendered)
        self.assertIn("async def disconnect(self) -> None", rendered)
        self.assertIn("async def destroy(self) -> None", rendered)
        self.assertIn("async def __aenter__(self) -> Sandbox", rendered)
        self.assertIn("async def __aexit__(self, *exc: object) -> None", rendered)
        self.assertIn(r'\{"status": "deleted"\}', rendered)
        self.assertIn("tunnel", rendered)
        self.assertIn("service_ports: dict[str, int]", rendered)
        self.assertIn("def expose(self, port: int) -> Image", rendered)
        self.assertIn("async def sandbox(name: str) -> AsyncIterator[Sandbox]", rendered)
        self.assertIn("## Interfaces", rendered)
        self.assertIn("### `Tunnel`", rendered)
        self.assertIn("async def open(self, port: int) -> str", rendered)

    def test_includes_directly_raised_exceptions(self) -> None:
        def raises_value_error() -> None:
            raise ValueError("invalid fixture")

        self.assertEqual(
            generator.raised_exceptions(SimpleNamespace(obj=raises_value_error)), ["ValueError"]
        )

    def test_escapes_mdx_angle_brackets_in_docstrings(self) -> None:
        self.assertEqual(
            generator.mdx_docstring("http://localhost:<random>/json"),
            "http://localhost:&lt;random&gt;/json",
        )

    def test_uses_the_member_declaration_for_async_detection(self) -> None:
        rendered = generator.render_reference(self.module())

        self.assertIn("def connect(self) -> ConnectResult", rendered)
        self.assertNotIn("async def connect(self) -> ConnectResult", rendered)

    def test_excludes_private_and_non_exported_symbols(self) -> None:
        rendered = generator.render_reference(self.module())

        self.assertNotIn("_raw_operation", rendered)
        self.assertNotIn("async def send", rendered)
        self.assertNotIn("RawOperation", rendered)
        self.assertNotIn("cyclops_sdk", rendered)

    def test_rendering_is_deterministic(self) -> None:
        module = self.module()

        self.assertEqual(generator.render_reference(module), generator.render_reference(module))

    def test_formats_generated_mdx_with_the_locked_docs_prettier(self) -> None:
        result = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout="formatted MDX\n",
            stderr="",
        )
        with (
            patch.object(generator, "PRETTIER", Path("/tmp/prettier")),
            patch.object(generator.subprocess, "run", return_value=result) as run,
        ):
            self.assertEqual(generator.format_mdx("unformatted MDX\n"), "formatted MDX\n")

        run.assert_called_once_with(
            [
                "/tmp/prettier",
                "--stdin-filepath",
                str(generator.OUTPUT.relative_to(generator.DOCS)),
            ],
            check=False,
            cwd=generator.DOCS,
            input="unformatted MDX\n",
            stderr=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )

    def test_loads_the_source_package_without_native_fleet_bindings(self) -> None:
        fixture_parent = str(self.package_root.parent)
        sys.path.remove(fixture_parent)
        self.addCleanup(sys.path.insert, 0, fixture_parent)
        for module_name in tuple(sys.modules):
            if module_name == "cua_sandbox" or module_name.startswith("cua_sandbox."):
                sys.modules.pop(module_name)
        sys.modules.pop("cyclops_sdk", None)

        module = generator.load_public_module()
        rendered = generator.render_reference(module)

        self.assertEqual(
            [member.name for member in generator.public_members(module)],
            [
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
            ],
        )
        self.assertIn("async def __aenter__(self) -> Sandbox", rendered)
        self.assertIn("async def __aexit__(self, *exc: Any) -> None", rendered)
        self.assertNotIn("FleetTransport", rendered)
        self.assertNotIn("cyclops_sdk", rendered)
        interface_module = generator.load_pdoc_module("cua_sandbox.interfaces")
        for name in interface_module.obj.__all__:
            self.assertIn(f"### `{name}`", rendered)


if __name__ == "__main__":
    unittest.main()
