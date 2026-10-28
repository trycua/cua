"""Conformance cases: watch_dir (async). The events are produced by shell
commands, so every implementation sees real file operations, and recorded as
sets of `<type>:<relative name>` per operation (E2B reports a write as one
or several events; the set keeps the contract, not the count)."""

from __future__ import annotations

import asyncio

from .common import case, exc_info, has_param, probe, until

OPS = [
    ("mkdir", "mkdir w/sub"),
    ("create", "touch w/f.txt"),
    ("append", "echo x >> w/f.txt"),
    ("chmod", "chmod 600 w/f.txt"),
    ("rename", "mv w/f.txt w/g.txt"),
    ("nested_create", "touch w/sub/inner.txt"),
    ("nested_dir", "mkdir -p w/deep/a/b"),
    ("deep_create", "touch w/deep/a/b/x.txt"),
    ("remove", "rm w/g.txt"),
    ("nested_remove", "rm w/sub/inner.txt"),
    ("rmdir", "rmdir w/sub"),
]


async def _run_ops(sb, root: str, recursive: bool, **kw) -> dict:
    await sb.commands.run(f"rm -rf {root}; mkdir -p {root}/w")
    seen = []
    h = await sb.files.watch_dir(f"{root}/w", on_event=seen.append, recursive=recursive, **kw)
    await asyncio.sleep(1.0)
    out = {}
    try:
        for label, cmd in OPS:
            seen.clear()
            await sb.commands.run(f"cd {root} && {cmd}")
            await asyncio.sleep(1.3)
            out[label] = sorted({f"{e.type.value}:{e.name}" for e in list(seen)})
    finally:
        await h.stop()
    return out


@case
async def watch_ops_flat(sb):
    return await _run_ops(sb, "/tmp/conf-watch-flat", False)


@case
async def watch_ops_recursive(sb):
    return await _run_ops(sb, "/tmp/conf-watch-rec", True)


@case
async def watch_burst(sb):
    root = "/tmp/conf-watch-burst"
    await sb.commands.run(f"rm -rf {root}; mkdir -p {root}")
    seen = []
    h = await sb.files.watch_dir(root, on_event=seen.append)
    await asyncio.sleep(1.0)
    await sb.commands.run(f"cd {root}; i=0; while [ $i -lt 200 ]; do : > b$i.txt; i=$((i+1)); done")

    def creates():
        return {e.name for e in list(seen) if e.type.value == "create"}

    await until(lambda: len(creates()) >= 200, timeout=20)
    await h.stop()
    return {
        "creates_seen": len(creates()),
        "all_names": creates() == {f"b{i}.txt" for i in range(200)},
        "event_class": type(seen[0]).__name__ if seen else None,
        "fields": sorted(vars(seen[0])) if seen else None,
    }


@case
async def watch_lifecycle(sb):
    root = "/tmp/conf-watch-life"
    await sb.commands.run(f"rm -rf {root}; mkdir -p {root}")
    res = {}
    # stop twice, events after stop
    seen = []
    h = await sb.files.watch_dir(root, on_event=seen.append)
    await asyncio.sleep(0.8)
    res["handle_type"] = type(h).__name__
    res["stop"] = await probe(h.stop())
    res["stop_twice"] = await probe(h.stop())
    await sb.commands.run(f"touch {root}/after-stop.txt")
    await asyncio.sleep(1.5)
    res["event_after_stop"] = any("after-stop" in e.name for e in seen)
    # timeout expiry: on_exit. The watch is opened in a task of its own:
    # E2B's SDK cancels the task that opened the stream when `timeout` runs
    # out (recorded as `opener_task`).
    exits = []
    holder = {}

    async def opener():
        holder["h"] = await sb.files.watch_dir(
            root, on_event=lambda e: None, on_exit=exits.append, timeout=2
        )
        await asyncio.sleep(8)

    t = asyncio.ensure_future(opener())
    await asyncio.wait([t], timeout=15)
    res["opener_task"] = (
        ("cancelled" if t.cancelled() else ("failed" if t.exception() else "finished"))
        if t.done()
        else "running"
    )
    t.cancel()
    await asyncio.sleep(0.5)
    res["on_exit_calls"] = [None if x is None else exc_info(x, True) for x in exits]
    if "h" in holder:
        res["stop_after_exit"] = await probe(holder["h"].stop())
    # on_exit when stopped by hand
    exits3 = []
    h3 = await sb.files.watch_dir(root, on_event=lambda e: None, on_exit=exits3.append)
    await asyncio.sleep(0.8)
    await h3.stop()
    await asyncio.sleep(0.5)
    res["on_exit_after_stop"] = [None if x is None else type(x).__name__ for x in exits3]
    # async callbacks
    got = []

    async def on_event(e):
        got.append(e.name)

    h4 = await sb.files.watch_dir(root, on_event=on_event)
    await asyncio.sleep(0.8)
    await sb.commands.run(f"touch {root}/async-cb.txt")
    await until(lambda: "async-cb.txt" in got, timeout=8)
    await h4.stop()
    res["async_callback"] = "async-cb.txt" in got
    # a callback that raises must not end the watch
    n = []

    def boom(e):
        n.append(e.name)
        raise RuntimeError("callback failure")

    h5 = await sb.files.watch_dir(root, on_event=boom)
    await asyncio.sleep(0.8)
    await sb.commands.run(f"touch {root}/boom1.txt; sleep 0.5; touch {root}/boom2.txt")
    await asyncio.sleep(2)
    res["after_raising_callback"] = sorted(set(n))
    res["stop_after_raising"] = await probe(h5.stop())
    return res


@case
async def watch_errors(sb):
    await sb.commands.run("mkdir -p /tmp/conf-watch-err; echo x > /tmp/conf-watch-err/file.txt")
    cb = lambda e: None  # noqa: E731
    return {
        "missing": await _watch_probe(sb, "/tmp/conf-watch-err/nope", cb),
        "file": await _watch_probe(sb, "/tmp/conf-watch-err/file.txt", cb),
        "empty_path": await _watch_probe(sb, "", cb),
        "relative_ok": await _watch_probe(sb, ".", cb),
    }


async def _watch_probe(sb, path, cb):
    async def go():
        h = await sb.files.watch_dir(path, on_event=cb)
        await asyncio.sleep(0.5)
        await h.stop()
        return "watching"

    return await probe(go())


@case
async def watch_include_entry(sb):
    if not has_param(sb.files.watch_dir, "include_entry"):
        return {"since": "2.56"}
    root = "/tmp/conf-watch-entry"
    await sb.commands.run(f"rm -rf {root}; mkdir -p {root}")
    seen = []
    h = await sb.files.watch_dir(root, on_event=seen.append, include_entry=True)
    plain = []
    h2 = await sb.files.watch_dir(root, on_event=plain.append)
    await asyncio.sleep(1.0)
    await sb.commands.run(f"echo hello > {root}/e.txt; sleep 0.7; rm {root}/e.txt")
    await until(lambda: any(e.type.value == "remove" for e in seen), timeout=8)
    await h.stop()
    await h2.stop()
    created = [e for e in seen if e.type.value in ("create", "write") and e.entry is not None]
    removed = [e for e in seen if e.type.value == "remove"]
    e = created[0].entry if created else None
    return {
        "entry_on_create": (
            None
            if e is None
            else {
                "class": type(e).__name__,
                "name": e.name,
                "path": e.path.replace(root, ""),
                "type": str(e.type),
                "size_is_int": isinstance(e.size, int),
            }
        ),
        "entry_on_remove": [x.entry is None for x in removed],
        "entry_without_flag": sorted({x.entry is None for x in plain}),
    }


@case
async def watch_watched_dir_removed(sb):
    root = "/tmp/conf-watch-gone"
    await sb.commands.run(f"rm -rf {root}; mkdir -p {root}/w")
    seen, exits = [], []
    h = await sb.files.watch_dir(f"{root}/w", on_event=seen.append, on_exit=exits.append)
    await asyncio.sleep(1.0)
    await sb.commands.run(f"rm -rf {root}/w")
    await asyncio.sleep(2.5)
    res = {
        "events": sorted({f"{e.type.value}:{e.name}" for e in seen}),
        "on_exit_called": bool(exits),
        "stop": await probe(h.stop()),
    }
    return res
