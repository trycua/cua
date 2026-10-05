from __future__ import annotations

import asyncio
import importlib.util
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock, patch


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
        self.assertEqual(arguments[arguments.index("--max-parallel-tasks") + 1], "1")
        self.assertEqual(arguments[-2:], ["--task", "CDB-S01"])

    def test_bootstrap_cleans_stale_global_agent_install(self) -> None:
        config = SimpleNamespace(tasks=("CDB-S01",))

        command = fleet._bootstrap_command(config, "1.2.3")

        uninstall = command.index("npm uninstall --global @openai/codex")
        install_cleanup = command.index('rm -rf -- "$npm_root/@openai/codex"')
        stale_cleanup = command.index(".codex-*")
        install = command.index("npm install --global --no-audit --no-fund")
        self.assertLess(uninstall, install_cleanup)
        self.assertLess(install_cleanup, stale_cleanup)
        self.assertLess(uninstall, stale_cleanup)
        self.assertLess(stale_cleanup, install)

    def test_driver_check_supports_diagnostic_version_aliases(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            drivers_root = Path(temporary)
            releases = {
                "3719.0.0": {"version": "3719.0.0", "binaryVersion": "0.28.2"},
                "0.23.2": {"version": "0.23.2"},
            }
            for version, manifest in releases.items():
                release = drivers_root / version
                (release / "binary").mkdir(parents=True)
                binary = release / "binary" / "cua-driver"
                binary.write_text("", encoding="utf-8")
                binary.chmod(0o755)
                (release / "release-manifest.json").write_text(
                    json.dumps(manifest), encoding="utf-8"
                )
            config = SimpleNamespace(
                drivers_root=drivers_root,
                baseline="3719.0.0",
                candidate="0.23.2",
            )

            command = fleet._driver_check_command(config)

        self.assertIn("grep -F 0.28.2", command)
        self.assertIn("grep -F 0.23.2", command)

    def test_run_checked_retries_transport_errors_only(self) -> None:
        shell = SimpleNamespace(
            run=AsyncMock(
                side_effect=[
                    RuntimeError("transport dropped"),
                    SimpleNamespace(returncode=0, stdout="ok", stderr=""),
                ]
            )
        )
        worker = SimpleNamespace(shell=shell)

        with patch.object(fleet.asyncio, "sleep", new=AsyncMock()):
            result = asyncio.run(fleet._run_checked(worker, "true", "driver verification", 30))

        self.assertEqual(result.stdout, "ok")
        self.assertEqual(shell.run.await_count, 2)

    def test_existing_pool_is_scaled_to_parallel_capacity(self) -> None:
        pool = SimpleNamespace(resource=SimpleNamespace(spec=SimpleNamespace(replicas=1)))
        scaled = SimpleNamespace(resource=SimpleNamespace(spec=SimpleNamespace(replicas=2)))
        sandbox = SimpleNamespace(
            Pool=SimpleNamespace(
                get=AsyncMock(return_value=pool),
                apply=AsyncMock(return_value=scaled),
            ),
            Image=SimpleNamespace(from_registry=Mock(return_value=object())),
        )

        result = asyncio.run(fleet._get_or_create_pool(sandbox, "bench", 2))

        self.assertIs(result, scaled)
        self.assertEqual(sandbox.Pool.apply.await_args.kwargs["replicas"], 2)

    def test_existing_pool_keeps_larger_capacity(self) -> None:
        pool = SimpleNamespace(resource=SimpleNamespace(spec=SimpleNamespace(replicas=3)))
        sandbox = SimpleNamespace(
            Pool=SimpleNamespace(get=AsyncMock(return_value=pool), apply=AsyncMock()),
            Image=SimpleNamespace(from_registry=Mock()),
        )

        result = asyncio.run(fleet._get_or_create_pool(sandbox, "bench", 2))

        self.assertIs(result, pool)
        sandbox.Pool.apply.assert_not_awaited()

    def test_worker_finalization_retries_release_and_checks_secret_cleanup(self) -> None:
        worker = SimpleNamespace(
            shell=SimpleNamespace(
                run=AsyncMock(
                    return_value=SimpleNamespace(
                        returncode=1,
                        stdout="",
                        stderr="permission denied",
                    )
                )
            ),
            close=AsyncMock(side_effect=[RuntimeError("transport dropped"), None]),
        )

        with patch.object(fleet.asyncio, "sleep", new=AsyncMock()):
            errors = asyncio.run(fleet._finalize_worker(worker, True))

        self.assertEqual(worker.close.await_count, 2)
        self.assertEqual(len(errors), 1)
        self.assertIn("secret cleanup failed", errors[0])

    def test_background_command_uses_durable_detached_process(self) -> None:
        shell = SimpleNamespace(
            run=AsyncMock(
                side_effect=[
                    SimpleNamespace(returncode=0, stdout="", stderr=""),
                    SimpleNamespace(returncode=0, stdout="4321\n", stderr=""),
                ]
            )
        )
        files = SimpleNamespace(
            exists=AsyncMock(return_value=True),
            read_text=AsyncMock(return_value="0\n"),
        )
        worker = SimpleNamespace(shell=shell, files=files)

        result = asyncio.run(
            fleet._run_background_command(
                worker,
                "google-chrome --version",
                30,
                label="task application provisioning",
                stdout_path="/tmp/provision.stdout",
                stderr_path="/tmp/provision.stderr",
                exit_path="/tmp/provision.exit",
            )
        )

        self.assertEqual(result.returncode, 0)
        launch = shell.run.await_args_list[1]
        self.assertIn("nohup setsid -f bash -lc", launch.args[0])
        self.assertIn("trap finish EXIT", launch.args[0])
        self.assertIn("/tmp/provision.exit.pid", launch.args[0])
        self.assertEqual(launch.kwargs, {"background": True})

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
        self.assertIn("apt-get clean", command)
        self.assertIn("rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*", command)

    def test_aggregates_shards_canonically_and_preserves_infrastructure_failure(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "result"
            shard_output = output / "shards" / "CDB-S01"
            trial_values = []
            releases = tuple(SimpleNamespace(version=version) for version in ("0.22.2", "0.23.2"))
            for version in ("0.22.2", "0.23.2"):
                trial_id = f"CDB-S01-{version}-stamp"
                (shard_output / "trials" / trial_id).mkdir(parents=True)
                trial_values.append(
                    {
                        "task": "CDB-S01",
                        "version": version,
                        "trial_id": trial_id,
                        "trial_dir": f"/remote/{trial_id}",
                        "passed": True,
                        "score": 1.0,
                        "total_ms": 100,
                        "cua_calls": 2,
                        "input_actions": 1,
                        "termination": "completed",
                    }
                )
            (shard_output / "comparison.json").write_text(
                json.dumps(
                    {
                        "schema_version": "1",
                        "generated_at": "2026-10-05T00:00:00+00:00",
                        "diagnostic": True,
                        "certifying": False,
                        "platform": "linux",
                        "baseline": "0.22.2",
                        "candidate": "0.23.2",
                        "trials": trial_values,
                        "comparisons": [],
                    }
                ),
                encoding="utf-8",
            )
            results = (
                fleet.TaskShardResult(
                    shard=fleet.TaskShard("CDB-S01", releases),
                    output=shard_output,
                ),
                fleet.TaskShardResult(
                    shard=fleet.TaskShard("CDB-S02", releases),
                    error=RuntimeError("worker unavailable"),
                ),
            )
            config = SimpleNamespace(
                output=output,
                baseline="0.22.2",
                candidate="0.23.2",
                max_parallel_tasks=2,
            )

            def create_report_dir(_report, destination: Path) -> None:
                (destination / "report").mkdir(parents=True, exist_ok=True)

            with (
                patch.object(fleet, "write_html_bundle", side_effect=create_report_dir),
                patch.object(fleet, "render_markdown", return_value="# Comparison\n"),
            ):
                json_path, markdown_path = fleet._aggregate_shard_results(
                    config, results, "20261005T000000Z"
                )

            report = json.loads(json_path.read_text(encoding="utf-8"))
            self.assertTrue(markdown_path.is_file())
            self.assertEqual(
                [trial["task"] for trial in report["trials"]],
                ["CDB-S01", "CDB-S01", "CDB-S02", "CDB-S02"],
            )
            self.assertFalse(report["execution"]["complete"])
            self.assertEqual(
                report["execution"]["infrastructure_failures"][0]["task"],
                "CDB-S02",
            )
            self.assertEqual(
                [comparison["task"] for comparison in report["comparisons"]],
                ["CDB-S01", "CDB-S02"],
            )
            self.assertTrue((output / "trials" / "CDB-S01-0.22.2-stamp").is_dir())


if __name__ == "__main__":
    unittest.main()
