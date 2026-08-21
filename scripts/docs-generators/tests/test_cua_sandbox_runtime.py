import importlib.util
import sys
import tempfile
import types
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "cua_sandbox_runtime.py"
REQUIREMENTS = Path(__file__).resolve().parents[1] / "cua_sandbox_runtime_requirements.txt"


def load_generator():
    spec = importlib.util.spec_from_file_location("cua_sandbox_runtime", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class RuntimeGeneratorTests(unittest.TestCase):
    def make_module(self):
        module = types.ModuleType("cua_sandbox")

        class Tunnel:
            """Expose a public service through a tunnel."""

            async def open(self, port: int) -> str:
                """Open a tunnel.

                Raises:
                    RuntimeError: When the service cannot be reached.
                """

        Tunnel.__module__ = "cua_sandbox.interfaces.tunnel"

        class _ConnectResult:
            pass

        _ConnectResult.__module__ = "cua_sandbox.sandbox"

        class Image:
            """A sandbox image definition."""

            @classmethod
            def linux(cls, version: str = "latest") -> "Image":
                """Create a Linux image."""

        Image.__module__ = "cua_sandbox.image"

        class Sandbox:
            """A sandbox lifecycle handle."""

            tunnel: Tunnel

            async def create(self, image: Image) -> _ConnectResult:
                """Create the sandbox."""

            async def delete(self) -> None:
                """Delete the sandbox."""

            async def start(self, image: str = "base") -> _ConnectResult:
                """Start the sandbox."""

            async def __aenter__(self):
                return self

            async def __aexit__(self, exc_type, exc_value, traceback):
                return None

            def raw_request(self):
                raise AssertionError("raw operations must not be documented")

        Sandbox.__module__ = "cua_sandbox.sandbox"

        class FleetClient:
            pass

        FleetClient.__module__ = "cua_sandbox.transport.fleet"

        def configure(api_key: str | None = None) -> None:
            """Configure the client."""

        configure.__module__ = "cua_sandbox._config"
        module.__all__ = ["Sandbox", "configure", "Image"]
        module.Sandbox = Sandbox
        module.configure = configure
        module.Image = Image
        module.FleetClient = FleetClient
        return module

    def test_collects_export_rooted_runtime_api(self):
        generator = load_generator()
        document = generator.collect_public_api(self.make_module())

        names = [item.name for item in document.items]
        self.assertEqual(names, ["Sandbox", "configure", "Image", "Tunnel"])
        sandbox = document.items[0]
        self.assertTrue(sandbox.context_manager)
        self.assertTrue(
            {"create", "delete", "start"}.issubset(member.name for member in sandbox.members)
        )
        self.assertNotIn("raw_request", [member.name for member in sandbox.members])
        self.assertNotIn("_ConnectResult", names)
        image = document.items[2]
        self.assertIn("linux", [member.name for member in image.members])
        tunnel = document.items[-1]
        self.assertTrue(tunnel.members[0].is_async)
        self.assertEqual(
            tunnel.members[0].raises, ("RuntimeError: When the service cannot be reached.",)
        )

    def test_rejects_excluded_public_exports(self):
        generator = load_generator()
        module = self.make_module()
        module.__all__.append("FleetClient")

        with self.assertRaisesRegex(ValueError, "unsupported public export: FleetClient"):
            generator.collect_public_api(module)

    def test_runtime_helper_requirements_are_pinned(self):
        requirements = [
            line.strip()
            for line in REQUIREMENTS.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        ]

        self.assertTrue(requirements)
        self.assertTrue(all("==" in requirement for requirement in requirements))

    def test_render_is_deterministic_and_check_reports_drift(self):
        generator = load_generator()
        document = generator.collect_public_api(self.make_module())
        first = generator.render_mdx(document)
        second = generator.render_mdx(document)
        self.assertEqual(first, second)
        self.assertIn("Async context manager", first)
        self.assertNotIn("FleetClient", first)
        self.assertNotIn("raw_request", first)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "index.mdx"
            output.write_text(first, encoding="utf-8")
            self.assertEqual(generator.check_output(output, first), 0)
            output.write_text("stale\n", encoding="utf-8")
            self.assertEqual(generator.check_output(output, first), 1)


if __name__ == "__main__":
    unittest.main()
