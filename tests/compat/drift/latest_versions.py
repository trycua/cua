#!/usr/bin/env python3
"""Latest released SDK versions (PyPI and npm) next to the versions Cua claims
(tests/compat/SDK_VERSIONS.json). Prints a table and writes
latest-versions.json; exits 0 (the surface and conformance jobs decide
pass/fail, this is the context for a readable report)."""

from __future__ import annotations

import json
import sys
import urllib.request
from pathlib import Path

V = json.loads((Path(__file__).resolve().parent.parent / "SDK_VERSIONS.json").read_text())


def get(url):
    with urllib.request.urlopen(url, timeout=30) as r:
        return json.load(r)


rows = []
for prov, langs in V.items():
    if prov.startswith("_"):
        continue
    for lang, spec in langs.items():
        pkg = spec["package"]
        try:
            if lang.startswith("py"):
                latest = get(f"https://pypi.org/pypi/{pkg}/json")["info"]["version"]
            else:
                latest = get(f"https://registry.npmjs.org/{pkg.replace('/', '%2F')}/latest")[
                    "version"
                ]
        except Exception as e:  # noqa: BLE001
            latest = f"error: {e}"
        rows.append(
            {
                "provider": prov,
                "lang": lang,
                "package": pkg,
                "pinned": spec["pinned"],
                "claimed": spec["claimed"],
                "latest": latest,
                "newer_than_claimed": latest not in spec["claimed"] and spec["pinned"] is not None,
            }
        )
for r in rows:
    flag = "  <- newer than claimed" if r["newer_than_claimed"] else ""
    print(
        f"{r['provider']:8} {r['lang']:20} {r['package']:24} pinned={r['pinned']} latest={r['latest']}{flag}"
    )
Path(__import__("os").environ.get("RUNNER_TEMP", ".")).joinpath("latest-versions.json").write_text(
    json.dumps(rows, indent=1)
)
