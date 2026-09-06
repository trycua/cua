"""Exercise the documented commands with synthetic secrets and a local shell.

Run with Python and cua-sandbox==0.4.3 installed. No sandbox is provisioned;
the released Shell interface sends commands to a disposable local test transport.
"""

import asyncio
import importlib.metadata
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

from cua_sandbox.interfaces.shell import Shell


PAGE = Path(__file__).resolve().parents[2] / "content/docs/how-to-guides/sandbox/secrets.mdx"
BLOCKS = re.findall(r"```python\n(.*?)\n```", PAGE.read_text(), re.DOTALL)
SYNTHETIC = "synthetic space 'quote' \"double\" $HOME $(exit 91) `exit 92`\nEOF\nlast\n"


class LocalTestTransport:
    """Run each command in a new shell; replace only the sample application."""

    def __init__(self, consumer):
        self.consumer = consumer
        self.results = []

    async def send(self, action, *, command, timeout):
        assert action == "run_command"
        consumer = f"{shlex.quote(sys.executable)} -c {shlex.quote(self.consumer)}"
        command = command.replace("python3 /app/main.py", consumer)
        result = subprocess.run(
            ["/bin/sh", "-c", command],
            capture_output=True,
            text=True,
            timeout=timeout,
            env={"PATH": "/usr/bin:/bin"},
        )
        self.results.append(result)
        return result


class SecretExampleTests(unittest.TestCase):
    def load_example(self, function_name):
        block = next(block for block in BLOCKS if f"async def {function_name}(" in block)
        namespace = {}
        exec(compile(block, str(PAGE), "exec"), namespace)
        return namespace[function_name]

    def test_released_api_and_python_syntax(self):
        self.assertEqual(importlib.metadata.version("cua-sandbox"), "0.4.3")
        for block in BLOCKS:
            compile(block, str(PAGE), "exec")

    def test_environment_round_trip_and_lifetime(self):
        transport = LocalTestTransport(
            "import json, os; print(json.dumps([os.environ['DATABASE_URL'], "
            "os.environ['GITHUB_TOKEN']]))"
        )
        shell = Shell(transport)
        with patch.dict(os.environ, DATABASE_URL=SYNTHETIC, GITHUB_TOKEN=SYNTHETIC):
            asyncio.run(self.load_example("run_with_secrets")(types.SimpleNamespace(shell=shell)))
        self.assertEqual(json.loads(transport.results[0].stdout), [SYNTHETIC, SYNTHETIC])
        later = asyncio.run(shell.run('test -z "${DATABASE_URL+x}${GITHUB_TOKEN+x}"'))
        self.assertTrue(later.success)

    def test_separate_export_does_not_persist(self):
        shell = Shell(LocalTestTransport(""))
        exported = asyncio.run(shell.run(f"export CUA_TEST_SECRET={shlex.quote(SYNTHETIC)}"))
        self.assertTrue(exported.success)
        later = asyncio.run(shell.run('test -z "${CUA_TEST_SECRET+x}"'))
        self.assertTrue(later.success)

    def test_key_bytes_permissions_and_cleanup_on_success_and_failure(self):
        for exit_code in (0, 7):
            with self.subTest(exit_code=exit_code), tempfile.TemporaryDirectory() as host_dir:
                host_key = Path(host_dir) / "task-key"
                host_key.write_bytes(SYNTHETIC.encode())
                host_key.chmod(0o600)
                consumer = (
                    "import json, os, pathlib, stat, sys; "
                    "p = pathlib.Path(os.environ['TASK_SSH_KEY_FILE']); "
                    "print(json.dumps([str(p), p.read_bytes().decode(), "
                    "stat.S_IMODE(p.stat().st_mode), stat.S_IMODE(p.parent.stat().st_mode)])); "
                    f"sys.exit({exit_code})"
                )
                transport = LocalTestTransport(consumer)
                sandbox = types.SimpleNamespace(shell=Shell(transport))
                with patch.dict(os.environ, TASK_SSH_KEY_FILE=str(host_key)):
                    task = self.load_example("run_with_key_file")(sandbox)
                    if exit_code:
                        with self.assertRaises(RuntimeError):
                            asyncio.run(task)
                    else:
                        asyncio.run(task)
                path, content, mode, directory_mode = json.loads(transport.results[0].stdout)
                self.assertEqual(content, SYNTHETIC)
                self.assertEqual((mode, directory_mode), (0o600, 0o700))
                self.assertFalse(Path(path).exists())
                self.assertFalse(Path(path).parent.exists())
                self.assertEqual(host_key.read_bytes(), SYNTHETIC.encode())


if __name__ == "__main__":
    unittest.main()
