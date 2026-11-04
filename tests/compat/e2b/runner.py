"""Shared-case runner for the Python `e2b` SDK.

Runs `cases.json` (the same file the TypeScript runner, runner.mjs, runs)
against whatever the environment points at (real E2B, the Cua gateway, the
cua-e2b shim) and prints one JSON object of observations. `compare.py`
diffs two runs. See cases.md for the case language.

    python runner.py [--only a,b] [--skip a,b] [--tag t] [--cases cases.json] > run.json

Every case is independent. One failing step is recorded and the case goes
on. Every sandbox a case or the run creates is killed, even on failure.
"""

from __future__ import annotations

import argparse
import asyncio
import dataclasses
import enum
import hashlib
import inspect
import io
import json
import os
import re
import ssl
import sys
import time
import urllib.request
from pathlib import Path

import e2b
from e2b import AsyncSandbox

HERE = Path(__file__).resolve().parent
ap = argparse.ArgumentParser()
ap.add_argument("--cases", default=str(HERE / "cases"), help="a cases/ directory or one JSON file")
ap.add_argument("--only", default="")
ap.add_argument("--skip", default="")
ap.add_argument("--tag", default="", help="run only cases carrying this tag")
ap.add_argument("--template", default=os.environ.get("CONFORMANCE_TEMPLATE"))
args = ap.parse_args()
ONLY = {s for s in args.only.split(",") if s}
SKIP = {s for s in args.skip.split(",") if s}

LANG = "py"
RUN_ID = "r" + hashlib.sha256(os.urandom(16)).hexdigest()[:12]
MASK_KEYS = set(json.loads((HERE / "common.json").read_text())["mask_keys"])
CANON = {}  # python names are the canonical ones


def snake(s: str) -> str:
    return re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", s).lower()


def sdk_version() -> str:
    from importlib.metadata import version

    return version("e2b")


# ── values ───────────────────────────────────────────────────────────────


def gen_bytes(spec: dict) -> bytes:
    n = int(spec["n"])
    seed = int(spec.get("seed", 1))
    pattern = spec.get("pattern", "mix")
    if pattern == "zero":
        return bytes(n)
    if pattern == "text":
        unit = spec.get("unit", "line of text\n").encode()
        return (unit * (n // len(unit) + 1))[:n]
    # deterministic, incompressible-ish, all 256 values: byte i = (i*167 + seed + (i>>8)*13) & 255
    return bytes(((i * 167 + seed + (i >> 8) * 13) & 255) for i in range(n))


class Ctx:
    def __init__(self):
        self.vars: dict = {"run_id": RUN_ID}
        self.collected: dict = {}
        self.cb_specs: dict = {}
        self.sync = False
        self.tasks: list = []

    def lookup(self, path: str):
        head, *rest = path.split(".")
        v = self.vars[head]
        for part in rest:
            if isinstance(v, dict):
                v = v[part]
            elif isinstance(v, (list, tuple)):
                v = v[int(part)]
            else:
                v = getattr(v, part)
        return v

    def resolve(self, v):
        if isinstance(v, str):
            m = re.fullmatch(r"\$([A-Za-z_][\w.]*)", v)
            if m:
                return self.lookup(m.group(1))
            if "${" in v:
                return re.sub(r"\$\{([\w.]+)\}", lambda m: str(self.lookup(m.group(1))), v)
            return v
        if isinstance(v, list):
            return [self.resolve(x) for x in v]
        if isinstance(v, dict):
            if "$bytes" in v:
                return gen_bytes(v["$bytes"])
            if "$utf8" in v:
                return v["$utf8"].encode()
            if "$stream" in v:
                return io.BytesIO(gen_bytes(v["$stream"]))
            if "$text" in v:
                t = v["$text"]
                return t["unit"] * int(t.get("repeat", 1))
            if "$cb" in v:
                return self.callback(v)
            if "$repeat" in v:
                r = v["$repeat"]
                return [
                    self.resolve(json.loads(json.dumps(r["item"]).replace("{i}", str(i))))
                    for i in range(r["n"])
                ]
            if "$template" in v:
                return build_template(v["$template"])
            if "$query" in v:
                q = dict(v["$query"])
                if "state" in q:
                    q["state"] = [e2b.SandboxState(x) for x in q["state"]]
                return e2b.SandboxQuery(**self.resolve(q))
            if "$size" in v:
                return e2b.PtySize(rows=v["$size"]["rows"], cols=v["$size"]["cols"])
            return {k: self.resolve(x) for k, x in v.items()}
        return v

    def callback(self, spec: dict):
        name = spec["$cb"]
        self.cb_specs[name] = spec
        as_text = spec.get("as") == "text"
        bucket = self.collected.setdefault(name, [])

        prefix = spec.get("prefix", "")

        def cb(x):
            if spec.get("as") == "event":
                x = f"{x.name}:{x.type.value}"
            elif spec.get("as") == "message":
                x = getattr(x, "message", str(x))
            if as_text and isinstance(x, (bytes, bytearray)):
                x = bytes(x).decode("utf-8", "replace")
            x = norm(x)
            bucket.append(prefix + x if prefix and isinstance(x, str) else x)

        if spec.get("async"):

            async def acb(x):
                cb(x)

            return acb
        return cb


# ── observations ─────────────────────────────────────────────────────────


def canon(name: str) -> str:
    return name


def exc_info(e: BaseException) -> dict:
    d = {
        "type": canon(type(e).__name__),
        "mro": [canon(c.__name__) for c in type(e).__mro__ if c.__module__.startswith("e2b")],
    }
    for k in ("exit_code", "stdout", "stderr", "error"):
        if hasattr(e, k):
            d[k] = norm(getattr(e, k))
    d["message"] = redact(str(e))
    return d


def redact(s: str) -> str:
    s = s.replace(RUN_ID, "<run>")
    s = re.sub(r"\bpid \d+", "pid <pid>", s)
    s = re.sub(r"\b[a-z0-9]{20,21}\b", "<id>", s)
    s = re.sub(r"[0-9a-z]{18,}", "<id>", s)
    s = re.sub(r"\b[a-z0-9]+(?:-[a-z0-9]+)*-\d+/", "<ns>/", s)
    s = re.sub(r"127\.0\.0\.1:\d+", "<host>", s)
    return s


def norm(v, key=None):
    if key is not None and snake(key) in MASK_KEYS:
        return "<masked>" if v is not None else None
    if isinstance(v, str):
        if len(v) > 4096:
            return {
                "$str": len(v),
                "sha256": hashlib.sha256(v.encode("utf-8", "replace")).hexdigest()[:16],
            }
        return redact(v) if RUN_ID in v else v
    if v is None or isinstance(v, (bool, int)):
        return v
    if (
        hasattr(v, "isoformat")
        and callable(v.isoformat)
        and key is not None
        and snake(key) == "end_at"
    ):
        import datetime

        now = datetime.datetime.now(v.tzinfo)
        return f"end_in:{round((v - now).total_seconds() / 30) * 30}"
    if isinstance(v, float):
        return round(v, 3)
    if isinstance(v, (bytes, bytearray)):
        return {"$bytes": len(v), "sha256": hashlib.sha256(bytes(v)).hexdigest()[:16]}
    if isinstance(v, enum.Enum):
        return v.value if isinstance(v.value, (str, int)) else v.name
    if isinstance(v, BaseException):
        return {"raised": exc_info(v)}
    if hasattr(v, "isoformat") and callable(v.isoformat):
        return "<datetime>"
    if isinstance(v, (list, tuple, set)):
        return [norm(x) for x in v]
    if isinstance(v, dict):
        if key is not None and snake(key) in ("envs", "metadata", "headers"):
            return {str(k): norm(x) for k, x in v.items()}  # user data: keys are not ours to rename
        return {snake(str(k)): norm(x, str(k)) for k, x in v.items()}
    if dataclasses.is_dataclass(v) and not isinstance(v, type):
        return {snake(f.name): norm(getattr(v, f.name), f.name) for f in dataclasses.fields(v)}
    if isinstance(v, (AsyncSandbox, e2b.Sandbox)) or type(v).__name__ in (
        "Sandbox",
        "AsyncSandbox",
    ):
        return {"$sandbox": True}
    if hasattr(v, "wait") and hasattr(v, "pid"):
        return {"$handle": True, "pid": "<pid>"}
    if inspect.isawaitable(v):
        return "<awaitable>"
    attrs = {}
    for k in dir(v):
        if k.startswith("_"):
            continue
        try:
            x = getattr(v, k)
        except Exception:  # noqa: BLE001
            continue
        if callable(x):
            continue
        attrs[snake(k)] = norm(x, k)
    return attrs or {"$object": type(v).__name__}


def pick(v, fields):
    if not fields:
        return v
    if isinstance(v, list):
        return [pick(x, fields) for x in v]
    if isinstance(v, dict):
        out = {}
        for f in fields:
            cur = v
            for p in f.split("."):
                cur = cur.get(p) if isinstance(cur, dict) else None
            out[f] = cur
        return out
    return v


# ── ops ──────────────────────────────────────────────────────────────────

STATICS = {
    "Sandbox": AsyncSandbox,
    "Volume": getattr(e2b, "AsyncVolume", None),
    "Secret": getattr(e2b, "AsyncSecret", None),
    "Template": getattr(e2b, "AsyncTemplate", None),
}


def multipart(spec: dict):
    boundary = "----cuaconformance0123456789"
    body = (
        b"".join(
            b"--"
            + boundary.encode()
            + b'\r\nContent-Disposition: form-data; name="file"; filename="'
            + f["filename"].encode()
            + b'"\r\nContent-Type: application/octet-stream\r\n\r\n'
            + (f["content"].encode() if isinstance(f["content"], str) else f["content"])
            + b"\r\n"
            for f in spec["files"]
        )
        + b"--"
        + boundary.encode()
        + b"--\r\n"
    )
    return body, f"multipart/form-data; boundary={boundary}"


def parsed_body(text: str):
    """JSON bodies compare as values (key order and trailing newlines differ between servers)."""
    text = redact(text[:2000])
    try:
        v = json.loads(text)
        return v if isinstance(v, (dict, list)) else text
    except ValueError:
        return text


def http_get(url, headers=None, method="GET", data=None, mp=None):
    ctx = ssl.create_default_context(cafile=os.environ.get("SSL_CERT_FILE"))
    headers = dict(headers or {})
    if mp:
        data, headers["Content-Type"] = multipart(mp)
    elif isinstance(data, str):
        data = data.encode()
    req = urllib.request.Request(url, headers=headers or {}, method=method, data=data)
    try:
        with urllib.request.urlopen(req, timeout=20, context=ctx) as r:
            return {"status": r.status, "body": parsed_body(r.read().decode("utf-8", "replace"))}
    except urllib.error.HTTPError as e:  # noqa: PERF203
        return {"status": e.code, "body": parsed_body(e.read().decode("utf-8", "replace"))}
    except Exception as e:  # noqa: BLE001
        return {"error": type(e).__name__}


def build_template(spec: dict):
    """`{"$template": {"context": {"file": "text"}, "steps": [[method, args, kw]]}}` as a builder chain."""
    import tempfile

    ctxdir = tempfile.mkdtemp(prefix="cua-cases-ctx-")
    for rel, text in (spec.get("context") or {}).items():
        path = Path(ctxdir) / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    t = e2b.AsyncTemplate(file_context_path=ctxdir)
    for step in spec["steps"]:
        t = getattr(t, step[0])(
            *(step[1] if len(step) > 1 else []), **(step[2] if len(step) > 2 else {})
        )
    return t


def api_call(method: str, path: str, body=None):
    import httpx

    base = os.environ.get("E2B_API_URL") or "https://api.e2b.app"
    r = httpx.request(
        method, base + path, headers={"X-API-KEY": os.environ["E2B_API_KEY"]}, json=body, timeout=60
    )
    try:
        text = parsed_body(r.text)
    except Exception:  # noqa: BLE001
        text = r.text[:500]
    return {"status": r.status_code, "body": text}


async def maybe_await(x):
    return await x if inspect.isawaitable(x) else x


def call_path(obj, path: str):
    for part in path.split("."):
        obj = getattr(obj, part)
    return obj


async def do_op(ctx: Ctx, step: dict, created: list):
    op = step["op"]
    a = ctx.resolve(step.get("args", []))
    kw = ctx.resolve({**step.get("kw", {}), **step.get("kw_py", {})})
    if "args_py" in step:
        a = ctx.resolve(step["args_py"])
    if op == "sleep":
        await asyncio.sleep(a[0])
        return None
    if op == "http":
        if kw.get("tamper"):
            a = [
                re.sub(
                    r"(signature=[^&]*)(.)(&|$)",
                    lambda m: m.group(1) + ("A" if m.group(2) != "A" else "B") + m.group(3),
                    a[0],
                )
            ]
        return await asyncio.to_thread(
            http_get,
            a[0],
            kw.get("headers"),
            kw.get("method", "GET"),
            kw.get("data"),
            kw.get("multipart"),
        )
    if op == "env":
        return os.environ.get(a[0])
    if op == "api":
        return await asyncio.to_thread(api_call, a[0], a[1], a[2] if len(a) > 2 else None)
    if op == "attr":  # read a property of a stored value
        return call_path(ctx.lookup(step["on"]), a[0])
    if op == "parallel":
        n = int(step.get("n", 1))
        results = await asyncio.gather(
            *[do_op(ctx, step["step"], created) for _ in range(n)],
            return_exceptions=True,
        )
        return list(results)
    if ctx.sync:
        return await do_sync_op(ctx, step, op, a, kw, created)
    cls, _, method = op.partition(".")
    if cls in STATICS and method:
        if STATICS[cls] is None:
            raise e2b.SandboxException(f"{cls} is not in this SDK version")
        if cls == "Sandbox" and method == "create":
            tpl = kw.pop("template", None) or (args.template if not kw.get("no_template") else None)
            kw.pop("no_template", None)
            r = await AsyncSandbox.create(template=tpl, **kw)
            created.append(r)
            return r
        target = call_path(STATICS[cls], method)
        r = await maybe_await(target(*a, **kw))
        if cls == "Sandbox" and method in ("connect", "fork"):
            if isinstance(r, list):
                created.extend(x for x in r if not isinstance(x, BaseException))
            else:
                created.append(r)
        return r
    on = ctx.lookup(step.get("on", "sb"))
    if op.startswith("@"):
        attr = call_path(on, op[1:])
        return await maybe_await(attr(*a, **kw)) if callable(attr) else attr
    if op == "files.watch_dir":
        return await on.files.watch_dir(*a, **kw)
    target = call_path(on, op)
    if "step_iter" in step:
        pass
    r = target(*a, **kw)
    return await maybe_await(r)


def until_ok(spec: dict, r) -> bool:
    """`until`: {"path": "state" | "#" (the length), "equals": value}."""
    v = norm(r)
    for part in spec["path"].split(".") if spec["path"] not in ("#", "") else []:
        v = v.get(part) if isinstance(v, dict) else None
    if spec["path"] == "#":
        v = len(v) if isinstance(v, list) else None
    return v == spec["equals"]


async def poll_until(ctx, step, created, r):
    spec = step["until"]
    deadline = time.time() + spec.get("timeout", 60)
    if step.get("paginate"):
        r = await paginate(r)
    while not until_ok(spec, r) and time.time() < deadline:
        await asyncio.sleep(spec.get("interval", 2))
        r = await do_op(ctx, step, created)
        if step.get("paginate"):
            r = await paginate(r)
    return r


async def do_sync_op(ctx, step, op, a, kw, created):
    """The same step with the blocking SDK, off the event loop."""
    cls, _, method = op.partition(".")
    if cls == "Sandbox" and method:
        if method == "create":
            tpl = kw.pop("template", None) or (args.template if not kw.get("no_template") else None)
            kw.pop("no_template", None)
            r = await asyncio.to_thread(lambda: e2b.Sandbox.create(template=tpl, **kw))
            created.append(r)
            return r
        r = await asyncio.to_thread(lambda: call_path(e2b.Sandbox, method)(*a, **kw))
        return r
    on = ctx.lookup(step.get("on", "sb"))
    if op.startswith("@"):
        attr = call_path(on, op[1:])
        return await asyncio.to_thread(lambda: attr(*a, **kw)) if callable(attr) else attr
    target = call_path(on, op)
    return await asyncio.to_thread(lambda: target(*a, **kw))


async def paginate(p, max_pages=50):
    items = []
    for _ in range(max_pages):
        items += await p.next_items()
        if not p.has_next:
            break
    return items


SHARED_TIMEOUT = 900


async def run_case(case: dict, shared, created: list) -> dict:
    ctx = Ctx()
    own = []
    mode = case.get("sandbox", "shared")
    out: list = []  # (key, record) pairs
    try:
        if case.get("sync"):
            ctx.sync = True
            sb = await asyncio.to_thread(
                lambda: e2b.Sandbox.create(
                    template=args.template, **ctx.resolve(case.get("create", {"timeout": 300}))
                )
            )
            created.append(sb)
            own.append(sb)
            ctx.vars["sb"] = sb
        elif mode == "shared":
            # A slow run (a Space is slower than E2B) must not outlive the
            # shared sandbox's timeout: every shared case starts with a full one.
            try:
                await shared.set_timeout(SHARED_TIMEOUT)
            except BaseException:  # noqa: BLE001
                pass
            ctx.vars["sb"] = shared
        elif mode == "own":
            try:
                sb = await AsyncSandbox.create(
                    template=args.template, **ctx.resolve(case.get("create", {"timeout": 300}))
                )
            except BaseException as e:  # noqa: BLE001
                return {"steps": {"create": {"op": "create", "raised": exc_info(e)}}}
            created.append(sb)
            own.append(sb)
            ctx.vars["sb"] = sb
        for i, step in enumerate(case["steps"]):
            if isinstance(step, list):
                step = {
                    "op": step[0],
                    **({"args": step[1]} if len(step) > 1 else {}),
                    **({"kw": step[2]} if len(step) > 2 else {}),
                    **(step[3] if len(step) > 3 else {}),
                }
            if "py" in step.get("skip", []):
                continue
            key = step.get("as") or f"s{i}"
            rec: dict = {"op": step["op"]}
            try:
                r = await do_op(ctx, step, created)
                if step.get("until"):
                    r = await poll_until(ctx, step, created, r)
                if step.get("consume"):
                    buf = b""
                    if hasattr(r, "__aenter__"):
                        async with r as stream:
                            async for chunk in stream:
                                buf += chunk
                    else:
                        async for chunk in r:
                            buf += chunk
                    r = buf
                if step.get("as"):
                    ctx.vars[step["as"]] = r
                if step.get("paginate") and not step.get("until"):
                    r = await paginate(r)
                if not step.get("quiet"):
                    full = norm(r)
                    if step.get("grep") and isinstance(full, list):
                        full = [
                            x for x in full if step["grep"] in json.dumps(x, ensure_ascii=False)
                        ]
                    rec["ok"] = pick(full, step.get("pick"))
                if step.get("first") and isinstance(rec.get("ok"), list):
                    rec["ok"] = rec["ok"][0] if rec["ok"] else None
                if step.get("sort"):
                    rec["ok"] = sorted(rec["ok"], key=lambda x: json.dumps(x, sort_keys=True))
            except BaseException as e:  # noqa: BLE001
                if step.get("as"):
                    ctx.vars[step["as"]] = e
                rec["raised"] = exc_info(e)
            if step.get("quiet") and "raised" not in rec:
                continue
            out.append((key if key not in dict(out) else f"{key}_{i}", rec))
    finally:
        for sb in own:
            try:
                r = sb.kill()
                if inspect.isawaitable(r):
                    await r
            except BaseException:  # noqa: BLE001
                pass
    res = {"steps": dict(out)}
    for name, spec in ctx.cb_specs.items():
        got = ctx.collected.get(name)
        if got is None:
            continue
        joined = "".join(x for x in got if isinstance(x, str))
        if spec.get("join"):
            ctx.collected[name] = joined
        if spec.get("probe"):
            ctx.collected[name] = {p: p in joined for p in spec["probe"]}
    ctx.collected = {k: v for k, v in ctx.collected.items() if not k.startswith("_")}
    if ctx.collected:
        res["collected"] = {
            k: (
                sorted(set(v), key=str)
                if k.endswith("_set")
                else len(v) if k.endswith("_count") else v
            )
            for k, v in ctx.collected.items()
        }
    return res


def load_cases(path: str) -> dict:
    p = Path(path)
    files = sorted(p.glob("*.json")) if p.is_dir() else [p]
    merged: dict = {"cases": []}
    for f in files:
        d = json.loads(f.read_text())
        merged["cases"] += d["cases"]
        for k, v in d.items():
            if k != "cases":
                merged[k] = v
    names = [c["name"] for c in merged["cases"]]
    assert len(names) == len(set(names)), "duplicate case names"
    return merged


async def main():
    cases = load_cases(args.cases)
    result: dict = {"sdk": {"lang": LANG, "version": sdk_version()}, "cases": {}}
    created: list = []
    cfg = cases.get("shared_create", {"timeout": 900, "metadata": {"suite": "cases"}})
    global SHARED_TIMEOUT
    SHARED_TIMEOUT = cfg.get("timeout", 900)
    shared = None
    try:
        shared = await AsyncSandbox.create(template=args.template, **cfg)
        created.append(shared)
        # Every case works under /tmp/c, writable by any user, whichever case runs first.
        await shared.commands.run("mkdir -p /tmp/c && chmod 777 /tmp/c", user="root")
        for case in cases["cases"]:
            name = case["name"]
            if (ONLY and name not in ONLY) or name in SKIP:
                continue
            if args.tag and args.tag not in case.get("tags", []):
                continue
            if LANG in case.get("skip", []):
                continue
            if case.get("sdk_min") and tuple(map(int, sdk_version().split(".")[:3])) < tuple(
                map(int, case["sdk_min"].split("."))
            ):
                continue
            t0 = time.time()
            res = await run_case(case, shared, created)
            result["cases"][name] = res
            print(f"{name} {time.time() - t0:.1f}s", file=sys.stderr)
    finally:
        for sb in created:
            try:
                r = sb.kill()
                if inspect.isawaitable(r):
                    await r
            except BaseException:  # noqa: BLE001
                pass
    text = json.dumps(result, indent=1, sort_keys=True, default=str)
    for secret in (os.environ.get("E2B_API_KEY"), os.environ.get("E2B_ACCESS_TOKEN")):
        if secret and secret in text:
            sys.exit("refusing to print: the output contains the API key")
    text = re.sub(r"e2b_[0-9a-f]{20,}", "e2b_<key>", text)
    print(text)


asyncio.run(main())
