"""Conformance cases: the filesystem API (async)."""

from __future__ import annotations

import asyncio
import io
import tempfile
from datetime import datetime, timezone

from .common import (
    NO_ROOT,
    ROOT,
    case,
    exc_info,
    fmt_entry,
    has_param,
    mask,
    probe,
    sha,
)


def _b(x) -> bytes:
    return bytes(x)


@case
async def fs_names(sb):
    d = "/tmp/conf-names"
    await sb.files.make_dir(d)
    out = {}
    for name in (
        "héllo wörld.txt",
        "sp ace.txt",
        "q'uote.txt",
        'd"quote.txt',
        "日本語.txt",
        "emoji-😀.txt",
        "semi;colon&amp.txt",
        "-dash.txt",
        "a#b.txt",
        "q?x.txt",
        "p%20x.txt",
        "plus+sign.txt",
        "back\\slash.txt",
        "new\nline.txt",
        "tab\there.txt",
    ):
        p = f"{d}/{name}"
        w = await probe(sb.files.write(p, name), lambda i: [i.name, i.path == p])
        rb = await probe(sb.files.read(p), lambda t: t == name)
        entry = {"write": w, "read_ok": rb, "exists": await probe(sb.files.exists(p))}
        out[name] = entry
    listed = await sb.files.list(d)
    out["_listed"] = sorted(e.name for e in listed)
    return out


@case
async def fs_empty_file(sb):
    p = "/tmp/conf-empty.txt"
    w = await sb.files.write(p, "")
    info = await sb.files.get_info(p)
    return {
        "write": [w.name, w.path],
        "text": await sb.files.read(p),
        "bytes": len(await sb.files.read(p, format="bytes")),
        "size": info.size,
        "stream": [len(c) async for c in await sb.files.read(p, format="stream")],
        "rewrite_empty_in_tmp": await probe(sb.files.write(p, b""), lambda i: i.name),
    }


@case
async def fs_binary_roundtrip(sb):
    p = "/tmp/conf-all-bytes.bin"
    data = bytes(range(256))
    await sb.files.write(p, data)
    got = await sb.files.read(p, format="bytes")
    bad = "/tmp/conf-bad-utf8.txt"
    await sb.files.write(bad, b"ok\xff\xfeend")
    return {
        "bytes_type": type(got).__name__,
        "equal": bytes(got) == data,
        "text_of_binary": await probe(sb.files.read(p), lambda t: [len(t), t.count("�")]),
        "invalid_utf8_text": await probe(sb.files.read(bad), lambda t: t),
        "invalid_utf8_bytes": (_b(await sb.files.read(bad, format="bytes"))).hex(),
        "size": (await sb.files.get_info(p)).size,
    }


@case
async def fs_large_roundtrip(sb):
    p = "/tmp/conf-20mb.bin"
    data = bytes(range(256)) * (20 * 4096)
    w = await probe(sb.files.write(p, data, request_timeout=180), lambda i: i.path == p)
    got = await probe(
        sb.files.read(p, format="bytes", request_timeout=180),
        lambda b: [len(b), sha(b) == sha(data)],
        timeout=170,
    )
    return {"write": w, "read": got, "size": (await sb.files.get_info(p)).size}


@case
async def fs_dir_misuse(sb):
    d = "/tmp/conf-dirs"
    await sb.files.make_dir(d)
    return {
        "read_dir_text": await probe(sb.files.read(d)),
        "read_dir_bytes": await probe(sb.files.read(d, format="bytes")),
        "write_onto_dir": await probe(sb.files.write(d, "x")),
        "write_into_file_as_dir": await probe(sb.files.write("/etc/hostname/x", "x")),
        "read_empty_path": await probe(sb.files.read("")),
        "write_empty_path": await probe(sb.files.write("", "x")),
        "list_empty_path": await probe(sb.files.list("")),
    }


@case
async def fs_relative_paths(sb):
    home = (await sb.commands.run("printf %s $HOME")).stdout
    await sb.commands.run("rm -f ~/conf-rel.txt ~/conf-tilde.txt")
    w1 = await probe(
        sb.files.write("conf-rel.txt", "rel"), lambda i: [i.name, i.path == f"{home}/conf-rel.txt"]
    )
    w2 = await probe(
        sb.files.write("~/conf-tilde.txt", "tilde"),
        lambda i: [i.name, i.path == f"{home}/conf-tilde.txt"],
    )
    return {
        "relative_write": w1,
        "relative_in_home": (await sb.commands.run("cat ~/conf-rel.txt 2>&1; true")).stdout,
        "relative_read": await probe(sb.files.read("conf-rel.txt")),
        "tilde_write": w2,
        "tilde_in_home": (await sb.commands.run("cat ~/conf-tilde.txt 2>&1; true")).stdout,
        "tilde_read": await probe(sb.files.read("~/conf-tilde.txt")),
        "dotdot": await probe(sb.files.read("/tmp/../etc/hostname"), lambda t: bool(t.strip())),
        "relative_listed": await probe(
            sb.files.list("."), lambda es: "conf-rel.txt" in [e.name for e in es]
        ),
        "relative_make_dir": await probe(sb.files.make_dir("conf-rel-dir")),
        "relative_exists": await probe(sb.files.exists("conf-rel-dir")),
    }


@case
async def fs_overwrite(sb):
    # E2B overwrites files in the home directory but is refused in /tmp
    # (the sticky directory: kernel protected_regular), recorded as is
    home = (await sb.commands.run("printf %s $HOME")).stdout
    out = {}
    for label, d in (("home", home), ("tmp", "/tmp")):
        p = f"{d}/conf-overwrite.txt"
        first = await probe(sb.files.write(p, "a much longer first content"), lambda i: i.name)
        second = await probe(sb.files.write(p, "short"), lambda i: i.name)
        out[label] = {
            "first": first,
            "second": second,
            "text": await probe(sb.files.read(p)),
            "size": await probe(sb.files.get_info(p), lambda i: i.size),
        }
    p = f"{home}/conf-overwrite-bytes.bin"
    await sb.files.write(p, bytes(range(200)))
    await sb.files.write(p, b"\x01")
    out["bytes_shorter"] = (await sb.files.read(p, format="bytes")).hex()
    out["remove_then_write_in_tmp"] = await _rewrite_after_remove(sb)
    return out


async def _rewrite_after_remove(sb):
    p = "/tmp/conf-overwrite-rm.txt"
    await sb.files.write(p, "one")
    await sb.files.remove(p)
    await sb.files.write(p, "two")
    return await sb.files.read(p)


@case
async def fs_write_files(sb):
    d = "/tmp/conf-wf"
    res = {}
    ents = [
        {"path": f"{d}/a.txt", "data": "alpha"},
        {"path": f"{d}/sub/b.bin", "data": b"\x00\x01"},
        {"path": f"{d}/c.txt", "data": io.BytesIO(b"gamma")},
    ]
    res["several"] = await probe(
        sb.files.write_files(ents), lambda xs: [[i.name, i.path, type(i).__name__] for i in xs]
    )
    res["read_back"] = {
        "a": await probe(sb.files.read(f"{d}/a.txt")),
        "b": await probe(sb.files.read(f"{d}/sub/b.bin", format="bytes"), lambda b: _b(b).hex()),
        "c": await probe(sb.files.read(f"{d}/c.txt")),
    }
    res["empty_list"] = await probe(sb.files.write_files([]), lambda xs: xs)
    res["duplicates"] = await probe(
        sb.files.write_files(
            [{"path": f"{d}/dup.txt", "data": "first"}, {"path": f"{d}/dup.txt", "data": "second"}]
        ),
        lambda xs: len(xs),
    )
    res["duplicate_content"] = await probe(sb.files.read(f"{d}/dup.txt"))
    res["entry_missing_data"] = await probe(sb.files.write_files([{"path": f"{d}/x.txt"}]))
    return res


@case
async def fs_write_types(sb):
    d = "/tmp/conf-wt"
    await sb.files.make_dir(d)
    res = {}

    async def attempt(label, data):
        p = f"{d}/{label}"
        res[label] = await probe(
            sb.files.write(p, data), lambda i: [type(i).__name__, i.name, i.path == p]
        )
        res[label + "_content"] = await probe(
            sb.files.read(p, format="bytes"), lambda b: _b(b).decode("utf-8", "replace")
        )

    await attempt("str", "héllo")
    await attempt("bytes", b"bytes!")
    await attempt("bytearray", bytearray(b"bytearray!"))
    await attempt("bytesio", io.BytesIO(b"bytesio!"))
    await attempt("stringio", io.StringIO("stringio!"))
    with tempfile.NamedTemporaryFile("wb", suffix=".bin") as f:
        f.write(b"file-rb-\x00\xff")
        f.flush()
        with open(f.name, "rb") as fh:
            await attempt("real_binary_file", fh)
        with open(f.name, "r", errors="replace") as fh:
            await attempt("real_text_file", fh)
    await attempt("int", 5)
    await attempt("none", None)
    return res


@case
async def fs_metadata_validation(sb):
    if not has_param(sb.files.write, "metadata"):
        return {"since": "2.56"}
    p = "/tmp/conf-meta-bad.txt"
    res = {}
    for label, md in (
        ("empty_key", {"": "v"}),
        ("int_value", {"k": 1}),
        ("none_value", {"k": None}),
        ("long_key", {"k" * 300: "v"}),
        ("long_value", {"k": "v" * 5000}),
        ("many_keys", {f"k{i}": "v" for i in range(200)}),
        ("not_a_dict", ["a"]),
        ("unicode", {"ключ": "значение ✓"}),
        ("spaces_and_symbols", {"a b": "c=d&e"}),
    ):
        res[label] = await probe(
            sb.files.write(p, "x", metadata=md),
            lambda i: {"metadata": i.metadata},
        )
    return res


@case
async def fs_metadata_roundtrip(sb):
    if not has_param(sb.files.write, "metadata"):
        return {"since": "2.56"}
    d = "/tmp/conf-meta"
    md = {"owner": "me", "kind": "input", "path-like": "a/b c"}
    w = await sb.files.write(f"{d}/m.txt", "m", metadata=md)
    wf = await sb.files.write_files([{"path": f"{d}/n.txt", "data": "n"}], metadata={"batch": "1"})
    info = await sb.files.get_info(f"{d}/m.txt")
    listed = {e.name: e.metadata for e in await sb.files.list(d)}
    ren = await sb.files.rename(f"{d}/m.txt", f"{d}/m2.txt")
    again = await probe(sb.files.write(f"{d}/m2.txt", "again"), lambda i: i.metadata)
    return {
        "overwrite_write": again,
        "write": w.metadata,
        "write_files": [i.metadata for i in wf],
        "info": info.metadata,
        "listed": listed,
        "after_rename": ren.metadata,
        "after_overwrite_without": await probe(
            sb.files.get_info(f"{d}/m2.txt"), lambda i: i.metadata
        ),
        "plain_file": (await sb.files.write(f"{d}/plain.txt", "p")).metadata,
    }


@case
async def fs_mode_owner_default(sb):
    d = "/tmp/conf-mode"
    w = await sb.files.write(f"{d}/f.txt", "x")
    await sb.files.make_dir(f"{d}/dir")
    me = (await sb.commands.run("id -un; id -gn")).stdout.split()
    f = await sb.files.get_info(f"{d}/f.txt")
    dd = await sb.files.get_info(f"{d}/dir")
    return {
        "write_type": type(w).__name__,
        "file": {
            "mode": oct(f.mode),
            "permissions": f.permissions,
            "owner_is_me": f.owner == me[0],
            "group_is_mine": f.group == me[1],
            "owner": f.owner,
            "group": f.group,
        },
        "dir": {
            "mode": oct(dd.mode),
            "permissions": dd.permissions,
            "owner_is_me": dd.owner == me[0],
        },
        "umask": (await sb.commands.run("umask")).stdout.strip(),
        "touch_mode": (await sb.commands.run(f"touch {d}/t && stat -c %a {d}/t")).stdout.strip(),
    }


@case
async def fs_mode_owner_root(sb):
    if NO_ROOT:
        return None
    d = "/tmp/conf-mode-root"
    await sb.files.write(f"{d}/f.txt", "x", user="root")
    f = await sb.files.get_info(f"{d}/f.txt")
    dd = await sb.files.get_info(d)
    return {
        "file": [oct(f.mode), f.permissions, f.owner, f.group],
        "dir": [oct(dd.mode), dd.owner],
    }


@case
async def fs_permission_denied(sb):
    d = "/tmp/conf-perm"
    await sb.files.write(f"{d}/locked.txt", "secret")
    await sb.commands.run(f"chmod 000 {d}/locked.txt")
    await sb.files.make_dir(f"{d}/locked-dir")
    await sb.files.write(f"{d}/locked-dir/in.txt", "x")
    await sb.commands.run(f"chmod 000 {d}/locked-dir")
    res = {
        "read_unreadable": await probe(sb.files.read(f"{d}/locked.txt")),
        "write_unwritable": await probe(sb.files.write(f"{d}/locked.txt", "new")),
        "list_unreadable_dir": await probe(sb.files.list(f"{d}/locked-dir")),
        "write_into_locked_dir": await probe(sb.files.write(f"{d}/locked-dir/new.txt", "n")),
        "write_to_root_dir": await probe(sb.files.write("/proc/conf-nope.txt", "x")),
        "write_to_etc": await probe(sb.files.write("/etc/conf-nope.txt", "x")),
        "make_dir_in_etc": await probe(sb.files.make_dir("/etc/conf-nope-dir")),
    }
    await sb.commands.run(f"chmod 755 {d}/locked-dir; chmod 644 {d}/locked.txt")
    return res


@case
async def fs_permission_other_user(sb):
    if NO_ROOT:
        return None
    d = "/tmp/conf-perm-user"
    await sb.files.write(f"{d}/mine.txt", "mine")
    await sb.commands.run(f"chmod 600 {d}/mine.txt", user="root")
    await sb.files.write(f"{d}/rootfile.txt", "r", user="root")
    await sb.commands.run(f"chmod 600 {d}/rootfile.txt", user="root")
    return {
        "other_user_read": await probe(sb.files.read(f"{d}/mine.txt", user="nobody")),
        "default_reads_root_file": await probe(sb.files.read(f"{d}/rootfile.txt")),
        "remove_root_file_as_user": await probe(sb.files.remove(f"{d}/rootfile.txt")),
        "write_root_file_as_user": await probe(sb.files.write(f"{d}/rootfile.txt", "x")),
        "missing_user": await probe(sb.files.read(f"{d}/mine.txt", user="nosuchuser")),
        "root_reads_any": await probe(sb.files.read(f"{d}/mine.txt", user="root")),
    }


@case
async def fs_stream_read(sb):
    p = "/tmp/conf-stream.bin"
    data = bytes(range(256)) * (3 * 4096)
    await sb.files.write(p, data)
    res = {}
    s = await sb.files.read(p, format="stream")
    res["type_has_aiter"] = hasattr(s, "__aiter__")
    got = b""
    n = 0
    async for c in s:
        got += c
        n += 1
        res["chunk_type"] = type(c).__name__
    res["reassembled"] = sha(got) == sha(data)
    res["chunked"] = n > 1
    # early close
    s2 = await sb.files.read(p, format="stream")
    first = None
    async for c in s2:
        first = c
        break
    res["early_first_chunk"] = bool(first)
    if hasattr(s2, "aclose"):
        res["aclose"] = await probe(s2.aclose())
        res["aclose_twice"] = await probe(s2.aclose())
    # async with
    async with await sb.files.read(p, format="stream") as s3:
        total = 0
        async for c in s3:
            total += len(c)
    res["async_with_total"] = total == len(data)
    res["read_after_early_close_works"] = (await sb.files.read(p, format="bytes")) == data
    res["stream_missing"] = await probe(_stream_all(sb, "/tmp/conf-nope-stream"))
    res["stream_dir"] = await probe(_stream_all(sb, "/tmp"))
    res["bad_format"] = await probe(sb.files.read(p, format="nope"))
    return res


async def _stream_all(sb, path):
    n = 0
    async for c in await sb.files.read(path, format="stream"):
        n += len(c)
    return n


async def _tree(sb) -> str:
    d = "/tmp/conf-tree"
    await sb.commands.run(
        f"rm -rf {d}; mkdir -p {d}/a/b {d}/empty; echo t > {d}/top.txt; echo h > {d}/.hidden; "
        f"echo c > {d}/a/b/c.txt; echo m > {d}/a/mid.txt; ln -s top.txt {d}/link; "
        f"ln -s /no/such/target {d}/broken; ln -s a {d}/dirlink"
    )
    return d


@case
async def fs_list_depth(sb):
    d = await _tree(sb)

    async def names(depth=None, path=d):
        kw = {} if depth is None else {"depth": depth}
        return await probe(
            sb.files.list(path, **kw),
            lambda es: sorted(f"{e.name}|{e.path.replace(d, '')}|{e.type}" for e in es),
        )

    return {
        "default": await names(),
        "depth_1": await names(1),
        "depth_2": await names(2),
        "depth_3": await names(3),
        "depth_99": await names(99),
        "depth_0": await names(0),
        "depth_neg": await names(-1),
        "depth_none": await probe(
            sb.files.list(d, depth=None), lambda es: sorted(e.name for e in es)
        ),
        "nonexistent": await names(1, "/tmp/conf-no-such-dir"),
        "on_file": await names(1, f"{d}/top.txt"),
        "empty_dir": await names(1, f"{d}/empty"),
        "trailing_slash": await probe(
            sb.files.list(d + "/"), lambda es: sorted(e.name for e in es)
        ),
        "through_symlink": await probe(
            sb.files.list(f"{d}/dirlink"), lambda es: sorted(e.name for e in es)
        ),
    }


@case
async def fs_symlinks(sb):
    d = await _tree(sb)
    out = {}
    for name in ("link", "broken", "dirlink", "top.txt", "a"):
        out[name] = await probe(
            sb.files.get_info(f"{d}/{name}"),
            lambda e: {
                "type": str(e.type),
                "symlink_target": e.symlink_target,
                "size_is_int": isinstance(e.size, int),
                "name": e.name,
            },
        )
        out[name + "_exists"] = await probe(sb.files.exists(f"{d}/{name}"))
    out["read_through_link"] = await probe(sb.files.read(f"{d}/link"))
    out["read_broken"] = await probe(sb.files.read(f"{d}/broken"))
    out["write_through_link"] = await probe(sb.files.write(f"{d}/link", "via-link"))
    out["target_after_write"] = await sb.files.read(f"{d}/top.txt")
    out["list_types"] = sorted(
        f"{e.name}:{e.type}:{e.symlink_target}" for e in await sb.files.list(d)
    )
    return out


@case
async def fs_entry_fields(sb):
    d = await _tree(sb)
    f = await sb.files.get_info(f"{d}/top.txt")
    dd = await sb.files.get_info(f"{d}/a")
    now = datetime.now(timezone.utc)

    def view(e):
        return {
            "class": type(e).__name__,
            "fields": sorted(vars(e)),
            "name": e.name,
            "type": str(e.type),
            "path_ok": e.path.startswith("/tmp/conf-tree/"),
            "size": e.size,
            "mode": oct(e.mode),
            "permissions": e.permissions,
            "owner_type": type(e.owner).__name__,
            "group_type": type(e.group).__name__,
            "mtime_aware": e.modified_time.tzinfo is not None,
            "mtime_recent": (
                abs((now - e.modified_time).total_seconds()) < 600
                if e.modified_time.tzinfo
                else None
            ),
            "symlink_target": e.symlink_target,
        }

    return {"file": view(f), "dir": view(dd)}


@case
async def fs_get_info_and_exists(sb):
    d = await _tree(sb)
    out = {}
    for name in ("top.txt", "a", "empty", "link", "broken", "missing"):
        out[name] = {
            "info": await probe(sb.files.get_info(f"{d}/{name}"), lambda e: str(e.type)),
            "exists": await probe(sb.files.exists(f"{d}/{name}")),
        }
    out["exists_file_under_file"] = await probe(sb.files.exists(f"{d}/top.txt/x"))
    out["exists_denied"] = await probe(sb.files.exists("/root/x"))
    out["exists_root_dir"] = await probe(sb.files.exists("/"))
    return out


@case
async def fs_remove(sb):
    d = "/tmp/conf-rm"
    await sb.files.write(f"{d}/dir/a.txt", "a")
    await sb.files.write(f"{d}/dir/sub/b.txt", "b")
    await sb.files.write(f"{d}/f.txt", "f")
    out = {
        "missing": await probe(sb.files.remove(f"{d}/missing")),
        "file": await probe(sb.files.remove(f"{d}/f.txt")),
        "file_gone": not await sb.files.exists(f"{d}/f.txt"),
        "dir_with_contents": await probe(sb.files.remove(f"{d}/dir")),
        "dir_gone": not await sb.files.exists(f"{d}/dir"),
        "missing_parent": await probe(sb.files.remove(f"{d}/nope/x")),
        "root_dir": await probe(sb.files.remove("/proc/1")),
    }
    return out


@case
async def fs_remove_other_user(sb):
    if NO_ROOT:
        return None
    await sb.files.write("/tmp/conf-rm-root.txt", "r", user="root")
    return {
        "as_default_user": await probe(sb.files.remove("/tmp/conf-rm-root.txt")),
        "still_there": await sb.files.exists("/tmp/conf-rm-root.txt"),
        "as_root": await probe(sb.files.remove("/tmp/conf-rm-root.txt", user="root")),
    }


@case
async def fs_rename(sb):
    d = "/tmp/conf-mv"
    await sb.files.write(f"{d}/a.txt", "a")
    await sb.files.write(f"{d}/b.txt", "b")
    await sb.files.write(f"{d}/dir/inner.txt", "in")
    info = lambda e: [type(e).__name__, e.name, e.path.replace(d, ""), str(e.type)]  # noqa: E731
    return {
        "missing_source": await probe(sb.files.rename(f"{d}/nope", f"{d}/x")),
        "over_existing": await probe(sb.files.rename(f"{d}/a.txt", f"{d}/b.txt"), info),
        "over_existing_content": await sb.files.read(f"{d}/b.txt"),
        "source_gone": not await sb.files.exists(f"{d}/a.txt"),
        "into_missing_dir": await probe(sb.files.rename(f"{d}/b.txt", f"{d}/new/deep/b.txt"), info),
        "dir_rename": await probe(sb.files.rename(f"{d}/dir", f"{d}/dir2"), info),
        "dir_contents": await probe(sb.files.read(f"{d}/dir2/inner.txt")),
        "same_path": await probe(sb.files.rename(f"{d}/dir2", f"{d}/dir2"), info),
        "dir_over_file": await _mv_over(sb, d, "dir2", "plain1.txt", info),
        "file_over_dir": await _mv_over(sb, d, "plain2.txt", "dir3", info),
    }


async def _mv_over(sb, d, src, dst, info):
    """rename `src` onto an existing `dst` of the other kind."""
    for name in (src, dst):
        if name.endswith(".txt"):
            await sb.files.write(f"{d}/{name}", name)
        elif not await sb.files.exists(f"{d}/{name}"):
            await sb.files.make_dir(f"{d}/{name}")
    r = await probe(sb.files.rename(f"{d}/{src}", f"{d}/{dst}"), info)
    r["source_exists_after"] = await sb.files.exists(f"{d}/{src}")
    return r


@case
async def fs_make_dir(sb):
    d = "/tmp/conf-mkdir"
    await sb.files.write(f"{d}/file.txt", "f")
    return {
        "nested": await probe(sb.files.make_dir(f"{d}/a/b/c")),
        "existing": await probe(sb.files.make_dir(f"{d}/a/b/c")),
        "existing_parent": await probe(sb.files.make_dir(f"{d}/a")),
        "over_file": await probe(sb.files.make_dir(f"{d}/file.txt")),
        "under_file": await probe(sb.files.make_dir(f"{d}/file.txt/sub")),
        "trailing_slash": await probe(sb.files.make_dir(f"{d}/t/")),
        "unicode": await probe(sb.files.make_dir(f"{d}/dir ünï ✓")),
        "mode": oct((await sb.files.get_info(f"{d}/a/b/c")).mode),
    }


@case
async def fs_large_directory(sb):
    d = "/tmp/conf-500"
    await sb.commands.run(
        f"rm -rf {d}; mkdir -p {d}; cd {d}; i=0; while [ $i -lt 500 ]; do : > f$i.txt; i=$((i+1)); done"
    )
    es = await sb.files.list(d)
    return {
        "count": len(es),
        "names_ok": sorted(e.name for e in es) == sorted(f"f{i}.txt" for i in range(500)),
        "all_files": all(str(e.type).endswith("FILE") for e in es),
    }


@case
async def fs_error_texts(sb):
    # exception texts users match on
    return {
        "read_missing": await probe(sb.files.read("/tmp/conf-nope-404")),
        "get_info_missing": await probe(sb.files.get_info("/tmp/conf-nope-404")),
        "list_missing": await probe(sb.files.list("/tmp/conf-nope-404")),
        "rename_missing": await probe(sb.files.rename("/tmp/conf-nope-404", "/tmp/conf-x")),
        "bytes_missing": await probe(sb.files.read("/tmp/conf-nope-404", format="bytes")),
    }
