"""Check local Markdown links in the public benchmark import."""

from __future__ import annotations

import re
from pathlib import Path


BENCHMARK_ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
RUNTIME_ROOT = REPOSITORY_ROOT / "libs" / "cua-bench-runtime"
MARKDOWN_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")


def test_local_markdown_links_resolve() -> None:
    missing: list[str] = []
    for root in (BENCHMARK_ROOT, RUNTIME_ROOT):
        for document in sorted(root.rglob("*.md")):
            for line_number, line in enumerate(
                document.read_text(encoding="utf-8").splitlines(), start=1
            ):
                for raw_target in MARKDOWN_LINK.findall(line):
                    target = raw_target.strip().split(maxsplit=1)[0].strip("<>")
                    if not target or target.startswith(("#", "http://", "https://", "mailto:")):
                        continue
                    path_target = target.split("#", maxsplit=1)[0]
                    if path_target and not (document.parent / path_target).resolve().exists():
                        missing.append(
                            f"{document.relative_to(REPOSITORY_ROOT)}:{line_number}: {target}"
                        )

    assert not missing, "Missing local Markdown targets:\n" + "\n".join(missing)
