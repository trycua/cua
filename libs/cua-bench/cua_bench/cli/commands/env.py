"""``cb env``: the sandboxes behind ``cb run``.

    cb env ls --local    local sandboxes
    cb env ls | gc       managed cloud pools: Cua Cloud has ended, so they fail

Managed cloud pools belonged to Cua Cloud, which has ended: ``cb env ls``
(without ``--local``) and ``cb env gc`` print the one "Cua Cloud has ended"
error.
"""

from __future__ import annotations

import asyncio

from cua_bench._ended import CLOUD_ENDED

from ._help import examples

RESET = "\033[0m"
BOLD = "\033[1m"
GREEN = "\033[92m"
RED = "\033[91m"
GREY = "\033[90m"


def register_parser(subparsers) -> None:
    parser = subparsers.add_parser(
        "env",
        help="List or garbage-collect sandboxes and pools",
        **examples(
            ("List managed cloud pools and claims", "cb env ls"),
            ("Delete pools idle for an hour", "cb env gc --idle 1h"),
        ),
    )
    sub = parser.add_subparsers(dest="env_command")
    ls = sub.add_parser(
        "ls",
        aliases=["list"],
        help="List managed cloud pools and claims",
        **examples(
            ("List managed cloud pools and claims", "cb env ls"),
            ("List local sandboxes", "cb env ls --local"),
        ),
    )
    ls.add_argument("--local", action="store_true", help="List local sandboxes instead")
    gc = sub.add_parser(
        "gc",
        help="Delete idle managed pools and stuck claims",
        **examples(
            ("Delete pools unused for 30 minutes", "cb env gc"),
            ("Delete pools idle for two hours", "cb env gc --idle 2h"),
        ),
    )
    gc.add_argument("--idle", default="30m", help="Delete pools unused for this long (default 30m)")


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
    command = getattr(args, "env_command", None)
    if command in ("ls", "list") and getattr(args, "local", False):
        return asyncio.run(_list_local())
    if command not in ("ls", "list", "gc"):
        print("Usage: cb env ls [--local] | cb env gc [--idle 30m]")
        return 1
    # Cua Cloud has ended: there are no managed pools to list or collect.
    print(f"{RED}Error: {CLOUD_ENDED}{RESET}")
    return 1
