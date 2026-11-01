"""Shared plumbing of the conformance suite: the case registries, the error
and text normalisers every case uses, and a few polling helpers.

Kept apart from `suite.py` so the case modules can import it without a cycle
(`python -m conformance.suite` runs suite.py as `__main__`).
"""

from __future__ import annotations

import asyncio
import hashlib
import inspect
import os
import re
import time
from typing import Any, Awaitable, Callable, Dict, Optional

import e2b
from e2b import AsyncSandbox

SCHEME = os.environ.get("CONFORMANCE_SCHEME", "https")
NO_ROOT = os.environ.get("CONFORMANCE_NO_ROOT") == "1"
ROOT = {} if NO_ROOT else {"user": "root"}

Case = Callable[[AsyncSandbox], Awaitable[Any]]
CASES: Dict[str, Case] = {}
FRESH: Dict[str, Callable[[Any], Awaitable[Any]]] = {}
SYNC_CASES: Dict[str, Callable[[Any], Any]] = {}

# Set by `run_suite`: what a case needs besides the sandbox it is handed.
SHARED: Dict[str, Any] = {"run_id": "r0"}


def case(fn: Case) -> Case:
    CASES[fn.__name__] = fn
    return fn


def fresh(fn):
    """A case that creates (and kills) its own sandbox."""
    FRESH[fn.__name__] = fn
    return fn


def sync_case(fn):
    """A case run through the sync `e2b.Sandbox` (recorded as `sync.<name>`)."""
    SYNC_CASES[fn.__name__] = fn
    return fn


_UUID = re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
_ID = re.compile(r"\b(?=[a-z0-9]*\d)(?=[a-z0-9]*[a-z])[a-z0-9]{16,32}\b")
_WATCHER = re.compile(r"(watcher with id )\S+( not found)")
_CUA_ID = re.compile(r"\be2b-[0-9a-f]{10}\b")  # the id shape of cua-e2b sandboxes
_PID = re.compile(r"\b(pid|PID|process)([ :=]+)\d+")


def mask(text: Any) -> Any:
    """Replaces ids and pids in a message: they differ in every run."""
    if not isinstance(text, str):
        return text
    # the request schema name is the server's wire detail, not behaviour
    text = text.replace("schemas/NewSandboxV2", "schemas/NewSandbox")
    text = _UUID.sub("<id>", text)
    text = _ID.sub("<id>", text)
    # a watcher id is whatever the server mints (E2B: lower-case, cua-spacesd: `w-` + mixed case)
    text = _WATCHER.sub(r"\1<id>\2", text)
    text = _CUA_ID.sub("<id>", text)
    return _PID.sub(r"\1\2<n>", text)


def exc_info(e: BaseException, msg: bool = False) -> dict:
    d: Dict[str, Any] = {
        "type": type(e).__name__,
        "mro": [c.__name__ for c in type(e).__mro__ if c.__module__.split(".")[0] == "e2b"],
    }
    for k in ("exit_code", "stdout", "stderr", "error"):
        if hasattr(e, k):
            d[k] = getattr(e, k)
    if d["type"] == "CommandExitException":
        d["str"] = str(e)
    elif msg:
        d["text"] = mask(str(e))
    return d


def result_info(r) -> dict:
    return {
        "type": type(r).__name__,
        "stdout": r.stdout,
        "stderr": r.stderr,
        "exit_code": r.exit_code,
        "error": r.error,
    }


def plain(v: Any) -> Any:
    """A JSON-able, run-independent view of a returned value (times and ids
    of dataclasses left out)."""
    import dataclasses
    import datetime
    import enum

    if v is None or isinstance(v, (str, int, float, bool)):
        return v
    if isinstance(v, (bytes, bytearray)):
        return {"bytes": bytes(v).hex()}
    if isinstance(v, (list, tuple)):
        return [plain(x) for x in v]
    if isinstance(v, dict):
        return {str(k): plain(x) for k, x in v.items()}
    if isinstance(v, enum.Enum):
        return str(v)
    if dataclasses.is_dataclass(v):
        name = type(v).__name__
        if name in ("EntryInfo", "WriteInfo"):
            return [name, v.name, str(v.type), v.path]
        return {
            name: {
                f.name: plain(getattr(v, f.name))
                for f in dataclasses.fields(v)
                if not isinstance(getattr(v, f.name), datetime.datetime)
            }
        }
    return type(v).__name__


def host_shape(host: str, sandbox_id: str) -> dict:
    """The structure of a `get_host` value with the sandbox id (and a numeric
    port) masked: `{prefix, suffix}`."""
    h = host.replace(sandbox_id, "<id>")
    head, sep, tail = h.partition("-<id>")
    if not sep:
        return {"form": re.sub(r"\d+", "<n>", h)}
    return {"prefix": "<port>" if re.fullmatch(r"-?\d+", head) else head, "suffix": tail}


def envd_token(sb) -> Any:
    return getattr(sb, "_envd_access_token", getattr(sb, "envd_access_token", None))


async def probe(aw, fmt=None, msg: bool = True, timeout: float = 90) -> dict:
    """Awaits `aw`; `{"ok": value}` (through `fmt`) or `{"raised": exc_info}`.
    One hung probe must not take the case down: it is recorded as `hung`."""
    try:
        v = await asyncio.wait_for(aw, timeout)
        return {"ok": fmt(v) if fmt else plain(v)}
    except asyncio.TimeoutError:
        return {"hung": True}
    except asyncio.CancelledError as e:
        # the SDK raises CancelledError from some failed streams; a real
        # cancellation of this task still propagates
        task = asyncio.current_task()
        if task is not None and task.cancelling():
            raise
        return {"raised": exc_info(e, msg)}
    except Exception as e:  # noqa: BLE001
        return {"raised": exc_info(e, msg)}


def sprobe(fn, fmt=None, msg: bool = True) -> dict:
    """`probe` for the sync API."""
    try:
        v = fn()
        return {"ok": fmt(v) if fmt else plain(v)}
    except Exception as e:  # noqa: BLE001
        return {"raised": exc_info(e, msg)}


def sha(b) -> str:
    return hashlib.sha256(bytes(b)).hexdigest()


async def until(pred, timeout: float = 10.0, step: float = 0.2) -> bool:
    """Polls `pred()` (sync) until true; False on timeout."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        await asyncio.sleep(step)
    return bool(pred())


def has_param(fn, param: str) -> bool:
    try:
        return param in inspect.signature(getattr(fn, "method", fn)).parameters
    except (TypeError, ValueError):
        return False


def newer(fn_name: str, param: str) -> bool:
    return has_param(getattr(AsyncSandbox, fn_name, None), param)


async def try_create(template=None, kill: bool = True, **kwargs) -> dict:
    """create(**kwargs): the refusal (class and text), or `created` (and the
    sandbox killed again)."""
    try:
        sb = await AsyncSandbox.create(
            template=template, timeout=kwargs.pop("timeout", 60), **kwargs
        )
    except Exception as e:  # noqa: BLE001
        return {"raised": exc_info(e, True)}
    if kill:
        try:
            await sb.kill()
        except Exception:  # noqa: BLE001
            pass
    return {"created": True}


def ts_aware(dt) -> Optional[bool]:
    return None if dt is None else dt.tzinfo is not None


def fmt_entry(e) -> dict:
    return {
        "name": e.name,
        "type": str(e.type),
        "size_is_int": isinstance(e.size, int),
        "symlink_target": getattr(e, "symlink_target", None),
    }
