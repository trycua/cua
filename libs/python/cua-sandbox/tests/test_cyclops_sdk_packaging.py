import tomllib
import unittest
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
PYPROJECT_PATH = PROJECT_ROOT / "pyproject.toml"
LOCK_PATH = PROJECT_ROOT / "uv.lock"
FLEET_IMPORTS = (
    PROJECT_ROOT / "cua_sandbox" / "transport" / "cyclops_http_client.py",
    PROJECT_ROOT / "cua_sandbox" / "transport" / "fleet.py",
    PROJECT_ROOT / "cua_sandbox" / "transport" / "fleet_cloud.py",
)


class CyclopsSdkPackagingTests(unittest.TestCase):
    def test_declares_fleet_without_a_direct_train_dependency(self):
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)

        dependencies = project["project"]["dependencies"]
        self.assertIn("cua-fleet==0.0.3", dependencies)
        self.assertFalse(any(dependency.startswith("cua-train") for dependency in dependencies))
        self.assertEqual(
            project["tool"]["uv"]["sources"]["cua-fleet"],
            {"path": "../cua-fleet", "editable": True},
        )
        self.assertNotIn("cua-train", project["tool"]["uv"]["sources"])
        self.assertEqual(
            project["tool"]["uv"]["index"][0],
            {
                "name": "cua-wheels",
                "url": "https://wheels.cua.ai/simple",
            },
        )

    def test_lock_keeps_train_only_as_a_fleet_transitive_dependency(self):
        with LOCK_PATH.open("rb") as lock_file:
            lock = tomllib.load(lock_file)

        packages = {package["name"]: package for package in lock["package"]}
        sandbox_dependencies = {
            dependency["name"] for dependency in packages["cua-sandbox"]["dependencies"]
        }
        sandbox_requires_dist = {
            requirement["name"]
            for requirement in packages["cua-sandbox"]["metadata"]["requires-dist"]
        }
        fleet_dependencies = {
            dependency["name"] for dependency in packages["cua-fleet"]["dependencies"]
        }
        fleet_requires_dist = {
            requirement["name"]
            for requirement in packages["cua-fleet"]["metadata"]["requires-dist"]
        }

        self.assertIn("cua-fleet", sandbox_dependencies)
        self.assertNotIn("cua-train", sandbox_dependencies)
        self.assertIn("cua-fleet", sandbox_requires_dist)
        self.assertNotIn("cua-train", sandbox_requires_dist)
        self.assertEqual(packages["cua-fleet"]["source"], {"editable": "../cua-fleet"})
        self.assertIn("cua-train", fleet_dependencies)
        self.assertEqual(fleet_requires_dist, {"cua-train"})
        self.assertEqual(packages["cua-train"]["version"], "0.1.1")
        self.assertEqual(
            packages["cua-train"]["source"],
            {"registry": "https://wheels.cua.ai/simple"},
        )

    def test_package_does_not_copy_the_binding_from_the_checkout(self):
        self.assertFalse((PROJECT_ROOT / "hatch_build.py").exists())
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)
        self.assertNotIn("hooks", project.get("tool", {}).get("hatch", {}).get("build", {}))

    def test_fleet_runtime_keeps_binding_imports_direct(self):
        for source_path in FLEET_IMPORTS:
            source = source_path.read_text()
            self.assertIn("from cyclops_sdk import", source)
            self.assertNotIn("from cua_train", source)
            self.assertNotIn("sys.path", source)
            self.assertNotIn("site.addsitedir", source)
            self.assertNotIn("ctypes.CDLL", source)


if __name__ == "__main__":
    unittest.main()
