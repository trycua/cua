"""Explicit disposable fixtures for release-uninstaller ownership tests."""

import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

import pytest


REPO_ROOT = Path(__file__).resolve().parents[4]
UNINSTALL = REPO_ROOT / "libs/cua-driver/scripts/uninstall.sh"


class ReleaseInstall:
    def __init__(self, root):
        self.home = root / "home"
        self.bin = root / "fake-bin"
        self.release = self.home / ".cua-driver/packages/current/cua-driver"
        self.launcher = self.home / ".local/bin/cua-driver"
        self.config = self.home / ".claude.json"
        self.executable(self.release)
        self.launcher.parent.mkdir(parents=True)
        self.launcher.symlink_to(self.release)
        for name, body in {
            "uname": "printf 'Linux\\n'",
            "pgrep": "exit 1",
            "id": 'if [ "$1" = -u ]; then printf "1000\\n"; else /usr/bin/id "$@"; fi',
            "pkill": "exit 0",
            "systemctl": "exit 0",
        }.items():
            self.executable(self.bin / name, body)
        # Real rm semantics, guarded against any removal outside this fixture.
        guard = self.bin / "safe_rm.py"
        guard.write_text(
            "import os, pathlib, sys\n"
            f"root = pathlib.Path({str(root.resolve())!r})\n"
            "for arg in sys.argv[1:]:\n"
            "    if arg.startswith('-'): continue\n"
            "    parent = pathlib.Path(arg).parent.resolve()\n"
            "    assert root == parent or root in parent.parents, arg\n"
            "os.execv('/bin/rm', ['/bin/rm', *sys.argv[1:]])\n"
        )
        self.executable(self.bin / "rm", f"exec {shlex.quote(sys.executable)} {shlex.quote(str(guard))} \"$@\"")
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("CUA_DRIVER_")}
        self.env.update(HOME=str(self.home), PATH=f"{self.bin}:/usr/bin:/bin")

    @staticmethod
    def executable(path, body="exit 0"):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
        path.chmod(0o755)

    def write_config(self, data):
        self.config.write_text(json.dumps(data), encoding="utf-8")
        return self.config

    def run(self, *args, cwd=REPO_ROOT):
        return subprocess.run(["/bin/bash", str(UNINSTALL), *args], cwd=cwd,
                              env=self.env, text=True, capture_output=True, check=False)


@pytest.fixture
def release_install(tmp_path):
    return ReleaseInstall(tmp_path)
