#!/usr/bin/env python3
"""The gRPC calls the JavaScript and Go SDKs make, against what the gateway serves.

    sdk_calls.py --js DIST_INDEX_JS --go GO_MODULE_DIR [--served served-rpcs.txt] > table.md
    sdk_calls.py ... --check     fail when an RPC an SDK calls is neither served nor a documented native error

The JavaScript library (`modal` on npm, Apache-2.0) calls `client.cpClient.<rpc>(`; the Go
library calls `<client>.cpClient.<Rpc>(`. Only the names are read.
"""

from __future__ import annotations

import argparse
import glob
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def js_calls(path: str) -> set[str]:
    text = open(path, encoding="utf8").read()
    return {m[0].upper() + m[1:] for m in re.findall(r"\.cpClient\.([A-Za-z0-9]+)", text)}


def go_calls(root: str) -> set[str]:
    out: set[str] = set()
    for f in glob.glob(os.path.join(root, "*.go")):
        if f.endswith("_test.go"):
            continue
        out.update(re.findall(r"cpClient\.([A-Z][A-Za-z0-9]*)\(", open(f, encoding="utf8").read()))
    return out


# Calls the gateway answers with Modal's UnimplementedError and a reason (metrics
# series, Proxies, sidecar containers); anything else an SDK calls must be served.
NATIVE_ERRORS = {
    "FunctionGetMetrics",
    "FunctionGetTimeRangeStats",
    "MetricsGetInfo",
    "ProxyGet",
    "SandboxContainerCreateV2",
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--js", required=True)
    ap.add_argument("--go", required=True)
    ap.add_argument(
        "--served",
        default=os.path.join(
            HERE, "../../../../../libs/cua/crates/cua-compat/modal-proto/served-rpcs.txt"
        ),
    )
    ap.add_argument(
        "--surface",
        default=os.path.join(
            HERE, "../../../../../libs/cua/crates/cua-compat/modal-proto/surface-1.6.2.json"
        ),
    )
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    import json

    served = {lnk.strip().split("/")[-1] for lnk in open(a.served) if lnk.strip()}
    known = {m for ms in json.load(open(a.surface))["services"].values() for m in ms}
    js, go = js_calls(a.js) & known, go_calls(a.go) & known
    names = sorted(js | go)
    rows = []
    unserved = []
    for n in names:
        s = n in served
        if not s:
            unserved.append(n)
        rows.append(
            (
                n,
                "yes" if n in js else "",
                "yes" if n in go else "",
                "served" if s else "native error",
            )
        )
    print("| RPC | JavaScript | Go | Gateway |\n|---|---|---|---|")
    for r in rows:
        print("| `%s` | %s | %s | %s |" % r)
    if a.check:
        bad = [n for n in unserved if n not in NATIVE_ERRORS]
        if bad:
            print(
                f"FAIL: SDK calls neither served nor documented as native errors: {bad}",
                file=sys.stderr,
            )
            return 1
    print(
        f"\n{len(names)} RPCs: {len(names) - len(unserved)} served, {len(unserved)} answered with Modal's UnimplementedError and the reason: {', '.join(unserved)}."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
