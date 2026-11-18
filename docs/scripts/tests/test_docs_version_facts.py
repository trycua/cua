"""Keep release versions in the maintained SDK documentation installable."""

from __future__ import annotations

import json
from pathlib import Path
import re
import tomllib


REPOSITORY = Path(__file__).resolve().parents[3]
DOCS = REPOSITORY / "docs" / "content" / "docs"


def load_project(path: str) -> dict:
    with (REPOSITORY / path).open("rb") as stream:
        return tomllib.load(stream)["project"]


# The cua SDK (Rust core, Python `cua` wheel, `@trycua/cua` and the Rust `cua`
# CLI) releases in lockstep from libs/cua/VERSION.
CUA_VERSION = (REPOSITORY / "libs/cua/VERSION").read_text().strip()
CUA_PYTHON_PROJECT = load_project("libs/cua/python/pyproject.toml")
TYPESCRIPT_CUA = json.loads((REPOSITORY / "libs/cua/typescript/package.json").read_text())

TYPESCRIPT_CUA_VERSION = TYPESCRIPT_CUA["version"]


# Hand-written pages that tell readers how to install cua-sandbox. The
# manifest version reaches the docs in the release PR, before PyPI publishes
# it, so an exact pin on these pages can name a version users cannot install
# (#7899). They install without a pin instead.
INSTALL_PAGES = (
    "cua-sdk/concepts/how-sandboxes-work.mdx",
    "cua-sdk/guides/agent-frameworks.mdx",
    "cua-sdk/guides/desktop.mdx",
)

# Generated reference pages name no release version at all (a stamp goes stale
# when a PR regenerates a page before a release and merges after it). The
# generator test scripts/docs-generators/docs-version-stamp.test.ts enforces it.
GENERATED_PAGES = (
    "cua-sdk/reference/index.mdx",
    "cua-sdk/reference/python/index.mdx",
    "cua-sdk/reference/runtime-support.mdx",
    "cua-sdk/reference/typescript/index.mdx",
)


def test_cua_sdk_manifests_release_in_lockstep() -> None:
    assert CUA_PYTHON_PROJECT["version"] == CUA_VERSION
    assert TYPESCRIPT_CUA_VERSION == CUA_VERSION


def test_install_pages_do_not_pin_the_sandbox_version() -> None:
    pin = re.compile(r"cua-sandbox(?:\[[a-z,]+\])?(?:`|\*\*)?[ =@]+\d+\.\d+\.\d+")
    for relative_path in INSTALL_PAGES:
        text = (DOCS / relative_path).read_text()
        assert "cua-sandbox" in text, f"{relative_path} no longer names cua-sandbox"
        match = pin.search(text)
        assert match is None, f"{relative_path} pins {match.group(0)!r}"


def test_owned_pages_drop_superseded_maintained_versions() -> None:
    stale_facts = (
        re.compile(r"cua-sandbox(?:`|\*\*)?[ =@]+0\.4\.3\b"),
        re.compile(r"cua-sandbox(?:`|\*\*)?[ =@]+0\.7\.0\b"),
        re.compile(r"cua-cli(?:`|\*\*)?[ =@]+0\.1\.14\b"),
    )

    for relative_path in (*INSTALL_PAGES, *GENERATED_PAGES):
        text = (DOCS / relative_path).read_text()
        for pattern in stale_facts:
            assert not pattern.search(text), f"{relative_path} contains {pattern.pattern!r}"

