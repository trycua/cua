"""``cb login``: sign in for ``--on cloud`` through the ``cua`` CLI.

Cloud runs authenticate like every cua tool: ``cua auth login`` (a stored
session), or ``CUA_CLIENT_ID``/``CUA_CLIENT_SECRET`` from
``cua auth keys create``, or ``FLEETS_TOKEN``.
"""

from __future__ import annotations

import shutil
import subprocess

RESET = "\033[0m"
GREEN = "\033[92m"
RED = "\033[91m"
GREY = "\033[90m"

INSTALL_HINT = "Install the cua CLI: curl -fsSL https://cua.ai/install.sh | sh"


def execute(args) -> int:
    cua = shutil.which("cua")
    if cua is None:
        print(f"{RED}The `cua` CLI is not on PATH.{RESET} {INSTALL_HINT}")
        print(f"{GREY}Or set CUA_CLIENT_ID and CUA_CLIENT_SECRET (or FLEETS_TOKEN).{RESET}")
        return 1
    cmd = [cua, "auth", "login"]
    if getattr(args, "no_browser", False):
        cmd.append("--no-browser")
    code = subprocess.call(cmd)
    if code == 0:
        print(f"{GREEN}✓ Signed in. Run tasks in the cloud with: cb run <task> --on cloud{RESET}")
    return code
