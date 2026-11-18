#!/usr/bin/env python3
"""config lane: every docs block tagged ``test="config"`` parses as its
language (JSON, JSONC, YAML or TOML), and one with ``schema="<name>"``
validates against that JSON Schema (``schemas`` in
docs/code-block-policy.json).

    uv run --no-project --with pyyaml==6.0.2 --with jsonschema==4.23.0 \\
      python3 tests/e2e/cua-sdk/docs/config_lane.py            # check
    ... config_lane.py --tag                                    # tag baselined blocks that parse

Tagging adds ``schema=`` where the content says what it is (an MCP client
config, a cua Image resource). With ``$CUA_E2E_RESULTS`` set, results go to
docs-blocks.jsonl (lane ``config``).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tomllib
from pathlib import Path

import extract

LANE = "config"
LANGS = {"json", "yaml", "toml"}


def strip_jsonc(text: str) -> str:
    """Removes // and /* */ comments outside strings, and trailing commas."""
    out, i, n = [], 0, len(text)
    in_str = False
    while i < n:
        c = text[i]
        if in_str:
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 2
                continue
            if c == '"':
                in_str = False
        elif c == '"':
            in_str = True
            out.append(c)
        elif text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
            continue
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            i = n if end < 0 else end + 2
            continue
        else:
            out.append(c)
        i += 1
    return re.sub(r",(\s*[}\]])", r"\1", "".join(out))


def parse(code: str, lang: str, jsonc: bool = False):
    lang = extract.norm_lang(lang)
    if lang == "json":
        return json.loads(strip_jsonc(code) if jsonc else code)
    if lang == "toml":
        return tomllib.loads(code)
    if lang == "yaml":
        import yaml  # pyyaml, installed by the lane's runner

        return yaml.safe_load(code)
    raise ValueError(f"no parser for {lang}")


def detect_schema(doc) -> str | None:
    if isinstance(doc, dict):
        if "mcpServers" in doc or "servers" in doc:
            return "mcp-client-config"
        if str(doc.get("apiVersion", "")).startswith("images.cua.ai/"):
            return "cua-image"
    return None


def validate(doc, schema_name: str, policy: dict) -> list[str]:
    import jsonschema  # installed by the lane's runner

    rel = policy.get("schemas", {}).get(schema_name)
    if rel is None:
        return [f"unknown schema {schema_name!r}"]
    schema = json.loads((extract.REPO / rel).read_text(encoding="utf-8"))
    cls = jsonschema.validators.validator_for(schema)
    errs = sorted(cls(schema).iter_errors(doc), key=lambda e: list(e.path))
    return [f"{'/'.join(map(str, e.path)) or '<root>'}: {e.message}" for e in errs[:5]]


def check(block, policy: dict) -> list[str]:
    try:
        doc = parse(block.code, block.lang, jsonc=block.lang.lower() == "jsonc")
    except Exception as e:  # noqa: BLE001 - any parse error is the finding
        return [f"does not parse as {block.lang}: {e}"]
    name = block.attrs.get("schema")
    return validate(doc, name, policy) if name else []


def tag(docs: Path, baseline: set[tuple[str, str]]) -> int:
    count = 0
    for path in sorted(docs.rglob("*.mdx")):
        rel = path.relative_to(docs).as_posix()
        todo = {}
        taken = {f.block_id for f in extract.fences(path, rel) if f.block_id}
        for f in extract.fences(path, rel):
            if f.generated or f.disposition or (f.page, f.fingerprint) not in baseline:
                continue
            if extract.norm_lang(f.lang) not in LANGS:
                continue
            try:
                doc = parse(f.code, f.lang, jsonc=f.lang.lower() == "jsonc")
            except Exception:  # noqa: BLE001
                continue
            if doc is None:
                continue
            base = f"{extract.norm_lang(f.lang)}-config"
            bid, n = base, 2
            while bid in taken:
                bid, n = f"{base}-{n}", n + 1
            taken.add(bid)
            meta = f'test="{LANE}" id="{bid}"'
            schema = detect_schema(doc)
            if schema:
                meta += f' schema="{schema}"'
            todo[f.line] = meta
        if not todo:
            continue
        lines = path.read_text(encoding="utf-8").split("\n")
        for line_no, meta in todo.items():
            lines[line_no - 1] = lines[line_no - 1].rstrip() + " " + meta
            count += 1
        path.write_text("\n".join(lines), encoding="utf-8")
    return count


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--docs", type=Path, default=extract.DOCS)
    ap.add_argument("--tag", action="store_true")
    a = ap.parse_args()
    if a.tag:
        print(f"tagged {tag(a.docs, extract.load_baseline())} block(s)")
        return 0
    policy = extract.load_policy()
    rows, failures = [], []
    blocks = [b for b in extract.all_blocks(a.docs) if LANE in b.lanes]
    for b in blocks:
        problems = check(b, policy)
        rows.append(
            {
                "block_id": b.id,
                "page": b.guide,
                "line": b.line,
                "lane": LANE,
                "lang": b.lang,
                "status": "fail" if problems else "pass",
                "test": "config_lane",
                "reason": "; ".join(problems)[:400],
            }
        )
        failures += [f"{b.guide}:{b.line} ({b.id}): {p}" for p in problems]
    out = os.environ.get("CUA_E2E_RESULTS")
    if out:
        Path(out).mkdir(parents=True, exist_ok=True)
        with open(Path(out) / "docs-blocks.jsonl", "a", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    if failures:
        print("config lane:", *failures, sep="\n  ", file=sys.stderr)
        return 1
    print(f"config lane OK: {len(blocks)} block(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
