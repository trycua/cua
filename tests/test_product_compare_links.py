"""Drift guard: the product links to one cua.ai comparison page only.

The apps, the CLI, the Rust crates, the SDKs and what they ship may link to
https://cua.ai/compare/clouds (comparing the clouds you can connect, This
Mac first). Every other cua.ai compare or benchmark page
(/compare, /compare/<anything else>, /benchmarks, /benchmarks/<slug>) is
for the website and docs, never the product.

    python3 -m unittest tests.test_product_compare_links -v
"""

from __future__ import annotations

import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Product sources: the apps (SwiftUI, web UI, desktop shell), the cua CLI and
# Rust crates (their user-facing strings, cloud_connect.rs included), the
# SDKs and bindings, the bench scripts `cua bench` embeds, the in-sandbox
# server, and the Swift packages the apps build from.
PRODUCT_ROOTS = (
    "apps/",
    "libs/cua/crates/",
    "libs/cua/python/",
    "libs/cua/typescript/",
    "libs/cua/swift/",
    "libs/cua/kotlin/",
    "libs/cua/skills/",
    "libs/cua/spaces-contract/",
    "libs/cua/vz/",
    "libs/cua/bench/",
    "libs/cua-spacesd/",
    "libs/cua-driver/",
    "libs/spaces-app-swift/",
    "libs/spaces-notch-swift/",
    "libs/spaces-sdk-swift/",
    "libs/python/",
    "libs/typescript/",
)

ALLOWED = "https://cua.ai/compare/clouds"

# Any cua.ai (or www.cua.ai) URL whose path starts with /compare or
# /benchmarks, with or without a scheme. Group 1 is the path; a query or
# fragment after it is ignored.
LINK = re.compile(
    r"(?:https?:)?(?://)?(?:www\.)?cua\.ai(/(?:compare|benchmarks)(?![\w-])[\w\-./%]*)",
    re.IGNORECASE,
)


def violations_in(text: str) -> list[str]:
    """The forbidden compare/benchmark links in `text`."""
    bad = []
    for m in LINK.finditer(text):
        path = m.group(1).rstrip("./")
        url = m.group(0)
        if path.lower() == "/compare/clouds" and (
            url.lower().startswith("https://") or not url.lower().startswith("http:")
        ):
            continue
        bad.append(url)
    return bad


def product_files() -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", *PRODUCT_ROOTS],
        cwd=ROOT,
        check=True,
        capture_output=True,
    ).stdout.decode()
    return [p for p in out.split("\0") if p]


class CompareLinkGuardTests(unittest.TestCase):
    def test_matcher(self) -> None:
        self.assertEqual(violations_in(f'let url = "{ALLOWED}"'), [])
        self.assertEqual(violations_in(f"{ALLOWED}?c=this-mac,hetzner#top"), [])
        self.assertEqual(violations_in("cua.ai/compare/clouds"), [])
        for bad in (
            "https://cua.ai/compare/sandboxes",
            "https://www.cua.ai/compare/sandboxes",
            "https://cua.ai/compare",
            "https://cua.ai/compare/",
            "https://cua.ai/compare/spaces-vs-x",
            "https://cua.ai/compare/clouds-and-more",
            "https://cua.ai/benchmarks",
            "https://cua.ai/benchmarks/fork-fanout",
            "https://cua.ai/benchmarks/fork-fanout.csv",
            "http://cua.ai/compare/clouds",
            "HTTPS://CUA.AI/Compare/Sandboxes",
        ):
            self.assertEqual(len(violations_in(f'href="{bad}"')), 1, bad)
        # Other cua.ai pages and other sites are not this guard's business.
        self.assertEqual(
            violations_in(
                "https://cua.ai/docs/compare-modes https://cua.ai/pricing https://example.com/compare"
            ),
            [],
        )

    def test_product_links_only_to_compare_clouds(self) -> None:
        files = product_files()
        self.assertGreater(len(files), 1000, "the product roots moved: update PRODUCT_ROOTS")
        found = []
        for rel in files:
            path = ROOT / rel
            if not path.is_file():
                continue
            try:
                text = path.read_text()
            except (UnicodeDecodeError, OSError):
                continue
            for url in violations_in(text):
                found.append(f"{rel}: {url}")
        self.assertEqual(
            found,
            [],
            f"The product may link only to {ALLOWED} among the cua.ai compare and benchmark pages:\n"
            + "\n".join(found),
        )

    def test_every_product_root_exists(self) -> None:
        missing = [r for r in PRODUCT_ROOTS if not (ROOT / r).is_dir()]
        self.assertEqual(missing, [], "a product root moved: update PRODUCT_ROOTS")


if __name__ == "__main__":
    unittest.main()
