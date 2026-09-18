from __future__ import annotations

import hashlib
import os
import sys
import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.process import clean_environment, run_process
from cua_bench_runtime.signals import InterruptFlag


class ProcessTests(unittest.TestCase):
    def test_parent_secrets_are_not_inherited(self) -> None:
        previous = os.environ.get("CB_FAKE_SECRET")
        os.environ["CB_FAKE_SECRET"] = "do-not-copy"
        try:
            self.assertNotIn("CB_FAKE_SECRET", clean_environment())
        finally:
            if previous is None:
                os.environ.pop("CB_FAKE_SECRET", None)
            else:
                os.environ["CB_FAKE_SECRET"] = previous

    def test_streams_exact_stdin_bytes_without_argv_or_environment_transport(self) -> None:
        brief = b"exact task brief\x00\xff\n"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdin_path = root / "brief.bin"
            stdin_path.write_bytes(brief)
            argv = (
                sys.executable,
                "-c",
                "import hashlib,sys; print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())",
            )
            result = run_process(
                argv,
                cwd=root,
                stdout_path=root / "stdout",
                stderr_path=root / "stderr",
                timeout_seconds=5,
                interrupt=InterruptFlag(),
                stdin_path=stdin_path,
            )

            self.assertEqual(result.returncode, 0)
            self.assertEqual(
                result.stdout.read_text(encoding="utf-8").strip(),
                hashlib.sha256(brief).hexdigest(),
            )
            self.assertFalse(any(brief.decode("latin-1") in value for value in argv))
            self.assertNotIn(brief.decode("latin-1"), clean_environment().values())

    def test_uses_devnull_when_no_stdin_path_is_supplied(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            result = run_process(
                (
                    sys.executable,
                    "-c",
                    "import sys; print(len(sys.stdin.buffer.read()))",
                ),
                cwd=root,
                stdout_path=root / "stdout",
                stderr_path=root / "stderr",
                timeout_seconds=5,
                interrupt=InterruptFlag(),
            )

            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout.read_text(encoding="utf-8"), "0\n")

    def test_rejects_stdin_larger_than_the_bound_before_spawn(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stdin_path = root / "brief"
            stdin_path.write_bytes(b"four")
            with self.assertRaisesRegex(ValidationFailure, "exceeded 3 bytes"):
                run_process(
                    (sys.executable, "-c", "raise SystemExit(99)"),
                    cwd=root,
                    stdout_path=root / "stdout",
                    stderr_path=root / "stderr",
                    timeout_seconds=5,
                    interrupt=InterruptFlag(),
                    stdin_path=stdin_path,
                    stdin_limit=3,
                )
            self.assertFalse((root / "stdout").exists())
            self.assertFalse((root / "stderr").exists())

    def test_rejects_symlinked_stdin_before_spawn(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "brief-target"
            target.write_bytes(b"brief")
            stdin_path = root / "brief-link"
            try:
                stdin_path.symlink_to(target)
            except OSError as error:
                self.skipTest(f"symlink creation is unavailable: {error}")
            with self.assertRaisesRegex(ValidationFailure, "regular file"):
                run_process(
                    (sys.executable, "-c", "raise SystemExit(99)"),
                    cwd=root,
                    stdout_path=root / "stdout",
                    stderr_path=root / "stderr",
                    timeout_seconds=5,
                    interrupt=InterruptFlag(),
                    stdin_path=stdin_path,
                )
            self.assertFalse((root / "stdout").exists())
            self.assertFalse((root / "stderr").exists())


if __name__ == "__main__":
    unittest.main()
