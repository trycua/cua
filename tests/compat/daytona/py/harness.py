"""Shared-case conformance harness for the Daytona SDK (Python, sync and async).

A case is registered once with `@case(id, needs=...)` and runs against both the
sync `Daytona` and the async `AsyncDaytona` client: it is written as a
coroutine and awaits every SDK call through `c.w(...)`, which is the identity
in sync mode and `await` in async mode. The TypeScript runner (../ts) registers
the same ids; `../cases.json` is the catalogue both are checked against.

A case asserts the behaviour the SDK documents (derived from the SDK sources,
see cases.json `ref`), then returns a JSON-able observation. Observations are
compared with the committed golden (../fixtures/daytona-<sdk>.json) so the
Python sync, Python async and TypeScript runs, and the pinned and latest SDKs,
all have to agree.
"""

from __future__ import annotations

import asyncio
import inspect
import json
import os
import sys
import time
import traceback
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable

CASES: dict[str, "Case"] = {}


@dataclass
class Case:
    id: str
    fn: Callable[["Ctx"], Awaitable[Any]]
    needs: str  # "mock": runs over the mock guest; "space": needs a real guest shell
    group: str
    modes: tuple[str, ...] = ("sync", "async")
    timeout: float = 180.0
    ref: str = ""
    #: environment variable that must be "1" for the case to run (else it is
    #: reported skipped with the reason): a capability of the runner's host
    #: that no gateway can supply, for example an OS trust store.
    requires: str = ""
    why: str = ""


def case(
    id: str,
    needs: str = "space",
    ref: str = "",
    modes: tuple[str, ...] = ("sync", "async"),
    timeout: float = 180.0,
    requires: str = "",
    why: str = "",
):
    """`ref`: the SDK source the expected behaviour is derived from."""

    def deco(fn):
        CASES[id] = Case(id, fn, needs, id.split(".")[0], modes, timeout, ref, requires, why)
        return fn

    return deco


async def _drive(coro):
    return await coro


def run_coro_sync(coro):
    """Runs a coroutine that never truly suspends (sync-mode cases)."""
    try:
        coro.send(None)
    except StopIteration as e:
        return e.value
    raise RuntimeError("a sync-mode case suspended")


@dataclass
class Ctx:
    mode: str
    daytona: Any
    sdk: Any  # the `daytona` module
    made: list = field(default_factory=list)
    _main: Any = None
    run_id: str = field(default_factory=lambda: f"{os.getpid()}{int(time.time()) % 100000}")
    needs: str = "space"

    @property
    def is_async(self) -> bool:
        return self.mode == "async"

    async def w(self, v):
        """Awaits `v` in async mode; the identity in sync mode."""
        if inspect.isawaitable(v):
            return await v
        return v

    async def collect(self, it):
        """All items of an iterator (sync generator or async generator)."""
        if hasattr(it, "__aiter__"):
            return [x async for x in it]
        return list(it)

    async def list(self, **query):
        """`daytona.list(ListSandboxesQuery(**query))` as a list in both modes."""
        return await self.collect(self.daytona.list(self.sdk.ListSandboxesQuery(**query)))

    async def new(self, params=None, **kw):
        params = params if params is not None else self.sdk.CreateSandboxFromSnapshotParams(**kw)
        sb = await self.w(self.daytona.create(params, timeout=300))
        self.made.append(sb)
        return sb

    async def main(self):
        """The suite's shared sandbox (created on first use)."""
        if self._main is None:
            self._main = await self.new(
                labels={"suite": "conformance", "run": self.run_id}, auto_stop_interval=0
            )
        return self._main

    async def drop(self, sb):
        try:
            await self.w(self.daytona.delete(sb))
        except Exception:  # noqa: BLE001
            pass
        if sb in self.made:
            self.made.remove(sb)

    async def cleanup(self):
        for sb in list(self.made):
            await self.drop(sb)

    def name(self, stem: str) -> str:
        return f"{stem}-{self.run_id}-{int(time.time() * 1000) % 100000}"

    async def raises(self, fn, *args, **kw):
        """Calls `fn`; returns the error as an observation (None when it did not raise)."""
        try:
            await self.w(fn(*args, **kw))
        except BaseException as e:  # noqa: BLE001
            return describe_error(e)
        return None


def describe_error(e: BaseException) -> dict:
    out = {"type": type(e).__name__}
    # Some SDK methods let the generated client's ApiException through
    # (`status`, `body`) instead of a DaytonaError (`status_code`).
    if getattr(e, "status_code", None) is None and isinstance(getattr(e, "status", None), int):
        out["status_code"] = e.status
    for k in ("status_code", "code", "source"):
        v = getattr(e, k, None)
        if v is not None:
            out[k] = v
    msg = getattr(e, "message", None) or str(e)
    out["message"] = str(msg)
    return out


def err_obs(o: dict | None, *, keep_message: bool = False) -> dict | None:
    """An error observation without the message (messages are asserted by substring in the case)."""
    if o is None:
        return None
    return {k: v for k, v in o.items() if keep_message or k != "message"}


def load_env(path: str | None):
    if path:
        os.environ.update(json.loads(open(path).read()))


async def run_cases(mode: str, selected: list[str], needs_filter: set[str], emit) -> dict:
    import daytona as sdk

    results: dict[str, dict] = {}
    if mode == "async":
        client = sdk.AsyncDaytona(sdk.DaytonaConfig())
    else:
        client = sdk.Daytona(sdk.DaytonaConfig())
    ctx = Ctx(mode=mode, daytona=client, sdk=sdk)
    try:
        for cid in selected:
            c = CASES[cid]
            if c.needs not in needs_filter or mode not in c.modes:
                continue
            ctx.needs = c.needs
            if c.requires and os.environ.get(c.requires) != "1":
                results[cid] = {
                    "ok": True,
                    "skipped": c.why or f"needs {c.requires}=1",
                    "obs": None,
                    "seconds": 0,
                }
                emit(f"SKIP {cid}: {c.why or c.requires}")
                continue
            t0 = time.time()
            try:
                if mode == "async":
                    obs = await asyncio.wait_for(c.fn(ctx), c.timeout)
                else:
                    obs = run_coro_sync(c.fn(ctx))
                results[cid] = {"ok": True, "obs": obs, "seconds": round(time.time() - t0, 2)}
                emit(f"PASS {cid} ({time.time() - t0:.1f}s)")
            except BaseException as e:  # noqa: BLE001
                tb = traceback.format_exc()
                results[cid] = {
                    "ok": False,
                    "error": f"{type(e).__name__}: {e}",
                    "seconds": round(time.time() - t0, 2),
                }
                emit(f"FAIL {cid}: {type(e).__name__}: {e}\n{tb}")
    finally:
        try:
            await ctx.cleanup()
        finally:
            try:
                if mode == "async":
                    await client.close()
                else:
                    client.close() if hasattr(client, "close") else None
            except Exception:  # noqa: BLE001
                pass
    return results


def main(argv=None):
    import argparse
    import importlib
    import pkgutil

    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["sync", "async"], default="sync")
    ap.add_argument("--env", help="JSON file of client environment (from the gateway)")
    ap.add_argument("--needs", default="mock,space", help="comma list: mock, space")
    ap.add_argument("--only", default="", help="comma list of case ids or group prefixes")
    ap.add_argument("--out", help="write the result JSON here (default stdout)")
    ap.add_argument("--list", action="store_true", help="print the registered cases and exit")
    a = ap.parse_args(argv)
    load_env(a.env)
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, here)
    import cases as pkg  # noqa: F401

    for m in pkgutil.iter_modules(pkg.__path__):
        importlib.import_module(f"cases.{m.name}")
    only = [s for s in a.only.split(",") if s]
    selected = [
        cid
        for cid in sorted(CASES)
        if not only or any(cid == o or cid.startswith(o + ".") for o in only)
    ]
    if a.list:
        print(
            json.dumps(
                [
                    {"id": c.id, "needs": c.needs, "ref": c.ref, "modes": list(c.modes)}
                    for c in map(CASES.get, sorted(CASES))
                ],
                indent=1,
            )
        )
        return 0
    from importlib.metadata import version

    t0 = time.time()
    res = asyncio.run(
        run_cases(
            a.mode,
            selected,
            set(a.needs.split(",")),
            lambda m: print(m, file=sys.stderr, flush=True),
        )
    )
    doc = {
        "runner": f"python-{a.mode}",
        "sdk": version("daytona"),
        "seconds": round(time.time() - t0, 1),
        "cases": res,
    }
    s = json.dumps(doc, indent=1, sort_keys=True)
    if a.out:
        open(a.out, "w").write(s)
    else:
        print(s)
    failed = [k for k, v in res.items() if not v["ok"]]
    print(f"{len(res) - len(failed)} passed, {len(failed)} failed {failed}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    import harness as _h

    _rc = _h.main()
    # An SDK background thread (a PTY or log websocket) must not keep a
    # finished run alive.
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(_rc)
