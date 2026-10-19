import tomllib
import unittest
from importlib.metadata import distribution
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
PYPROJECT_PATH = PROJECT_ROOT / "pyproject.toml"
LOCK_PATH = PROJECT_ROOT / "uv.lock"
CUA_SDK_PYPROJECT_PATH = PROJECT_ROOT.parents[1] / "cua" / "python" / "pyproject.toml"
PACKAGE_DIR = PROJECT_ROOT / "cua_sandbox"


class SdkPackagingTests(unittest.TestCase):
    """cua-sandbox's one binding is the ``cua`` SDK (spacesd and the local
    runtimes). It installs from PyPI alone: no extra index, no Cua Cloud
    packages."""

    def test_installs_from_pypi_alone(self):
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)

        dependencies = project["project"]["dependencies"]
        self.assertFalse(any(dependency.startswith("cua-fleet") for dependency in dependencies))
        self.assertFalse(any(dependency.startswith("cua-train") for dependency in dependencies))
        uv = project["tool"]["uv"]
        self.assertNotIn("index", uv)
        self.assertNotIn("cua-fleet", uv["sources"])
        self.assertNotIn("pdm", project["tool"])
        self.assertNotIn("wheels.cua.ai", PYPROJECT_PATH.read_text())

    def test_lock_has_no_cloud_packages_and_no_extra_index(self):
        lock_text = LOCK_PATH.read_text()
        with LOCK_PATH.open("rb") as lock_file:
            lock = tomllib.load(lock_file)

        packages = {package["name"]: package for package in lock["package"]}
        sandbox_dependencies = {
            dependency["name"] for dependency in packages["cua-sandbox"]["dependencies"]
        }
        self.assertNotIn("cua-fleet", packages)
        self.assertNotIn("cua-fleet", sandbox_dependencies)
        self.assertNotIn("cua-train", packages)
        self.assertNotIn("wheels.cua.ai", lock_text)

    def test_depends_on_the_cua_sdk_for_the_data_plane(self):
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)

        dependencies = project["project"]["dependencies"]
        self.assertIn("cua>=0.2.0", dependencies)
        # In the monorepo the SDK binding is built from libs/cua/python.
        self.assertEqual(
            project["tool"]["uv"]["sources"]["cua"],
            {"path": "../../cua/python", "editable": True},
        )
        # computer-server's WebSocket client is gone with its transport.
        self.assertFalse(any(dep.startswith("websockets") for dep in dependencies))

    def test_lock_resolves_the_cua_sdk_from_the_checkout(self):
        with LOCK_PATH.open("rb") as lock_file:
            lock = tomllib.load(lock_file)

        packages = {package["name"]: package for package in lock["package"]}
        sandbox_dependencies = {
            dependency["name"] for dependency in packages["cua-sandbox"]["dependencies"]
        }
        self.assertIn("cua", sandbox_dependencies)
        self.assertEqual(packages["cua"]["source"], {"editable": "../../cua/python"})
        # The lock pins whatever version the checkout's SDK declares.
        with CUA_SDK_PYPROJECT_PATH.open("rb") as sdk_file:
            sdk_version = tomllib.load(sdk_file)["project"]["version"]
        self.assertEqual(packages["cua"]["version"], sdk_version)

    def test_package_does_not_copy_the_binding_from_the_checkout(self):
        self.assertFalse((PROJECT_ROOT / "hatch_build.py").exists())
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)
        self.assertNotIn("hooks", project.get("tool", {}).get("hatch", {}).get("build", {}))

    def test_source_never_imports_the_fleet_sdk(self):
        for source_path in PACKAGE_DIR.rglob("*.py"):
            source = source_path.read_text()
            self.assertNotIn("fleet_sdk", source, source_path)
            self.assertNotIn("import cua_fleet", source, source_path)

    def test_cua_sdk_is_the_native_binding_distribution(self):
        """cua-sandbox's data plane is the cua SDK (``cua`` >= 0.2, UniFFI over
        cua-sdk), not the old ``cua`` meta-package (0.1.x, pure re-exports)."""
        import cua

        cua_distribution = distribution("cua")
        major, minor = (int(part) for part in cua_distribution.version.split(".")[:2])

        self.assertGreaterEqual((major, minor), (0, 2))
        self.assertTrue(hasattr(cua, "_native") and hasattr(cua._native, "Cua"))
        self.assertTrue(hasattr(cua._native, "SpacesdClient"))


if __name__ == "__main__":
    unittest.main()
