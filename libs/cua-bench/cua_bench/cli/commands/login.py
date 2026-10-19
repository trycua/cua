"""``cb login``: removed with Cua Cloud.

``cb login`` signed in for ``--on cloud``. Cua Cloud has ended, so the command
prints the one "Cua Cloud has ended" error.
"""

from __future__ import annotations

from cua_bench._ended import CLOUD_ENDED

RESET = "\033[0m"
RED = "\033[91m"


def execute(args) -> int:
    # Cua Cloud has ended: there is nothing to sign in to for --on cloud.
    print(f"{RED}Error: {CLOUD_ENDED}{RESET}")
    return 1
