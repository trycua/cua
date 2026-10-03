"""``cb env``: the sandboxes behind ``cb run``.

    cb env ls            managed cloud pools (cua-auto-*) and their claims
    cb env ls --local    local sandboxes
    cb env gc [--idle 30m]   delete idle managed pools and stuck claims now

Managed pools are created and reused by cua-sandbox for ``--on cloud`` runs;
they scale to zero when idle and are garbage collected after
``CUA_FLEET_POOL_IDLE_GC`` (30 min) automatically. ``gc`` just does it now.
"""

from __future__ import annotations
from ._help import examples

import asyncio
import shutil
import subprocess
from datetime import datetime, timezone
from typing import Any, Optional

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


def _age(value: Any) -> str:
    if value is None:
        return "-"
    if isinstance(value, str):
        try:
            value = datetime.fromisoformat(value.replace("Z", "+00:00"))
        except ValueError:
            return value
    if isinstance(value, datetime):
        if value.tzinfo is None:
            value = value.replace(tzinfo=timezone.utc)
        seconds = int((datetime.now(timezone.utc) - value).total_seconds())
        for unit, size in (("d", 86400), ("h", 3600), ("m", 60)):
            if seconds >= size:
                return f"{seconds // size}{unit} ago"
        return f"{max(seconds, 0)}s ago"
    return str(value)


def _cua_passthrough(args_list: list[str]) -> Optional[int]:
    cua = shutil.which("cua")
    if cua is None:
        return None
    return subprocess.call([cua, "fleet", "pools", *args_list])


def _require_auth() -> bool:
    from cua_bench.sandboxes import CloudAuthError, cloud_auth_source

    try:
        cloud_auth_source()
    except CloudAuthError as error:
        print(f"{RED}{error}{RESET}")
        return False
    return True


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


async def _list_cloud(autopool: Any) -> int:
    pools = await autopool.list_pools()
    claims = await autopool.list_claims()
    if not pools:
        print(f"{GREY}No managed pools. cb run <task> --on cloud creates one per image.{RESET}")
    else:
        print(f"{BOLD}{'POOL':<32}  {'READY/REPLICAS':<14}  {'CLAIMS':<6}  LAST USED{RESET}")
        for pool in pools:
            ready = f"{pool.ready_replicas or 0}/{pool.replicas or 0}"
            print(f"{pool.name:<32}  {ready:<14}  {pool.claims:<6}  {_age(pool.last_used)}")
    managed = [c for c in claims if c.managed]
    if managed:
        print(f"\n{BOLD}{'CLAIM':<40}  {'POOL':<32}  PHASE{RESET}")
        for claim in managed:
            print(f"{claim.name:<40}  {claim.pool:<32}  {claim.phase or '-'}")
    return 0


async def _gc(autopool: Any, idle: str) -> int:
    from cua_bench.targets import parse_duration_s

    report = await autopool.gc(parse_duration_s(idle))
    for name in report.pools_deleted:
        print(f"{GREEN}deleted pool{RESET} {name}")
    for name in report.claims_deleted:
        print(f"{GREEN}deleted claim{RESET} {name}")
    for error in report.errors:
        print(f"{RED}error{RESET} {error}")
    if not (report.pools_deleted or report.claims_deleted or report.errors):
        print(f"{GREY}Nothing to collect.{RESET}")
    return 1 if report.errors else 0


def execute(args) -> int:
    from cua_bench.sandboxes import pools_module
    from cua_bench.targets import TargetError

    command = getattr(args, "env_command", None)
    if command in ("ls", "list") and getattr(args, "local", False):
        return asyncio.run(_list_local())
    if command not in ("ls", "list", "gc"):
        print("Usage: cb env ls [--local] | cb env gc [--idle 30m]")
        return 1
    if not _require_auth():
        return 1
    try:
        pools = pools_module()
    except ImportError:
        # An older cua-sandbox without the public pools API: the `cua` CLI.
        passthrough = ["gc", "--idle", args.idle] if command == "gc" else ["ls"]
        code = _cua_passthrough(passthrough)
        if code is None:
            print(
                f"{RED}Managed pools need cua-sandbox >= 0.9 (cua_sandbox.pools) or the `cua` "
                f"CLI on PATH (cua fleet pools {' '.join(passthrough)}).{RESET}"
            )
            return 1
        return code
    try:
        if command == "gc":
            return asyncio.run(_gc(pools, args.idle))
        return asyncio.run(_list_cloud(pools))
    except TargetError as error:
        print(f"{RED}Error: {error}{RESET}")
        return 1
