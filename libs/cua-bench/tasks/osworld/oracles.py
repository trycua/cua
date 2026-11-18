"""Scripted solutions for the OSWorld parity tasks (no model).

Each is a shell command run as ``user`` through the OSWorld server's
``/setup/execute`` (the same endpoint the task's own setup uses), so it
behaves the same on the VM and the container variant. The tasks were chosen
because their evaluators read file state through ``vm_command_line`` or
the task's own ``eval.sh`` (no web, no Google Drive, no GUI timing): a
scripted solve must score 1.0 and a no-op 0.0 on both variants. See
``libs/images/bench/osworld/parity.txt``.
"""

from __future__ import annotations

from typing import Any

#: task id -> shell command that solves it.
ORACLES: dict[str, str] = {
    # Rename ~/Desktop/todo_list_Jan_1 (created by setup with sudo) to _Jan_2.
    "e0df059f-28a6-4169-924f-b9623e7184cc":
        "mv ~/Desktop/todo_list_Jan_1 ~/Desktop/todo_list_Jan_2",
    # Write "1<br/>\n2<br/>\n3<br/>" to /home/user/output.txt.
    "5ced85fc-fa1a-4217-95fd-0fb530545ce2":
        "printf '1<br/>\\n2<br/>\\n3<br/>\\n' > /home/user/output.txt",
    # Every regular file under /home/user/testDir to 644.
    "4d117223-a354-47fb-8b45-62ab1390a95f":
        "find /home/user/testDir -type f -exec chmod 644 {} +",
    # Copy /home/user/file1 into dir1, dir2 and dir3.
    "6f56bf42-85b8-4fbb-8e06-6c44960184ba":
        "cd /home/user && for d in dir1 dir2 dir3; do cp file1 \"$d/\"; done",
}

#: The parity split, in order.
PARITY: list[str] = list(ORACLES)


class NoOracle(LookupError):
    pass


async def solve(task_id: str, ep: Any) -> None:
    command = ORACLES.get(task_id)
    if command is None:
        raise NoOracle(
            f"OSWorld task {task_id} has no scripted solution (only the parity split does: "
            "CUA_BENCH_OSWORLD_SPLIT=parity)"
        )
    r = await ep.request("server", "POST", "/setup/execute",
                         json={"command": command, "shell": True}, timeout=120)
    status = getattr(r, "status_code", 200)
    body = r.json() if hasattr(r, "json") else {}
    if status != 200 or body.get("returncode", 0) != 0:
        raise RuntimeError(f"oracle for {task_id} failed ({status}): {body}")
