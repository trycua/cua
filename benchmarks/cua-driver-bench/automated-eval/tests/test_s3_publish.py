from __future__ import annotations

import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

MODULE_PATH = Path(__file__).resolve().parents[1] / "s3_publish.py"
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("test_s3_publish_module", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
s3_publish = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = s3_publish
SPEC.loader.exec_module(s3_publish)

CLI_PATH = MODULE_PATH.with_name("cli.py")
CLI_SPEC = importlib.util.spec_from_file_location("test_s3_publish_cli", CLI_PATH)
assert CLI_SPEC is not None and CLI_SPEC.loader is not None
cli = importlib.util.module_from_spec(CLI_SPEC)
sys.modules[CLI_SPEC.name] = cli
CLI_SPEC.loader.exec_module(cli)


class _Response:
    def __enter__(self):
        return self

    def __exit__(self, *_arguments) -> None:
        return None

    def read(self, _size: int) -> bytes:
        return b"<"


class S3PublishTests(unittest.TestCase):
    def _report_bundle(self, root: Path) -> None:
        (root / "report" / "trials" / "trial" / "assets").mkdir(parents=True)
        (root / "report" / "index.html").write_text("index", encoding="utf-8")
        (root / "report" / "trials" / "trial" / "trajectory.html").write_text(
            "trajectory", encoding="utf-8"
        )
        (root / "report" / "trials" / "trial" / "assets" / "0001.jpg").write_bytes(b"jpg")
        (root / "comparison.md").write_text("markdown", encoding="utf-8")
        (root / "comparison.json").write_text("{}", encoding="utf-8")

    def test_publishes_only_report_bundle_and_comparison_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary).resolve()
            self._report_bundle(run_dir)
            secret = run_dir / ".env"
            secret.write_text("AWS_SECRET_ACCESS_KEY=secret", encoding="utf-8")
            before = {path: path.read_bytes() for path in run_dir.rglob("*") if path.is_file()}
            completed = subprocess.CompletedProcess([], 0, "", "")
            with (
                patch.object(s3_publish.shutil, "which", return_value="/usr/bin/aws"),
                patch.object(s3_publish, "_run_id", return_value="20260927T120000Z-a18f39c"),
                patch.object(s3_publish.subprocess, "run", return_value=completed) as run,
                patch.object(s3_publish.urllib.request, "urlopen", return_value=_Response()),
            ):
                url = s3_publish.publish_report(
                    run_dir,
                    bucket="cua-agent-artifacts",
                    prefix="cua-driver-bench",
                    profile="cua-artifacts",
                    region="us-west-2",
                )
            after = {path: path.read_bytes() for path in run_dir.rglob("*") if path.is_file()}

        self.assertEqual(before, after)
        self.assertEqual(
            url,
            "https://cua-agent-artifacts.s3.us-west-2.amazonaws.com/"
            "cua-driver-bench/20260927T120000Z-a18f39c/index.html",
        )
        self.assertEqual(run.call_count, 3)
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[0][1:3], ["s3", "sync"])
        self.assertIn(str(run_dir / "report"), commands[0])
        self.assertIn("--no-follow-symlinks", commands[0])
        self.assertEqual(commands[1][1:3], ["s3", "cp"])
        self.assertEqual(commands[2][1:3], ["s3", "cp"])
        self.assertNotIn(str(secret), " ".join(" ".join(command) for command in commands))
        for command in commands:
            self.assertEqual(
                command[-4:],
                ["--profile", "cua-artifacts", "--region", "us-west-2"],
            )

    def test_rejects_incomplete_report_before_running_aws(self) -> None:
        with (
            tempfile.TemporaryDirectory() as temporary,
            patch.object(s3_publish.subprocess, "run") as run,
            self.assertRaisesRegex(ValueError, "report bundle is incomplete"),
        ):
            s3_publish.publish_report(
                Path(temporary),
                bucket="cua-agent-artifacts",
                prefix="cua-driver-bench",
                profile=None,
                region="us-west-2",
            )
        run.assert_not_called()

    def test_reports_missing_aws_cli(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            self._report_bundle(run_dir)
            with (
                patch.object(s3_publish.shutil, "which", return_value=None),
                self.assertRaisesRegex(RuntimeError, "AWS CLI is unavailable"),
            ):
                s3_publish.publish_report(
                    run_dir,
                    bucket="cua-agent-artifacts",
                    prefix="cua-driver-bench",
                    profile=None,
                    region="us-west-2",
                )

    def test_public_access_failure_explains_multi_file_report(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            self._report_bundle(run_dir)
            completed = subprocess.CompletedProcess([], 0, "", "")
            forbidden = urllib.error.HTTPError(
                "https://example.invalid/index.html", 403, "Forbidden", {}, None
            )
            with (
                patch.object(s3_publish.shutil, "which", return_value="/usr/bin/aws"),
                patch.object(s3_publish, "_run_id", return_value="run"),
                patch.object(s3_publish.subprocess, "run", return_value=completed),
                patch.object(s3_publish.urllib.request, "urlopen", side_effect=forbidden),
                self.assertRaisesRegex(RuntimeError, "single presigned index.html"),
            ):
                s3_publish.publish_report(
                    run_dir,
                    bucket="cua-agent-artifacts",
                    prefix="cua-driver-bench",
                    profile=None,
                    region="us-west-2",
                )

    def test_upload_failure_includes_destination_and_aws_error(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            run_dir = Path(temporary)
            self._report_bundle(run_dir)
            failed = subprocess.CompletedProcess([], 1, "", "AccessDenied")
            with (
                patch.object(s3_publish.shutil, "which", return_value="/usr/bin/aws"),
                patch.object(s3_publish, "_run_id", return_value="run"),
                patch.object(s3_publish.subprocess, "run", return_value=failed),
                self.assertRaisesRegex(
                    RuntimeError,
                    r"s3://cua-agent-artifacts/cua-driver-bench/run/.*AccessDenied",
                ),
            ):
                s3_publish.publish_report(
                    run_dir,
                    bucket="cua-agent-artifacts",
                    prefix="cua-driver-bench",
                    profile=None,
                    region="us-west-2",
                )

    def test_url_encodes_each_object_key_component(self) -> None:
        self.assertEqual(
            s3_publish._report_url("bucket", "us-west-2", "reports/run with spaces/index.html"),
            "https://bucket.s3.us-west-2.amazonaws.com/reports/run%20with%20spaces/index.html",
        )

    def test_cli_supports_explicit_and_legacy_compare_modes(self) -> None:
        self.assertEqual(cli._command_mode(["compare", "--dry-run"]), ("compare", ["--dry-run"]))
        self.assertEqual(
            cli._command_mode(["publish", "--run-dir", "run"]),
            ("publish", ["--run-dir", "run"]),
        )
        self.assertEqual(cli._command_mode(["--dry-run"]), ("compare", ["--dry-run"]))

    def test_publish_command_prints_machine_readable_url(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = io.StringIO()
            with (
                patch.object(cli, "_load_environment"),
                patch.object(cli, "_publish_run", return_value="https://example.test/index.html"),
                redirect_stdout(output),
            ):
                exit_code = cli.main(["publish", "--run-dir", temporary])

        self.assertEqual(exit_code, 0)
        self.assertEqual(output.getvalue().strip(), "REPORT_URL=https://example.test/index.html")

    def test_publish_configuration_uses_safe_environment_values(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            environment = {
                "AWS_PROFILE": "cua-artifacts",
                "AWS_REGION": "us-west-2",
                "AWS_S3_BUCKET": "cua-agent-artifacts",
                "AWS_S3_REPORT_PREFIX": "cua-driver-bench",
            }
            with (
                patch.dict(os.environ, environment, clear=True),
                patch.object(cli, "publish_report", return_value="https://example.test") as publish,
            ):
                url = cli._publish_run(Path(temporary))

        self.assertEqual(url, "https://example.test")
        publish.assert_called_once_with(
            Path(temporary),
            bucket="cua-agent-artifacts",
            prefix="cua-driver-bench",
            profile="cua-artifacts",
            region="us-west-2",
        )


if __name__ == "__main__":
    unittest.main()
