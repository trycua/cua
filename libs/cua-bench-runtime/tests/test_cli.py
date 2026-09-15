from __future__ import annotations

import contextlib
import io
import json
import unittest

from cua_bench_runtime import exit_codes
from cua_bench_runtime.cli import main


class CliTests(unittest.TestCase):
    def test_usage_error_has_stable_exit_code(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            code = main(["run"])
        self.assertEqual(code, exit_codes.USAGE)

    def test_repository_validation_json_is_machine_readable(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            code = main(["validate", "--json"])
        self.assertEqual(code, exit_codes.OK)
        self.assertTrue(json.loads(stdout.getvalue())["valid"])


if __name__ == "__main__":
    unittest.main()
