"""Conformance cases: the Sandbox object and class API (async). Cases that
create sandboxes of their own are `@fresh`; the rest share one."""

from __future__ import annotations

import asyncio
import json
import re
import shlex
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timedelta, timezone

import e2b
from e2b import AsyncSandbox

from .common import (
    NO_ROOT,
    ROOT,
    SCHEME,
    SHARED,
    case,
    envd_token,
    exc_info,
    fresh,
    has_param,
    host_shape,
    mask,
    newer,
    probe,
    try_create,
    ts_aware,
    until,
)

UNKNOWN_ID = "iconf0000000000000000"
ALL_TRAFFIC = getattr(e2b, "ALL_TRAFFIC", "0.0.0.0/0")


def _info_view(i) -> dict:
    return {
        "type": type(i).__name__,
        "fields": sorted(vars(i)),
        "id_is_str": isinstance(i.sandbox_id, str),
        "template_id_is_str": isinstance(i.template_id, str),
        "name": i.name,
        "metadata": i.metadata,
        "started_aware": ts_aware(i.started_at),
        "end_aware": ts_aware(i.end_at),
        "end_after_start": i.end_at > i.started_at,
        "state": str(i.state),
        "cpu_count_type": type(i.cpu_count).__name__,
        "memory_mb_type": type(i.memory_mb).__name__,
        "envd_version_is_str": isinstance(i.envd_version, str),
    }


# ── shared sandbox ─────────────────────────────────────────────────────────


@case
async def sbx_info_fields(sb):
    i = await sb.get_info()
    now = datetime.now(timezone.utc)
    return {
        **_info_view(i),
        "started_recently": abs((now - i.started_at).total_seconds()) < 3600,
        "allow_internet_access": getattr(i, "allow_internet_access", "n/a"),
        "volume_mounts": getattr(i, "volume_mounts", "n/a"),
        "via_class_matches": (await AsyncSandbox.get_info(sb.sandbox_id)).sandbox_id
        == i.sandbox_id,
    }


@case
async def sbx_end_at_moves(sb):
    now = lambda: datetime.now(timezone.utc)  # noqa: E731
    out = {}
    before = (await sb.get_info()).end_at
    r = await sb.set_timeout(300)
    out["returns"] = r
    after = (await sb.get_info()).end_at
    out["shrinks"] = after < before
    out["end_in_300"] = abs((after - now()).total_seconds() - 300) < 45
    await sb.set_timeout(1500)
    longer = (await sb.get_info()).end_at
    out["extends"] = longer > after
    out["end_in_1500"] = abs((longer - now()).total_seconds() - 1500) < 45
    out["delta_matches"] = abs((longer - after).total_seconds() - 1200) < 45
    out["via_class"] = await probe(AsyncSandbox.set_timeout(sb.sandbox_id, 1500))
    return out


@case
async def sbx_metrics_types(sb):
    ms = []
    for _ in range(14):
        ms = await sb.get_metrics()
        if ms:
            break
        await asyncio.sleep(2)
    if not ms:
        return {"empty": True}
    m = ms[-1]
    info = await sb.get_info()
    return {
        "type": type(m).__name__,
        "fields": sorted(vars(m)),
        "cpu_count_is_int": isinstance(m.cpu_count, int),
        "cpu_count_matches_info": m.cpu_count == info.cpu_count,
        "cpu_used_pct_is_float": isinstance(m.cpu_used_pct, float),
        "cpu_used_pct_in_range": 0 <= m.cpu_used_pct <= 100 * m.cpu_count,
        "mem_total_is_bytes": m.mem_total > 100 * 1024 * 1024,
        "mem_total_near_info": abs(m.mem_total / (1024 * 1024) - info.memory_mb)
        < info.memory_mb * 0.3,
        "mem_used_le_total": m.mem_used <= m.mem_total,
        "disk_total_is_bytes": m.disk_total > 10**9,
        "disk_used_le_total": m.disk_used <= m.disk_total,
        "timestamp_aware": ts_aware(m.timestamp),
        "timestamp_recent": (
            abs((datetime.now(timezone.utc) - m.timestamp).total_seconds()) < 300
            if m.timestamp.tzinfo
            else None
        ),
        "sorted_ascending": [x.timestamp for x in ms] == sorted(x.timestamp for x in ms),
        "future_start": await probe(
            sb.get_metrics(start=datetime.now(timezone.utc) + timedelta(hours=1)),
            lambda xs: len(xs),
        ),
        "window_ok": await probe(
            sb.get_metrics(
                start=datetime.now(timezone.utc) - timedelta(minutes=10),
                end=datetime.now(timezone.utc),
            ),
            lambda xs: type(xs).__name__,
        ),
        "end_before_start": await probe(
            sb.get_metrics(
                start=datetime.now(timezone.utc),
                end=datetime.now(timezone.utc) - timedelta(minutes=10),
            ),
            lambda xs: len(xs),
        ),
        "via_class": await probe(
            AsyncSandbox.get_metrics(sb.sandbox_id), lambda xs: type(xs).__name__
        ),
    }


@case
async def sbx_get_host_ports(sb):
    root = "/tmp/conf-www"
    await sb.commands.run(f"mkdir -p {root}; echo conf-index > {root}/index.html")
    await sb.commands.run(
        "python3 -m http.server 8081", background=True, cwd=root, timeout=0, **ROOT
    )
    await asyncio.sleep(2)
    out = {"formats": {}}
    for p in (1, 80, 3000, 8081, 65535, 0, -1, 70000, "8081", None):
        out["formats"][repr(p)] = await probe(
            _wrap(sb.get_host, p), lambda h: host_shape(h, sb.sandbox_id)
        )

    async def fetch(port):
        url = f"{SCHEME}://{sb.get_host(port)}/"

        def go():
            try:
                with urllib.request.urlopen(url, timeout=10) as r:
                    return {"status": r.status, "body": r.read(200).decode(errors="replace")}
            except urllib.error.HTTPError as e:
                body = e.read(500).decode(errors="replace")
                try:
                    j = json.loads(body)
                    shape = {
                        k: (mask(v) if isinstance(v, str) else type(v).__name__)
                        for k, v in j.items()
                    }
                except ValueError:
                    shape = body[:80]
                return {
                    "status": e.code,
                    "content_type": e.headers.get("content-type"),
                    "body": shape,
                }
            except Exception as e:  # noqa: BLE001
                return {"error": type(e).__name__}

        for _ in range(8):
            r = await asyncio.to_thread(go)
            if r.get("status") == 200 or port != 8081:
                return r
            await asyncio.sleep(1)
        return r

    out["in_use"] = await fetch(8081)
    out["unused"] = await fetch(8099)
    return out


async def _wrap(fn, *a):
    return fn(*a)


@case
async def sbx_is_running_shared(sb):
    return {
        "running": await sb.is_running(),
        "with_timeout": await sb.is_running(request_timeout=20),
        "type": type(await sb.is_running()).__name__,
    }


@case
async def sbx_secure_default(sb):
    return {
        "envd_token_present": envd_token(sb) is not None,
        "traffic_token_attr": hasattr(sb, "traffic_access_token"),
        "sandbox_id_str": isinstance(sb.sandbox_id, str),
        "sandbox_domain_str": isinstance(sb.sandbox_domain, str),
    }


def _url_shape(u: str, sid: str = "") -> dict:
    p = urllib.parse.urlsplit(u)
    q = urllib.parse.parse_qs(p.query, keep_blank_values=True)
    return {
        "host": host_shape(p.netloc, sid) if sid else None,
        "scheme_is_http": p.scheme in ("http", "https"),
        "path": p.path,
        "query_keys": sorted(q),
        "path_param": q.get("path"),
        "username_param": q.get("username"),
        "signature_masked": bool(q.get("signature")) and q["signature"][0].startswith("v1_"),
    }


def _http(method: str, url: str, body: bytes | None = None, headers: dict | None = None) -> dict:
    req = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            raw = r.read()
            return {"status": r.status, "body": raw}
    except urllib.error.HTTPError as e:
        return {"status": e.code, "body": e.read()}
    except Exception as e:  # noqa: BLE001
        return {"error": type(e).__name__, "body": b""}


def _multipart(name: str, content: bytes) -> tuple[bytes, dict]:
    b = "----confboundary" + uuid.uuid4().hex[:12]
    body = (
        (
            f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
            f"Content-Type: application/octet-stream\r\n\r\n"
        ).encode()
        + content
        + f"\r\n--{b}--\r\n".encode()
    )
    return body, {"Content-Type": f"multipart/form-data; boundary={b}"}


@case
async def sbx_file_urls(sb):
    path = "/tmp/conf-url dir/file é.txt"
    out = {}
    out["upload_shape"] = _url_shape(sb.upload_url(path), sb.sandbox_id)
    out["download_shape"] = _url_shape(sb.download_url(path), sb.sandbox_id)
    out["with_expiration"] = _expiring(sb, path)
    try:
        out["with_user"] = _url_shape(sb.download_url(path, user="root"))["username_param"]
    except Exception as e:  # noqa: BLE001
        out["with_user"] = {"raised": exc_info(e, True)}
    body, hdr = _multipart("file é.txt", b"uploaded \xff bytes")
    up = await asyncio.to_thread(_http, "POST", sb.upload_url(path), body, hdr)
    out["upload_status"] = up["status"]
    try:
        j = json.loads(up["body"])
        out["upload_response"] = (
            [{k: (type(v).__name__ if k == "path" else v) for k, v in x.items()} for x in j]
            if isinstance(j, list)
            else type(j).__name__
        )
    except ValueError:
        out["upload_response"] = up["body"][:60].decode(errors="replace")
    out["written_bytes"] = bytes(await sb.files.read(path, format="bytes")).hex()
    dl = await asyncio.to_thread(_http, "GET", sb.download_url(path))
    out["download_status"] = dl["status"]
    out["download_body"] = dl["body"].hex()
    out["download_missing"] = (
        await asyncio.to_thread(_http, "GET", sb.download_url("/tmp/conf-no-such-file"))
    )["status"]
    good = sb.download_url(path)
    bad = re.sub(r"signature=([^&]{3})", r"signature=xxx", good)
    out["bad_signature"] = (await asyncio.to_thread(_http, "GET", bad))["status"]
    unsigned = re.sub(r"&?signature(_expiration)?=[^&]*", "", good)
    out["unsigned"] = (await asyncio.to_thread(_http, "GET", unsigned))["status"]
    up2 = await asyncio.to_thread(_http, "POST", sb.download_url(path), body, hdr)
    out["write_with_read_signature"] = up2["status"]
    return out


def _expiring(sb, path):
    return {
        "upload": _url_shape(sb.upload_url(path, use_signature_expiration=60))["query_keys"],
        "download": _url_shape(sb.download_url(path, use_signature_expiration=60))["query_keys"],
    }


@case
async def sbx_unknown_ids(sb):
    return {
        "connect": await probe(AsyncSandbox.connect(UNKNOWN_ID)),
        "kill": await probe(AsyncSandbox.kill(UNKNOWN_ID)),
        "get_info": await probe(AsyncSandbox.get_info(UNKNOWN_ID)),
        "get_metrics": await probe(AsyncSandbox.get_metrics(UNKNOWN_ID)),
        "set_timeout": await probe(AsyncSandbox.set_timeout(UNKNOWN_ID, 60)),
        "pause": await probe(AsyncSandbox.pause(UNKNOWN_ID)),
        "malformed_id_connect": await probe(AsyncSandbox.connect("not a valid id!")),
        "malformed_id_kill": await probe(AsyncSandbox.kill("not a valid id!")),
        "empty_id_kill": await probe(AsyncSandbox.kill("")),
    }


@case
async def sbx_fork_validation(sb):
    out = {}
    for label, count in (
        ("zero", 0),
        ("negative", -1),
        ("twenty_one", 21),
        ("bool", True),
        ("str", "2"),
        ("float", 1.5),
    ):
        out[label] = await _fork_try(sb, count)
    return out


async def _fork_try(sb, count):
    try:
        kids = await sb.fork(count=count, timeout=120)
    except Exception as e:  # noqa: BLE001
        return {"raised": exc_info(e, True)}
    for k in kids:  # accepted: do not leave the forks running
        if isinstance(k, AsyncSandbox):
            await k.kill()
    return {"ok": [type(k).__name__ for k in kids]}


@case
async def sbx_kill_class_form_on_shared_id(sb):
    # a second handle to the same sandbox sees the same state
    other = await AsyncSandbox.connect(sb.sandbox_id)
    r = await other.commands.run("echo via-connect")
    return {
        "same_id": other.sandbox_id == sb.sandbox_id,
        "type": type(other).__name__,
        "stdout": r.stdout,
        "files_shared": await _share(sb, other),
    }


async def _share(a, b):
    await a.files.write("/tmp/conf-share.txt", "shared")
    return await b.files.read("/tmp/conf-share.txt")


# ── sandboxes of their own ─────────────────────────────────────────────────


@fresh
async def sbx_list_pagination(template):
    rid = SHARED["run_id"]
    notes = ["plain", "a b&c=d é", "x"]
    made = []
    out = {}
    try:
        for i in range(3):
            made.append(
                await AsyncSandbox.create(
                    template=template,
                    timeout=240,
                    metadata={"suite": "e2b", "grp": rid, "idx": str(i), "note": notes[i]},
                )
            )
            await asyncio.sleep(1.2)
        ids = [s.sandbox_id for s in made]
        mine = set(ids)

        async def all_pages(query=None, limit=None, order=None):
            kw = {}
            if order is not None:
                kw["order"] = order
            p = AsyncSandbox.list(query=query, limit=limit, **kw)
            pages = []
            while p.has_next:
                items = await p.next_items()
                pages.append([i.sandbox_id for i in items])
                if len(pages) > 50:
                    break
            return p, pages

        Q = e2b.SandboxQuery
        grp = Q(metadata={"grp": rid})
        p, pages = await all_pages(grp)
        out["by_metadata"] = {
            "ids_equal": set(sum(pages, [])) == mine,
            "paginator": type(p).__name__,
            "item_type": type((await AsyncSandbox.list(query=grp).next_items())[0]).__name__,
        }
        two = await AsyncSandbox.list(query=Q(metadata={"grp": rid, "idx": "1"})).next_items()
        out["two_keys"] = sorted(i.metadata["idx"] for i in two)
        out["special_chars_value"] = len(
            await AsyncSandbox.list(
                query=Q(metadata={"grp": rid, "note": "a b&c=d é"})
            ).next_items()
        )
        out["no_match"] = len(
            await AsyncSandbox.list(query=Q(metadata={"grp": rid + "zz"})).next_items()
        )
        # one at a time
        p, pages = await all_pages(grp, limit=1)
        out["limit_1"] = {
            "page_sizes": [len(x) for x in pages],
            "covers_all": sorted(sum(pages, [])) == sorted(mine),
            "no_repeats": len(sum(pages, [])) == len(set(sum(pages, []))),
            "has_next_at_end": p.has_next,
            "next_token_at_end": p.next_token,
        }
        p = AsyncSandbox.list(query=grp, limit=1)
        first = await p.next_items()
        out["first_page"] = {
            "len": len(first),
            "has_next": p.has_next,
            "next_token_is_str": isinstance(p.next_token, str),
        }
        out["resume_from_token"] = await probe(
            AsyncSandbox.list(query=grp, limit=1, next_token=p.next_token).next_items(),
            lambda xs: [len(xs), xs[0].sandbox_id != first[0].sandbox_id],
        )
        out["exhausted_next_items"] = await probe(_exhaust(grp), lambda x: x)
        out["bad_next_token"] = await probe(
            AsyncSandbox.list(query=grp, next_token="garbage-token").next_items(),
            lambda xs: len(xs),
        )
        for label, lim in (("limit_0", 0), ("limit_neg", -1), ("limit_huge", 100000)):
            out[label] = await probe(
                AsyncSandbox.list(query=grp, limit=lim).next_items(), lambda xs: len(xs)
            )
        # state filter
        S = e2b.SandboxState
        running = await AsyncSandbox.list(
            query=Q(metadata={"grp": rid}, state=[S.RUNNING])
        ).next_items()
        paused = await AsyncSandbox.list(
            query=Q(metadata={"grp": rid}, state=[S.PAUSED])
        ).next_items()
        both = await AsyncSandbox.list(
            query=Q(metadata={"grp": rid}, state=[S.RUNNING, S.PAUSED])
        ).next_items()
        out["state_filter"] = {
            "running": len(running),
            "paused": len(paused),
            "both": len(both),
            "state_values": sorted({str(i.state) for i in running}),
        }
        # started_after
        now = datetime.now(timezone.utc)
        if "started_after" in getattr(Q, "__dataclass_fields__", {}):
            for label, at in (
                ("past", now - timedelta(hours=1)),
                ("future", now + timedelta(hours=1)),
            ):
                got = await AsyncSandbox.list(
                    query=Q(metadata={"grp": rid}, started_after=at)
                ).next_items()
                out.setdefault("started_after", {})[label] = len(got)
        else:
            out["started_after"] = {"since": "2.56"}
        # order
        if has_param(AsyncSandbox.list, "order"):
            for order in ("asc", "desc"):
                _, pg = await all_pages(grp, order=order)
                out[f"order_{order}"] = sum(pg, []) == (ids if order == "asc" else ids[::-1])
            out["order_bad"] = await probe(
                AsyncSandbox.list(query=grp, order="sideways").next_items(), lambda xs: len(xs)
            )
        else:
            for key in ("order_asc", "order_desc", "order_bad"):
                out[key] = {"since": "2.56"}
        # metadata round trip through get_info and list items
        infos = await AsyncSandbox.list(query=grp).next_items()
        out["metadata_in_list"] = sorted([i.metadata["idx"], i.metadata["note"]] for i in infos)
        out["metadata_in_get_info"] = sorted([(await s.get_info()).metadata["note"] for s in made])
        out["list_item_fields"] = sorted(vars(infos[0])) if infos else None
        out["list_item_view"] = _info_view(infos[0]) if infos else None
        if infos:
            out["list_item_view"]["metadata"] = {
                k: ("<run>" if k == "grp" else v) for k, v in infos[0].metadata.items()
            }
        # the default list shows running ones
        default = await AsyncSandbox.list().next_items()
        out["default_list_has_mine"] = mine <= {i.sandbox_id for i in default} or "paged"
        return out
    finally:
        for s in made:
            try:
                await s.kill()
            except Exception:  # noqa: BLE001
                pass


async def _exhaust(query):
    p = AsyncSandbox.list(query=query)
    while p.has_next:
        await p.next_items()
    return await p.next_items()


@fresh
async def sbx_set_timeout_edges(template):
    sb = await AsyncSandbox.create(template=template, timeout=300)
    out = {}
    try:
        for label, v in (
            ("negative", -5),
            ("huge", 10**9),
            ("float", 1.5),
            ("str", "10"),
            ("none", None),
            ("bool", True),
            ("seconds_in_day_plus", 86400 * 2),
        ):
            r = await probe(sb.set_timeout(v))
            r["running_after"] = await sb.is_running()
            if "ok" in r or "raised" not in r:
                end = (await sb.get_info()).end_at
                r["end_in_hours_le_48"] = (
                    end - datetime.now(timezone.utc)
                ).total_seconds() < 48 * 3600
            out[label] = r
        await sb.set_timeout(300)
        out["zero"] = await probe(sb.set_timeout(0))
        await asyncio.sleep(2)
        out["running_after_zero"] = await sb.is_running()
        out["info_after_zero"] = await probe(sb.get_info(), lambda i: str(i.state))
        return out
    finally:
        try:
            await AsyncSandbox.kill(sb.sandbox_id)
        except Exception:  # noqa: BLE001
            pass


@fresh
async def sbx_create_validation(template):
    out = {}
    out["bad_template"] = await try_create(template="no-such-template-conf-x")
    out["bad_template_chars"] = await try_create(template="Not A Template!!")
    for label, kw in (
        ("timeout_negative", {"timeout": -1}),
        ("timeout_zero", {"timeout": 0}),
        ("timeout_float", {"timeout": 30.5}),
        ("timeout_str", {"timeout": "30"}),
        ("metadata_int_value", {"metadata": {"k": 1}}),
        ("metadata_empty_key", {"metadata": {"": "v"}}),
        ("metadata_long_value", {"metadata": {"k": "v" * 5000}}),
        ("envs_int_value", {"envs": {"A": 1}}),
        ("envs_bad_name", {"envs": {"A B": "1"}}),
        ("allow_internet_str", {"allow_internet_access": "no"}),
    ):
        out[label] = await try_create(template=template, **kw)
    return out


@fresh
async def sbx_create_options_validation(template):
    out = {}
    if has_param(AsyncSandbox.create, "network"):
        for label, net in (
            ("network_allow_str", {"allow_out": "1.1.1.1"}),
            ("network_deny_int_items", {"deny_out": [1]}),
            ("network_allow_bad_ip", {"allow_out": ["not an ip!!"]}),
            ("network_allow_bad_cidr", {"allow_out": ["1.1.1.1/33"]}),
            ("network_allow_empty_item", {"allow_out": [""]}),
            ("network_unknown_key", {"bogus": []}),
            ("network_rules_not_dict", {"rules": ["x"]}),
            ("network_transform_not_mapping", {"rules": {"example.com": [{"transform": 5}]}}),
            ("network_not_a_dict", "deny"),
        ):
            out[label] = await try_create(template=template, network=net)
    else:
        out["network"] = {"since": "2.56"}
    if has_param(AsyncSandbox.create, "iam"):
        tok = {"audience": "aud", "token_type": "JWT-SVID"}
        for label, iam in (
            ("iam_empty_name", {"tokens": {"": tok}}),
            ("iam_brace_name", {"tokens": {"a{b": tok}}),
            ("iam_control_name", {"tokens": {"a\nb": tok}}),
            ("iam_missing_audience", {"tokens": {"x": {"token_type": "JWT-SVID"}}}),
            ("iam_audience_int", {"tokens": {"x": {"audience": 1, "token_type": "JWT-SVID"}}}),
            ("iam_tokens_not_dict", {"tokens": ["x"]}),
            ("iam_not_a_dict", "tokens"),
        ):
            out[label] = await try_create(template=template, iam=iam)
    else:
        out["iam"] = {"since": "2.58"}
    if has_param(AsyncSandbox.create, "mcp"):
        for label, mcp in (("mcp_not_a_dict", "github"),):
            out[label] = await try_create(template=template, mcp=mcp)
    else:
        out["mcp"] = {"since": "2.56"}
    if has_param(AsyncSandbox.create, "lifecycle"):
        for label, lc in (
            ("lifecycle_str", "pause"),
            ("lifecycle_auto_resume_only", {"auto_resume": True}),
            ("lifecycle_pause_ok", {"on_timeout": "pause"}),
            ("lifecycle_pause_auto_resume_ok", {"on_timeout": "pause", "auto_resume": True}),
            ("lifecycle_auto_resume_str", {"on_timeout": "pause", "auto_resume": "yes"}),
            ("lifecycle_action_none", {"on_timeout": {"action": None}}),
        ):
            out[label] = await try_create(template=template, lifecycle=lc)
    else:
        out["lifecycle"] = {"since": "2.56"}
    return out


@fresh
async def sbx_context_manager(template):
    """`async with`, envs, secure=False, template="base"."""
    ready = SHARED.get("ready")
    if ready is not None:
        await asyncio.wait_for(ready.wait(), 60)
    shared = SHARED.get("sb")
    out = {}
    # read now: the shared sandbox is killed when the shared cases are done,
    # which can be before this slower case reaches its end
    default_name = None
    if shared is not None:
        try:
            default_name = (await shared.get_info()).name
        except e2b.SandboxException:  # a run of this case alone: already torn down
            pass
    sb = await AsyncSandbox.create(
        template="base",
        timeout=180,
        envs={"CONF_E": "é ✓"},
        secure=False,
        allow_internet_access=True,
        metadata={"suite": "e2b", "case": "ctx"},
    )
    sid = sb.sandbox_id
    async with sb as entered:
        out["enter_returns_self"] = entered is sb
        out["env"] = (await sb.commands.run('printf %s "$CONF_E"')).stdout
        out["files"] = await _share(sb, sb)
        out["outbound"] = await _reach(sb, A)
        out["envd_token_present"] = envd_token(sb) is not None
        out["unsigned_url_shape"] = _url_shape(sb.download_url("/tmp/conf-share.txt"))["query_keys"]
        out["expiration_unsecured"] = _unsecured_expiration(sb)
        info = await sb.get_info()
        out["info_name_base"] = info.name
        out["info_metadata"] = info.metadata
        out["info_allow_internet"] = getattr(info, "allow_internet_access", "n/a")
        if shared is not None:
            out["info_name_default_template"] = default_name
    out["after_exit_running"] = await probe(AsyncSandbox.get_info(sid), lambda i: str(i.state))
    out["after_exit_kill"] = await probe(AsyncSandbox.kill(sid))
    out["after_exit_connect"] = await probe(AsyncSandbox.connect(sid))
    # an exception inside the block still kills
    sb2 = await AsyncSandbox.create(template=template, timeout=120)
    try:
        async with sb2:
            raise RuntimeError("boom")
    except RuntimeError:
        pass
    out["killed_on_exception"] = await probe(
        AsyncSandbox.get_info(sb2.sandbox_id), lambda i: str(i.state)
    )
    return out


def _unsecured_expiration(sb):
    try:
        sb.upload_url("/tmp/x", use_signature_expiration=10)
        return {"ok": True}
    except Exception as e:  # noqa: BLE001
        return {"raised": exc_info(e, True)}


@fresh
async def sbx_lifecycle_calls(template):
    """Class-method forms and the calls on a sandbox after it was killed."""
    sb = await AsyncSandbox.create(
        template=template, timeout=300, metadata={"suite": "e2b", "case": "calls"}
    )
    sid = sb.sandbox_id
    out = {}
    try:
        out["class_get_info"] = await probe(
            AsyncSandbox.get_info(sid), lambda i: [type(i).__name__, str(i.state), i.metadata]
        )
        out["class_set_timeout"] = await probe(AsyncSandbox.set_timeout(sid, 400))
        out["class_get_metrics"] = await probe(
            AsyncSandbox.get_metrics(sid), lambda xs: type(xs).__name__
        )
        out["is_running"] = await sb.is_running()
        again = await AsyncSandbox.connect(sid)
        out["connect_runs"] = (await again.commands.run("echo c")).stdout
        out["connect_with_timeout"] = await probe(
            AsyncSandbox.connect(sid, timeout=500), lambda s: s.sandbox_id == sid
        )
        out["class_kill"] = await probe(AsyncSandbox.kill(sid))
        out["class_kill_again"] = await probe(AsyncSandbox.kill(sid))
        out["instance_kill_again"] = await probe(sb.kill())
        out["is_running_after"] = await probe(sb.is_running())
        out["connect_after_kill"] = await probe(AsyncSandbox.connect(sid))
        out["get_info_after_kill"] = await probe(AsyncSandbox.get_info(sid), lambda i: str(i.state))
        out["get_info_instance_after_kill"] = await probe(sb.get_info(), lambda i: str(i.state))
        out["get_metrics_after_kill"] = await probe(sb.get_metrics(), lambda xs: type(xs).__name__)
        out["set_timeout_after_kill"] = await probe(sb.set_timeout(60))
        out["run_after_kill"] = await probe(
            sb.commands.run("echo x", request_timeout=20), lambda r: r.stdout
        )
        out["files_read_after_kill"] = await probe(
            sb.files.read("/etc/hostname", request_timeout=20)
        )
        out["files_list_after_kill"] = await probe(sb.files.list("/tmp", request_timeout=20))
        out["files_write_after_kill"] = await probe(
            sb.files.write("/tmp/x", "x", request_timeout=20)
        )
        out["commands_list_after_kill"] = await probe(sb.commands.list(request_timeout=20))
        out["pty_after_kill"] = await probe(
            sb.pty.create(e2b.PtySize(rows=24, cols=80), on_data=lambda d: None, request_timeout=20)
        )
        out["get_host_after_kill"] = await probe(
            _wrap(sb.get_host, 8080), lambda h: host_shape(h, sid)
        )
        out["upload_url_after_kill"] = await probe(
            _wrap(sb.upload_url, "/tmp/x"), lambda u: bool(u)
        )
        out["pause_after_kill"] = await probe(sb.pause())
        out["list_has_killed"] = sid in {
            i.sandbox_id
            for i in await AsyncSandbox.list(
                query=e2b.SandboxQuery(metadata={"case": "calls", "suite": "e2b"})
            ).next_items()
        }
    finally:
        try:
            await AsyncSandbox.kill(sid)
        except Exception:  # noqa: BLE001
            pass
    return out


@fresh
async def sbx_paused_ops(template):
    """pause / connect / pause twice / kill while paused (class forms)."""
    sb = await AsyncSandbox.create(
        template=template, timeout=600, metadata={"suite": "e2b", "case": "paused"}
    )
    sid = sb.sandbox_id
    out = {}
    try:
        await sb.files.write("/tmp/conf-kept.txt", "kept")
        out["pause_class"] = await probe(AsyncSandbox.pause(sid))
        out["info_paused"] = await probe(AsyncSandbox.get_info(sid), lambda i: str(i.state))
        out["instance_info_paused"] = await probe(sb.get_info(), lambda i: str(i.state))
        out["is_running_paused"] = await probe(sb.is_running())
        out["pause_twice"] = await probe(AsyncSandbox.pause(sid))
        out["instance_pause_twice"] = await probe(sb.pause())
        out["run_while_paused"] = await probe(
            sb.commands.run("echo x", request_timeout=20), lambda r: r.stdout
        )
        out["set_timeout_paused"] = await probe(AsyncSandbox.set_timeout(sid, 300))
        out["metrics_paused"] = await probe(
            AsyncSandbox.get_metrics(sid), lambda xs: type(xs).__name__
        )
        listed = [
            i
            for i in await AsyncSandbox.list(
                query=e2b.SandboxQuery(
                    metadata={"case": "paused", "suite": "e2b"}, state=[e2b.SandboxState.PAUSED]
                )
            ).next_items()
            if i.sandbox_id == sid
        ]
        out["listed_as_paused"] = len(listed)
        out["connect_class"] = await probe(_connect_read(sid), lambda t: t)
        out["running_after_connect"] = await sb.is_running()
        out["connect_again"] = await probe(_connect_read(sid), lambda t: t)
        out["pause_after_resume"] = await probe(AsyncSandbox.pause(sid))
        out["connect_with_timeout"] = await probe(_connect_read(sid, timeout=400), lambda t: t)
        await AsyncSandbox.pause(sid)
        out["kill_while_paused"] = await probe(AsyncSandbox.kill(sid))
        out["info_after_kill"] = await probe(AsyncSandbox.get_info(sid), lambda i: str(i.state))
        out["connect_after_kill"] = await probe(AsyncSandbox.connect(sid))
    finally:
        try:
            await AsyncSandbox.kill(sid)
        except Exception:  # noqa: BLE001
            pass
    return out


async def _connect_read(sid, **kw):
    s = await AsyncSandbox.connect(sid, **kw)
    return await s.files.read("/tmp/conf-kept.txt")


@fresh
async def snapshot_pagination(template):
    if not newer("create_snapshot", "mode"):
        return {"since": "2.56"}
    sb = await AsyncSandbox.create(
        template=template, timeout=600, metadata={"suite": "e2b", "case": "snaps"}
    )
    made = []
    out = {}
    try:
        for i in range(3):
            snap = await sb.create_snapshot(mode="filesystem")
            made.append(snap.snapshot_id)
        out["snapshot_view"] = {
            "type": type(snap).__name__,
            "fields": sorted(vars(snap)),
            "id_is_str": isinstance(snap.snapshot_id, str),
        }
        p = sb.list_snapshots(limit=1)
        pages = []
        while p.has_next and len(pages) < 40:
            pages.append([s.snapshot_id for s in await p.next_items()])
        flat = sum(pages, [])
        out["limit_1"] = {
            "all_made_listed": set(made) <= set(flat),
            "page_sizes_le_1": all(len(x) <= 1 for x in pages),
            "no_repeats": len(flat) == len(set(flat)),
            "ends": p.has_next,
        }
        full = await sb.list_snapshots().next_items()
        out["unlimited_has_all"] = set(made) <= {s.snapshot_id for s in full}
        out["by_name_missing"] = (
            await probe(
                sb.list_snapshots(name="conf-no-such-name").next_items(), lambda xs: len(xs)
            )
            if has_param(sb.list_snapshots, "name")
            else "n/a"
        )
        out["bad_limit"] = await probe(sb.list_snapshots(limit=0).next_items(), lambda xs: len(xs))
        out["delete"] = [await AsyncSandbox.delete_snapshot(s) for s in made]
        out["delete_twice"] = [await probe(AsyncSandbox.delete_snapshot(s)) for s in made]
        out["delete_unknown"] = await probe(
            AsyncSandbox.delete_snapshot("conf/no-such-snapshot:v0")
        )
        out["delete_malformed"] = await probe(AsyncSandbox.delete_snapshot("!! bad"))
    finally:
        for s in made:
            try:
                await AsyncSandbox.delete_snapshot(s)
            except Exception:  # noqa: BLE001
                pass
        await AsyncSandbox.kill(sb.sandbox_id)
    return out


async def _reach(sb, host: str, port: int = 80) -> str:
    """`open` when an HTTP server answers (any status), else `blocked`. A TCP
    connect is no probe: E2B's filter lets the handshake through."""
    code = (
        "import urllib.request, urllib.error\n"
        "class NoRedirect(urllib.request.HTTPRedirectHandler):\n"
        "    def redirect_request(self, *a, **k): return None\n"
        "op = urllib.request.build_opener(NoRedirect)\n"
        "try:\n"
        f" op.open('http://{host}:{port}/', timeout=5); print('open')\n"
        "except urllib.error.HTTPError: print('open')\n"
        "except Exception: print('blocked')\n"
    )
    r = await sb.commands.run(f"python3 -c {shlex.quote(code)}", timeout=40)
    return r.stdout.strip()


async def _https(sb, url: str) -> str:
    code = (
        "import urllib.request\ntry:\n"
        f" print(urllib.request.urlopen({url!r}, timeout=5).status)\n"
        "except Exception as e: print('blocked')\n"
    )
    r = await sb.commands.run(f"python3 -c {shlex.quote(code)}", timeout=40)
    return r.stdout.strip()


A, B = "1.1.1.1", "1.0.0.1"  # two addresses that both answer on port 80


async def _both(sb) -> dict:
    return {A: await _reach(sb, A), B: await _reach(sb, B)}


@fresh
async def sbx_network_update(template):
    if not hasattr(AsyncSandbox, "update_network") or not has_param(AsyncSandbox.create, "network"):
        return {"since": "2.58"}
    sb = await AsyncSandbox.create(
        template=template,
        timeout=300,
        network={"allow_out": [A], "deny_out": [ALL_TRAFFIC]},
    )
    out = {}
    try:
        out["create_allow_one"] = await _both(sb)
        out["info_network"] = getattr(await sb.get_info(), "network", "n/a")

        async def step(label, upd):
            out[label] = {"call": await probe(sb.update_network(upd)), **await _both(sb)}

        await step("deny_all", {"deny_out": [ALL_TRAFFIC]})
        await step("allow_ip", {"deny_out": [ALL_TRAFFIC], "allow_out": [B]})
        await step("allow_cidr", {"deny_out": [ALL_TRAFFIC], "allow_out": ["1.1.1.0/24"]})
        await step("deny_cidr_only", {"deny_out": ["1.0.0.0/24"]})
        out["info_after_update"] = getattr(await sb.get_info(), "network", "n/a")
        out["allow_hostname"] = {
            "call": await probe(
                sb.update_network({"deny_out": [ALL_TRAFFIC], "allow_out": ["example.com"]})
            ),
            "example.com": await _https(sb, "https://example.com/"),
            "other": await _https(sb, "https://www.iana.org/"),
        }
        out["deny_hostname"] = {
            "call": await probe(sb.update_network({"deny_out": ["example.com"]})),
        }
        await step("clear", {})
        # validation: the sandbox keeps what it had
        await sb.update_network({"deny_out": [ALL_TRAFFIC], "allow_out": [A]})
        bad = {}
        for label, upd in (
            ("allow_str", {"deny_out": [ALL_TRAFFIC], "allow_out": "1.1.1.1"}),
            ("bad_ip", {"deny_out": [ALL_TRAFFIC], "allow_out": ["not an ip!!"]}),
            ("allow_without_deny_all", {"allow_out": [A]}),
            ("octet_out_of_range", {"deny_out": ["999.1.1.1"]}),
            ("cidr_out_of_range", {"deny_out": ["1.1.1.1/33"]}),
            ("empty_item", {"deny_out": [""]}),
            ("int_item", {"deny_out": [1]}),
            ("unknown_key", {"bogus": []}),
            ("none", None),
        ):
            bad[label] = await probe(sb.update_network(upd))
        out["validation"] = bad
        out["after_bad_updates"] = await _both(sb)
    finally:
        await AsyncSandbox.kill(sb.sandbox_id)
    return out
