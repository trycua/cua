#!/usr/bin/env python3
"""Compat overhead guard: how long the Daytona gateway itself takes.

Over the mock Spaces (so the guest costs nothing) it times the requests a
Daytona client makes most, on one keep-alive connection, and divides each p50
by the p50 of a route that does no work (`GET /api/config`), which factors out
the machine. `baseline.json` holds the committed ratios (recorded on the
nightly runner); a ratio above 1.5x its baseline fails.

    overhead.py --env env.json [--check baseline.json | --write baseline.json] [--n 200]
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import statistics
import sys
import time
import urllib.parse

THRESHOLD = 1.5


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--env", required=True)
    ap.add_argument("--n", type=int, default=200)
    ap.add_argument("--check")
    ap.add_argument("--write")
    a = ap.parse_args()
    env = json.load(open(a.env))
    u = urllib.parse.urlparse(env["DAYTONA_API_URL"])
    conn = http.client.HTTPConnection(u.hostname, u.port, timeout=30)
    H = {"Authorization": f"Bearer {env['DAYTONA_API_KEY']}"}

    def call(method, path, body=None, headers=None, raw=False):
        h = {**H, **(headers or {})}
        if body is not None and not raw:
            body = json.dumps(body)
            h["Content-Type"] = "application/json"
        conn.request(method, u.path + path, body=body, headers=h)
        r = conn.getresponse()
        data = r.read()
        if r.status >= 300:
            raise RuntimeError(f"{method} {path}: {r.status} {data[:200]!r}")
        return data

    sb = json.loads(call("POST", "/sandbox", {"labels": {"perf": "1"}, "autoStopInterval": 0}))
    sid = sb["id"]
    tb = f"/toolbox/{sid}"
    boundary = "perfboundary"
    payload = os.urandom(4096)
    upload = (
        (
            f'--{boundary}\r\nContent-Disposition: form-data; name="files[0].path"\r\n\r\n/tmp/perf.bin\r\n'
            f'--{boundary}\r\nContent-Disposition: form-data; name="files[0].file"; filename="perf.bin"\r\n'
            f"Content-Type: application/octet-stream\r\n\r\n"
        ).encode()
        + payload
        + f"\r\n--{boundary}--\r\n".encode()
    )

    ops = {
        "noop_config": lambda: call("GET", "/config"),
        "get_sandbox": lambda: call("GET", f"/sandbox/{sid}"),
        "list_sandboxes": lambda: call("GET", "/sandbox?limit=20"),
        "set_labels": lambda: call("PUT", f"/sandbox/{sid}/labels", {"labels": {"perf": "1"}}),
        "toolbox_exec": lambda: call("POST", f"{tb}/process/execute", {"command": "echo hi"}),
        "toolbox_upload_4k": lambda: call(
            "POST",
            f"{tb}/files/bulk-upload",
            upload,
            {"Content-Type": f"multipart/form-data; boundary={boundary}"},
            raw=True,
        ),
        "toolbox_download_4k": lambda: call(
            "POST", f"{tb}/files/bulk-download", {"paths": ["/tmp/perf.bin"]}
        ),
        "toolbox_list_files": lambda: call("GET", f"{tb}/files?path=/tmp"),
        "toolbox_file_info": lambda: call("GET", f"{tb}/files/info?path=/tmp/perf.bin"),
    }
    ops["toolbox_upload_4k"]()
    p50 = {}
    for name, fn in ops.items():
        for _ in range(10):
            fn()
        ts = []
        for _ in range(a.n):
            t0 = time.perf_counter()
            fn()
            ts.append((time.perf_counter() - t0) * 1000)
        p50[name] = statistics.median(ts)
    call("DELETE", f"/sandbox/{sid}")
    ref = p50["noop_config"]
    ratios = {k: round(v / ref, 3) for k, v in p50.items() if k != "noop_config"}
    out = {
        "unit": "p50 ratio to GET /api/config",
        "n": a.n,
        "ratios": ratios,
        "noop_p50_ms": round(ref, 3),
        "p50_ms": {k: round(v, 3) for k, v in p50.items()},
    }
    print(json.dumps(out, indent=1))
    if a.write:
        json.dump({"unit": out["unit"], "n": a.n, "ratios": ratios}, open(a.write, "w"), indent=1)
        open(a.write, "a").write("\n")
        return 0
    if a.check:
        base = json.load(open(a.check))["ratios"]
        bad = {
            k: (ratios[k], base[k]) for k in base if k in ratios and ratios[k] > THRESHOLD * base[k]
        }
        missing = [k for k in base if k not in ratios]
        for k, (got, want) in bad.items():
            print(f"PERF {k}: ratio {got} > {THRESHOLD} x baseline {want}")
        for k in missing:
            print(f"PERF {k}: in the baseline, not measured")
        return 1 if bad or missing else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
