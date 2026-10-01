#!/usr/bin/env python3
"""swift and kotlin lanes: every docs block tagged ``test="swift"`` or
``test="kotlin"`` compiles against the cua SDK in this repository.

Compile-only by default (nothing runs, no library is linked): each block
becomes one library target of a generated package, its statements wrapped
in a function so top-level snippets compile, its ``import`` lines hoisted.
Hidden preludes (``prelude="a,b"``) come from ``preludes/<name>.swift`` or
``preludes/<name>.kt`` and are prepended inside the same wrapper.

    compile_lane.py --lane swift      # SwiftPM, depends on libs/cua/swift by path
    compile_lane.py --lane kotlin     # Gradle, sources compiled with libs/cua/kotlin

Results go to ``$CUA_E2E_RESULTS/docs-blocks.jsonl``. Stdlib only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import extract

LANGS = {"swift": ("swift",), "kotlin": ("kotlin", "kt")}
PRELUDES = Path(__file__).resolve().parent / "preludes"
SWIFT_PKG = extract.REPO / "libs" / "cua" / "swift"
KOTLIN_PKG = extract.REPO / "libs" / "cua" / "kotlin"
IMPORT = {
    "swift": re.compile(r"^\s*(@testable\s+)?import\s+\w"),
    "kotlin": re.compile(r"^\s*import\s+[\w.*]+\s*$"),
}


def target_name(block_id: str) -> str:
    """A Swift/Kotlin identifier from ``<page>#<id>``."""
    words = re.split(r"[^A-Za-z0-9]+", block_id)
    return "Docs" + "".join(w[:1].upper() + w[1:] for w in words if w)


def prelude_code(block: extract.Block, ext: str) -> str:
    parts = []
    for name in [p.strip() for p in block.prelude.split(",") if p.strip()]:
        f = PRELUDES / f"{name}.{ext}"
        if f.exists():
            parts.append(f.read_text(encoding="utf-8"))
    return "\n".join(parts)


def wrap(code: str, lane: str, name: str, prelude: str = "") -> str:
    """Hoists imports and wraps the statements in an async throwing function
    (Swift) or a suspend function (Kotlin), so snippets written as top-level
    script code compile as library code."""
    imports, body = [], []
    for line in (prelude + "\n" + code).splitlines():
        (imports if IMPORT[lane].match(line) else body).append(line)
    seen: list[str] = []
    for line in imports:
        if line.strip() not in seen:
            seen.append(line.strip())
    indented = "\n".join(("    " + ln) if ln.strip() else "" for ln in body)
    if lane == "swift":
        return "\n".join(
            [*seen, "", f"func {name[:1].lower() + name[1:]}() async throws {{", indented, "}", ""]
        )
    return "\n".join(
        [
            f"package docs.{name.lower()}",
            "",
            *seen,
            "",
            f"suspend fun {name[:1].lower() + name[1:]}() {{",
            indented,
            "}",
            "",
        ]
    )


def swift_package(root: Path, blocks: list[extract.Block]) -> list[str]:
    names = [target_name(b.id) for b in blocks]
    root.mkdir(parents=True, exist_ok=True)
    targets = ",\n".join(
        f'        .target(name: "{n}", dependencies: [.product(name: "Cua", package: "swift")], '
        f'path: "Sources/{n}")'
        for n in names
    )
    (root / "Package.swift").write_text(
        "// swift-tools-version: 5.9\n"
        "import PackageDescription\n\n"
        "let package = Package(\n"
        '    name: "CuaDocsBlocks",\n'
        "    platforms: [.macOS(.v13), .iOS(.v16)],\n"
        f'    dependencies: [.package(path: "{SWIFT_PKG}")],\n'
        f"    targets: [\n{targets}\n    ]\n)\n",
        encoding="utf-8",
    )
    for b, n in zip(blocks, names):
        src = root / "Sources" / n
        src.mkdir(parents=True, exist_ok=True)
        (src / "Block.swift").write_text(
            wrap(b.code, "swift", n, prelude_code(b, "swift")), encoding="utf-8"
        )
    return names


def kotlin_project(root: Path, blocks: list[extract.Block]) -> list[str]:
    names = [target_name(b.id) for b in blocks]
    root.mkdir(parents=True, exist_ok=True)
    (root / "settings.gradle.kts").write_text(
        'rootProject.name = "cua-docs-blocks"\n', encoding="utf-8"
    )
    (root / "build.gradle.kts").write_text(
        'plugins { kotlin("jvm") version "2.0.21" }\n'
        "repositories { mavenCentral() }\n"
        "dependencies {\n"
        '    implementation("net.java.dev.jna:jna:5.14.0")\n'
        '    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")\n'
        "}\n"
        "kotlin { jvmToolchain(17) }\n"
        'sourceSets { main { kotlin.srcDir("'
        + str(KOTLIN_PKG / "src" / "main" / "kotlin")
        + '") } }\n',
        encoding="utf-8",
    )
    src = root / "src" / "main" / "kotlin"
    src.mkdir(parents=True, exist_ok=True)
    for b, n in zip(blocks, names):
        (src / f"{n}.kt").write_text(
            wrap(b.code, "kotlin", n, prelude_code(b, "kt")), encoding="utf-8"
        )
    return names


def attribute(output: str, names: list[str], blocks: list[extract.Block]) -> dict[str, str]:
    """Compiler errors by block (errors name the generated source path)."""
    errs: dict[str, list[str]] = {}
    for line in output.splitlines():
        if "error" not in line and not line.startswith("e: "):  # kotlinc: `e: file:///...`
            continue
        for n, b in zip(names, blocks):
            if f"/{n}/" in line or f"/{n}.kt" in line:
                errs.setdefault(b.id, []).append(line.strip())
    return {k: "\n".join(v[:5]) for k, v in errs.items()}


def build(lane: str, blocks: list[extract.Block], work: Path) -> tuple[int, str, list[str]]:
    if lane == "swift":
        names = swift_package(work, blocks)
        cmd = ["swift", "build", "--package-path", str(work)]
        for n in names:
            cmd += ["--target", n]
    else:
        names = kotlin_project(work, blocks)
        gradle = shutil.which("gradle")
        if not gradle:
            raise SystemExit("gradle is required for the kotlin lane")
        cmd = [gradle, "-q", "--no-daemon", "-p", str(work), "compileKotlin"]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=3600)
    return out.returncode, out.stdout + out.stderr, names


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--lane", required=True, choices=sorted(LANGS))
    ap.add_argument("--docs", type=Path, default=extract.DOCS)
    ap.add_argument("--keep", type=Path, help="generate the project here and keep it")
    a = ap.parse_args()
    blocks = [
        b
        for b in extract.all_blocks(a.docs)
        if a.lane in b.lanes and extract.norm_lang(b.lang) in LANGS[a.lane]
    ]
    if not blocks:
        print(f'no block is tagged test="{a.lane}"')
        return 0
    work = a.keep or Path(tempfile.mkdtemp(prefix=f"cua-docs-{a.lane}-"))
    work.mkdir(parents=True, exist_ok=True)
    code, output, names = build(a.lane, blocks, work)
    errors = attribute(output, names, blocks) if code else {}
    rows = []
    for b in blocks:
        failed = code != 0 and (b.id in errors or not errors)
        rows.append(
            {
                "block_id": b.id,
                "page": b.guide,
                "line": b.line,
                "lane": a.lane,
                "lang": b.lang,
                "status": "fail" if failed else "pass",
                "test": f"compile_lane[{a.lane}]",
                "reason": (errors.get(b.id) or output[-400:])[:400] if failed else "",
            }
        )
    out_dir = os.environ.get("CUA_E2E_RESULTS")
    if out_dir:
        Path(out_dir).mkdir(parents=True, exist_ok=True)
        with open(Path(out_dir) / "docs-blocks.jsonl", "a", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    if not a.keep:
        shutil.rmtree(work, ignore_errors=True)
    if code:
        print(output[-4000:], file=sys.stderr)
        print(
            f"{a.lane} lane: {sum(r['status'] == 'fail' for r in rows)} block(s) failed",
            file=sys.stderr,
        )
        return 1
    print(f"{a.lane} lane OK: {len(blocks)} block(s) compile")
    return 0


if __name__ == "__main__":
    sys.exit(main())
