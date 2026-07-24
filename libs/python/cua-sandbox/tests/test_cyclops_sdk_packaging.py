import tomllib
import unittest
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[1]
PYPROJECT_PATH = PROJECT_ROOT / "pyproject.toml"
FLEET_IMPORTS = (
    PROJECT_ROOT / "cua_sandbox" / "transport" / "cyclops_http_client.py",
    PROJECT_ROOT / "cua_sandbox" / "transport" / "fleet.py",
    PROJECT_ROOT / "cua_sandbox" / "transport" / "fleet_cloud.py",
)


class CyclopsSdkPackagingTests(unittest.TestCase):
    def test_declares_packaged_cyclops_sdk_distribution(self):
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)

        self.assertIn("cua-train==0.1.1", project["project"]["dependencies"])
        self.assertEqual(project["tool"]["uv"]["sources"]["cua-train"], {"index": "cua-wheels"})
        self.assertEqual(
            project["tool"]["uv"]["index"][0],
            {
                "name": "cua-wheels",
                "url": "https://wheels.cua.ai/simple",
                "explicit": True,
            },
        )

    def test_package_does_not_copy_the_binding_from_the_checkout(self):
        self.assertFalse((PROJECT_ROOT / "hatch_build.py").exists())
        with PYPROJECT_PATH.open("rb") as pyproject_file:
            project = tomllib.load(pyproject_file)
        self.assertNotIn("hooks", project.get("tool", {}).get("hatch", {}).get("build", {}))

    def test_fleet_imports_do_not_mutate_paths_or_load_native_libraries(self):
        for source_path in FLEET_IMPORTS:
            source = source_path.read_text()
            self.assertIn("from cyclops_sdk import", source)
            self.assertNotIn("sys.path", source)
            self.assertNotIn("site.addsitedir", source)
            self.assertNotIn("ctypes.CDLL", source)


if __name__ == "__main__":
    unittest.main()
