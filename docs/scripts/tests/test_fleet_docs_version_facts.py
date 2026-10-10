"""Keep maintained Fleet and Sandbox documentation versions aligned with manifests."""

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


SANDBOX_PROJECT = load_project("libs/python/cua-sandbox/pyproject.toml")
# The cua SDK (Rust core, Python `cua` wheel, `@trycua/cua` and the Rust `cua`
# CLI) releases in lockstep from libs/cua/VERSION.
CUA_VERSION = (REPOSITORY / "libs/cua/VERSION").read_text().strip()
CUA_PYTHON_PROJECT = load_project("libs/cua/python/pyproject.toml")
TYPESCRIPT_CUA = json.loads((REPOSITORY / "libs/cua/typescript/package.json").read_text())
TYPESCRIPT_FLEET = json.loads(
    (REPOSITORY / "libs/typescript/fleet/package.json").read_text()
)

SANDBOX_VERSION = SANDBOX_PROJECT["version"]
TYPESCRIPT_CUA_VERSION = TYPESCRIPT_CUA["version"]
TYPESCRIPT_FLEET_VERSION = TYPESCRIPT_FLEET["version"]
PYTHON_FLEET_VERSION = next(
    dependency.removeprefix("cua-fleet==")
    for dependency in SANDBOX_PROJECT["dependencies"]
    if dependency.startswith("cua-fleet==")
)


EXPECTED_FACTS = {
    "cua-sdk/concepts/how-sandboxes-work.mdx": (f"`cua-sandbox` {SANDBOX_VERSION}",),
    "cua-sdk/guides/agent-frameworks.mdx": (f"cua-sandbox=={SANDBOX_VERSION}",),
    "cua-sdk/guides/desktop.mdx": (f"'cua-sandbox[driver]=={SANDBOX_VERSION}'",),
    "fleets/guides/images.mdx": (f"`cua` {CUA_VERSION}",),
    "cua-sdk/reference/index.mdx": (
        f"`pip install cua` ({CUA_VERSION}",
        f"`npm install @trycua/cua` ({TYPESCRIPT_CUA_VERSION}",
    ),
    "cua-sdk/reference/python/index.mdx": (
        f"`cua-sandbox` {SANDBOX_VERSION}",
        f"`cua` {CUA_VERSION}",
    ),
    "cua-sdk/reference/runtime-support.mdx": (
        f"`cua-sandbox` **{SANDBOX_VERSION}**",
        f"`cua` **{CUA_VERSION}**",
        f"`cua-fleet` {PYTHON_FLEET_VERSION}",
        f"`@trycua/fleet` {TYPESCRIPT_FLEET_VERSION}",
    ),
    "cua-sdk/reference/typescript/index.mdx": (
        f"`npm install @trycua/cua` ({TYPESCRIPT_CUA_VERSION})",
    ),
    "fleets/quickstart.mdx": (
        f"cua-sandbox=={SANDBOX_VERSION}",
        f"@trycua/cua@{TYPESCRIPT_CUA_VERSION}",
    ),
}


def test_cua_sdk_manifests_release_in_lockstep() -> None:
    assert CUA_PYTHON_PROJECT["version"] == CUA_VERSION
    assert TYPESCRIPT_CUA_VERSION == CUA_VERSION


def test_maintained_version_facts_match_package_manifests() -> None:
    for relative_path, expected_facts in EXPECTED_FACTS.items():
        text = (DOCS / relative_path).read_text()
        for fact in expected_facts:
            assert fact in text, f"{relative_path} must contain manifest fact {fact!r}"


def test_owned_pages_drop_superseded_maintained_versions() -> None:
    stale_facts = (
        re.compile(r"cua-sandbox(?:`|\*\*)?[ =@]+0\.4\.3\b"),
        re.compile(r"cua-sandbox(?:`|\*\*)?[ =@]+0\.7\.0\b"),
        re.compile(r"cua-fleet(?:`|\*\*)?[ =@]+0\.1\.14\b"),
        re.compile(r"@trycua/fleet(?:`|\*\*)?[ =@]+0\.1\.1\b"),
        re.compile(r"cua-cli(?:`|\*\*)?[ =@]+0\.1\.14\b"),
    )

    for relative_path in EXPECTED_FACTS:
        text = (DOCS / relative_path).read_text()
        for pattern in stale_facts:
            assert not pattern.search(text), f"{relative_path} contains {pattern.pattern!r}"

