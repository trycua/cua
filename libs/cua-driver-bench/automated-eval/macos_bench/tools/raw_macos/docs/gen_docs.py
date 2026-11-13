#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Generate the macOS API docs index the `pi-raw-macos-docs` arm's lookup tool searches.

Run on the macOS machine whose APIs are documented (the bench guest), with the Command Line Tools installed:

    /usr/bin/python3 gen_docs.py OUT_ROOT

It writes OUT_ROOT/<build>/index.jsonl and OUT_ROOT/<build>/manifest.json, where <build> is the running macOS build
(ProductBuildVersion, for example 25F84). The same build and SDK give byte-identical files: nothing time- or
path-dependent is written, and entries are sorted.

Sources:
- Public frameworks: `clang -extract-api` over every header of the framework in the active SDK, which emits a symbol
  graph with each symbol's declaration and its header doc comment. Objective-C methods also get the name pyobjc calls
  them by (the selector with every ':' replaced by '_').
- Private frameworks and undocumented exports: `dyld_info -exports` on the framework binaries as the running system
  loads them (from the dyld shared cache). These carry a name only: no header declares them. For a public framework's
  binary, only the exports its headers do not declare are kept. Swift-mangled names are skipped.
"""

from __future__ import annotations

import json
import os
import plistlib
import re
import subprocess
import sys
import tempfile
from pathlib import Path

# Public frameworks: (name in the index, framework directory relative to the SDK, extra -F directories relative to the SDK)
FW = "System/Library/Frameworks"
PUBLIC = (
    (
        "HIServices (ApplicationServices: Accessibility, AX)",
        f"{FW}/ApplicationServices.framework/Frameworks/HIServices.framework",
        (f"{FW}/ApplicationServices.framework/Frameworks",),
    ),
    ("CoreGraphics (Quartz)", f"{FW}/CoreGraphics.framework", ()),
    ("ImageIO", f"{FW}/ImageIO.framework", ()),
    ("AppKit", f"{FW}/AppKit.framework", ()),
    ("Foundation", f"{FW}/Foundation.framework", ()),
    ("CoreFoundation", f"{FW}/CoreFoundation.framework", ()),
    ("ScreenCaptureKit", f"{FW}/ScreenCaptureKit.framework", ()),
    ("UniformTypeIdentifiers", f"{FW}/UniformTypeIdentifiers.framework", ()),
    (
        "LaunchServices (CoreServices)",
        f"{FW}/CoreServices.framework/Frameworks/LaunchServices.framework",
        (f"{FW}/CoreServices.framework/Frameworks",),
    ),
)
# Binaries whose exports are listed: (name in the index, path on the running system, public: only header-less exports)
SL = "/System/Library"
BINARIES = (
    ("SkyLight (private)", f"{SL}/PrivateFrameworks/SkyLight.framework/SkyLight", False),
    (
        "HIServices (ApplicationServices: Accessibility, AX)",
        f"{SL}/Frameworks/ApplicationServices.framework/Versions/A/Frameworks/HIServices.framework/Versions/A/HIServices",
        True,
    ),
    ("CoreGraphics (Quartz)", f"{SL}/Frameworks/CoreGraphics.framework/CoreGraphics", True),
    (
        "LaunchServices (CoreServices)",
        f"{SL}/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/LaunchServices",
        True,
    ),
    ("AppKit", f"{SL}/Frameworks/AppKit.framework/AppKit", True),
)
DOC_TAG = re.compile(
    r"^\s*@(function|method|abstract|discussion|const|constant|typedef|enum|struct|define|group|header|class|protocol|category)\b\s*"
)
PYOBJC_KINDS = {"objective-c.method", "objective-c.type.method"}


def run(argv: list[str]) -> str:
    return subprocess.run(argv, check=True, capture_output=True, text=True).stdout


def os_build() -> str:
    with open("/System/Library/CoreServices/SystemVersion.plist", "rb") as handle:
        info = plistlib.load(handle)
    return f"{info['ProductBuildVersion']}"


def sdk_info() -> tuple[Path, str]:
    sdk = Path(run(["xcrun", "--show-sdk-path"]).strip()).resolve()
    settings = json.loads((sdk / "SDKSettings.json").read_text("utf-8"))
    return sdk, str(settings.get("CanonicalName") or settings.get("Version"))


def clean_doc(lines: list[dict]) -> str:
    out = []
    for line in lines:
        text = DOC_TAG.sub("", line.get("text", "")).strip()
        text = re.sub(r"</?(code|b|i|p|br)\s*/?>", "", text)
        text = re.sub(r"@link\s+(\S+)\s+.*?@/link", r"\1", text)
        if text:
            out.append(text)
    return " ".join(out)


class Headers:
    """Header lines by path, read once: many headers document a declaration with a plain comment right above it (or
    on its own line), which the symbol graph does not carry as a doc comment."""

    def __init__(self) -> None:
        self.files: dict[str, list[str]] = {}

    def lines(self, uri: str) -> list[str]:
        if uri not in self.files:
            path = uri.removeprefix("file://")
            try:
                self.files[uri] = Path(path).read_text("utf-8", "replace").splitlines()
            except OSError:
                self.files[uri] = []
        return self.files[uri]

    def comment_above(self, uri: str, line: int) -> str:
        """The comment block that ends on the line above ``line`` (0-based), at most one blank line away."""
        lines = self.lines(uri)
        i = line - 1
        while i >= 0 and i >= line - 2 and not lines[i].strip():
            i -= 1
        if i < 0:
            return ""
        block: list[str] = []
        if lines[i].rstrip().endswith("*/"):
            while i >= 0 and len(block) < 60:
                block.insert(0, lines[i])
                if "/*" in lines[i]:
                    break
                i -= 1
            else:
                return ""
        else:
            while i >= 0 and lines[i].lstrip().startswith("//") and len(block) < 60:
                block.insert(0, lines[i])
                i -= 1
        text = " ".join(
            re.sub(r"^\s*(/\*+!?|//+!?|\*+/?)\s?", "", s).replace("*/", "").strip() for s in block
        )
        return re.sub(r"\s+", " ", text).strip()

    def case_value(self, uri: str, line: int, name: str) -> str:
        lines = self.lines(uri)
        if 0 <= line < len(lines):
            m = re.match(
                r"\s*" + re.escape(name) + r"\b[^=,]*=\s*([^,/]+?)\s*(,|/[/*]|$)", lines[line]
            )
            if m:
                return m.group(1)
        return ""

    def macro_value(self, uri: str, line: int, name: str) -> str:
        lines = self.lines(uri)
        if 0 <= line < len(lines):
            m = re.match(
                r"\s*#\s*define\s+" + re.escape(name) + r"\s+(.+?)\s*(/[/*].*)?$", lines[line]
            )
            if m:
                return m.group(1)
        return ""


def declaration(fragments: list[dict]) -> str:
    return re.sub(r"\s+", " ", "".join(f.get("spelling", "") for f in fragments)).strip()


def extract(
    sdk: Path,
    framework: str,
    fw_dir: str,
    extra_f: tuple[str, ...],
    scratch: Path,
    headers_cache: Headers,
) -> list[dict]:
    headers = sorted((sdk / fw_dir / "Headers").glob("*.h"))
    if not headers:
        raise SystemExit(f"no headers for {framework} in {sdk / fw_dir}")
    out = scratch / "graph.json"
    argv = [
        "xcrun",
        "clang",
        "-extract-api",
        "--product-name=" + framework.split(" ")[0],
        "-x",
        "objective-c-header",
        "-isysroot",
        str(sdk),
    ]
    for f in extra_f:
        argv += ["-F", str(sdk / f)]
    run(argv + [str(h) for h in headers] + ["-o", str(out)])
    graph = json.loads(out.read_text("utf-8"))
    entries = []
    for sym in graph["symbols"]:
        if sym.get("accessLevel") not in (None, "public"):
            continue
        kind = sym["kind"]["identifier"]
        path = sym.get("pathComponents") or [sym["names"]["title"]]
        name = ".".join(path)
        loc = sym.get("location") or {}
        uri, line = loc.get("uri", ""), (loc.get("position") or {}).get("line", -1)
        decl = declaration(sym.get("declarationFragments") or [])
        doc = clean_doc((sym.get("docComment") or {}).get("lines") or [])
        if not doc and uri and line >= 0:
            doc = headers_cache.comment_above(uri, line)
        if kind == "objective-c.enum.case" and uri:
            value = headers_cache.case_value(uri, line, path[-1])
            if value:
                decl = f"{path[-1]} = {value}"
        if kind == "objective-c.macro" and uri:
            value = headers_cache.macro_value(uri, line, path[-1])
            if value:
                decl = f"#define {path[-1]} {value}"
        entry = {
            "framework": framework,
            "kind": kind.replace("objective-c.", ""),
            "name": name,
            "decl": decl,
            "doc": doc,
        }
        if kind in PYOBJC_KINDS:
            entry["pyobjc"] = path[-1].replace(":", "_")
        avail = [a for a in sym.get("availability") or [] if a.get("domain") == "macos"]
        if avail and avail[0].get("deprecated"):
            d = avail[0]["deprecated"]
            entry["deprecated"] = f"{d.get('major')}.{d.get('minor', 0)}"
        entries.append(entry)
    return entries


def exports(binary: str) -> list[tuple[str, str]]:
    """(symbol without its leading underscore, re-export note) for each export of ``binary``."""
    out = []
    for line in run(["dyld_info", "-exports", binary]).splitlines():
        line = line.strip()
        m = re.match(r"^(?:0x[0-9A-Fa-f]+|\[re-export\])\s+(_\S+)(?:\s+\((.*)\))?$", line)
        if not m:
            continue
        name, note = m.group(1)[1:], m.group(2) or ""
        if name.startswith(("$s", "_$s", "OBJC_", ".objc", "_OBJC_")):
            continue
        out.append((name, note))
    return out


def generate(out_root: Path) -> Path:
    build = os_build()
    sdk, sdk_name = sdk_info()
    entries: list[dict] = []
    counts: dict[str, int] = {}
    declared: set[str] = set()
    cache = Headers()
    with tempfile.TemporaryDirectory() as tmp:
        for framework, fw_dir, extra_f in PUBLIC:
            found = extract(sdk, framework, fw_dir, extra_f, Path(tmp), cache)
            counts[framework] = len(found)
            entries += found
            declared |= {e["name"].split(".")[-1] for e in found}
    for framework, binary, public in BINARIES:
        n = 0
        for name, note in exports(binary):
            if public and name in declared:
                continue
            entry = {
                "framework": framework,
                "kind": "export",
                "name": name,
                "decl": "",
                "doc": "Exported by the framework binary; no public header declares it, so the signature is not "
                "published." + (f" {note}." if note else ""),
            }
            entries.append(entry)
            n += 1
        counts[framework + " exports"] = n
    # one entry per (framework, name, kind, decl); sorted, so the output does not depend on traversal order
    unique = {(e["framework"], e["name"], e["kind"], e["decl"]): e for e in entries}
    lines = [json.dumps(unique[k], sort_keys=True, ensure_ascii=False) for k in sorted(unique)]
    dest = out_root / build
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "index.jsonl").write_text("\n".join(lines) + "\n", "utf-8")
    manifest = {
        "build": build,
        "sdk": sdk_name,
        "clang": run(["xcrun", "clang", "--version"]).splitlines()[0],
        "entries": len(lines),
        "counts": dict(sorted(counts.items())),
    }
    (dest / "manifest.json").write_text(
        json.dumps(manifest, indent=1, sort_keys=True) + "\n", "utf-8"
    )
    return dest


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    print(generate(Path(os.path.expanduser(sys.argv[1]))))
