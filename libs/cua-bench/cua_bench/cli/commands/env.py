"""``cb env``: the sandboxes behind ``cb run``.

    cb env ls            local sandboxes
"""

from __future__ import annotations

import asyncio

from ._help import examples

RESET = "\033[0m"
BOLD = "\033[1m"
GREY = "\033[90m"


def register_parser(subparsers) -> None:
    parser = subparsers.add_parser(
        "env",
        help="List local sandboxes",
        **examples(("List local sandboxes", "cb env ls")),
    )
    sub = parser.add_subparsers(dest="env_command")
    ls = sub.add_parser(
        "ls",
        aliases=["list"],
        help="List local sandboxes",
        **examples(("List local sandboxes", "cb env ls")),
    )
    ls.add_argument(
        "--local", action="store_true", help="List local sandboxes (the default; kept as a no-op)"
    )


async def _list_local() -> int:
    from cua_sandbox import Sandbox

    sandboxes = await Sandbox.list(local=True)
    if not sandboxes:
        print(f"{GREY}No local sandboxes.{RESET}")
        return 0
    # Sandbox refs (`local:<name>`), the ids every cua surface prints.
    print(f"{BOLD}{'ID':<40}  {'STATUS':<12}  SOURCE{RESET}")
    for sb in sandboxes:
        print(f"{sb.id or sb.name:<40}  {sb.status:<12}  {sb.source}")
    return 0


def execute(args) -> int:
    if getattr(args, "env_command", None) not in ("ls", "list"):
        print("Usage: cb env ls")
        return 1
    return asyncio.run(_list_local())
