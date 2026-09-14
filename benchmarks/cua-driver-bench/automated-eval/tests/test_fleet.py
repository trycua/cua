from __future__ import annotations

import importlib.util
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from types import SimpleNamespace


MODULE_PATH = Path(__file__).resolve().parents[1] / "fleet.py"
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("test_fleet_module", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
fleet = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = fleet
SPEC.loader.exec_module(fleet)


class FleetHelpersTests(unittest.TestCase):
    def _write_task(self, root: Path, task: str, prerequisite_commands: tuple[str, ...]) -> None:
        bundle = root / "shared" / task.casefold()
        (bundle / "platform").mkdir(parents=True)
        (bundle / "task.cuabench.json").write_text("{}", encoding="utf-8")
        descriptor = {
            "platform": "linux",
            "apps": [],
            "prerequisites": [
                {"id": command, "check": [command, "--version"]}
                for command in prerequisite_commands
            ],
        }
        (bundle / "platform" / "launch.linux.json").write_text(
            json.dumps(descriptor), encoding="utf-8"
        )

    def test_archive_excludes_secrets_generated_files_and_dependencies(self) -> None:
        for path in (
            ".env",
            "automated-eval/.env.local",
            "artifacts/run/comparison.json",
            "tasks/shared/cdb-s01/task.cuabench.json",
            "tasks/shared/cdb-s01/apps/app/node_modules/electron/index.js",
            "cua-drivers/0.23.2/binary/cua-driver",
        ):
            self.assertFalse(fleet._archive_member_allowed(PurePosixPath(path)))
        self.assertTrue(fleet._archive_member_allowed(PurePosixPath("automated-eval/.env.example")))

    def test_repo_archive_contains_benchmark_and_runtime_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            workspace = Path(temporary) / "cua"
            benchmark = workspace / "benchmarks" / "cua-driver-bench"
            runtime = workspace / "libs" / "cua-bench-runtime"
            (benchmark / "automated-eval").mkdir(parents=True)
            (benchmark / "automated-eval" / "cli.py").write_text("", encoding="utf-8")
            (benchmark / "tasks").mkdir()
            (benchmark / "tasks" / "private.json").write_text("{}", encoding="utf-8")
            (runtime / "src" / "cua_bench_runtime").mkdir(parents=True)
            (runtime / "src" / "cua_bench_runtime" / "__init__.py").write_text("", encoding="utf-8")
            destination = Path(temporary) / "repo.tar.gz"

            fleet._create_repo_archive(benchmark, destination)

            with tarfile.open(destination, "r:gz") as archive:
                names = set(archive.getnames())

        self.assertIn("cua/benchmarks/cua-driver-bench/automated-eval/cli.py", names)
        self.assertIn("cua/libs/cua-bench-runtime/src/cua_bench_runtime/__init__.py", names)
        self.assertFalse(any(name.endswith("tasks/private.json") for name in names))

    def test_remote_arguments_preserve_single_release_shape(self) -> None:
        config = SimpleNamespace(
            baseline="0.23.2",
            candidate=None,
            model="small",
            reasoning_effort="high",
            timeout_seconds=1800.0,
            tasks=("CDB-S01",),
        )
        arguments = fleet._remote_cli_arguments(config, "/tmp/results")

        self.assertNotIn("--candidate", arguments)
        self.assertEqual(arguments[0], f"{fleet.REMOTE_WORKSPACE}/.fleet-venv/bin/python")
        self.assertEqual(arguments[arguments.index("--tasks-root") + 1], fleet.REMOTE_TASKS_ROOT)
        self.assertEqual(arguments[-2:], ["--task", "CDB-S01"])

    def test_task_archive_contains_only_selected_task_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            selected = root / "tasks" / "shared" / "cdb-s01"
            selected.mkdir(parents=True)
            (selected / "task.cuabench.json").write_text("{}", encoding="utf-8")
            dependencies = selected / "apps" / "app" / "node_modules"
            dependencies.mkdir(parents=True)
            (dependencies / "ignored.js").write_text("", encoding="utf-8")
            other = root / "tasks" / "shared" / "cdb-s02"
            other.mkdir(parents=True)
            (other / "task.cuabench.json").write_text("{}", encoding="utf-8")
            destination = root / "tasks.tar.gz"

            fleet._create_task_archive(
                SimpleNamespace(tasks_root=root / "tasks", tasks=("CDB-S01",)),
                destination,
            )

            with tarfile.open(destination, "r:gz") as archive:
                names = set(archive.getnames())

        self.assertIn("shared/cdb-s01/task.cuabench.json", names)
        self.assertFalse(any("node_modules" in name for name in names))
        self.assertFalse(any("cdb-s02" in name for name in names))

    def test_provisions_only_apps_required_by_selected_tasks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            tasks_root = Path(temporary) / "tasks"
            self._write_task(tasks_root, "CDB-S01", ("python3",))
            self._write_task(tasks_root, "CDB-S04", ("google-chrome",))
            config = SimpleNamespace(tasks_root=tasks_root, tasks=("CDB-S01", "CDB-S04"))

            applications = fleet._required_fleet_applications(config)
            command = fleet._application_provision_command(config)

        self.assertEqual(applications, ("google-chrome",))
        self.assertIsNotNone(command)
        assert command is not None
        self.assertIn("google-chrome-stable_current_amd64.deb", command)
        self.assertNotIn("flatpak install", command)

    def test_provisions_flatpak_wrappers_for_office_tasks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            tasks_root = Path(temporary) / "tasks"
            self._write_task(tasks_root, "CDB-S03", ("libreoffice", "gnucash"))
            config = SimpleNamespace(tasks_root=tasks_root, tasks=("CDB-S03",))

            applications = fleet._required_fleet_applications(config)
            command = fleet._application_provision_command(config)

        self.assertEqual(applications, ("libreoffice", "gnucash"))
        self.assertIsNotNone(command)
        assert command is not None
        self.assertIn("org.libreoffice.LibreOffice", command)
        self.assertIn("org.gnucash.GnuCash", command)
        self.assertIn("/usr/local/bin/libreoffice", command)
        self.assertIn("/usr/local/bin/gnucash", command)


if __name__ == "__main__":
    unittest.main()
