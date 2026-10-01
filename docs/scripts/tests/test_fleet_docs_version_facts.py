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
CLI_PROJECT = load_project("libs/python/cua-cli/pyproject.toml")
CUA_PROJECT = load_project("libs/python/cua/pyproject.toml")
TYPESCRIPT_FLEET = json.loads(
    (REPOSITORY / "libs/typescript/fleet/package.json").read_text()
)

SANDBOX_VERSION = SANDBOX_PROJECT["version"]
CLI_VERSION = CLI_PROJECT["version"]
CUA_VERSION = CUA_PROJECT["version"]
TYPESCRIPT_FLEET_VERSION = TYPESCRIPT_FLEET["version"]
PYTHON_FLEET_VERSION = next(
    dependency.removeprefix("cua-fleet==")
    for dependency in SANDBOX_PROJECT["dependencies"]
    if dependency.startswith("cua-fleet==")
)


EXPECTED_FACTS = {
    "concepts/how-fleet-images-work.mdx": (f"`cua-sandbox` {SANDBOX_VERSION}",),
    "concepts/how-sandboxes-work.mdx": (f"`cua-sandbox` {SANDBOX_VERSION}",),
    "concepts/sandbox-lifecycle.mdx": (f"`cua-sandbox` {SANDBOX_VERSION}",),
    "how-to-guides/sandbox/choose-an-image.mdx": (
        f"`cua-sandbox` {SANDBOX_VERSION}",
    ),
    "how-to-guides/sandbox/images.mdx": (f"`cua-sandbox` {SANDBOX_VERSION}",),
    "how-to-guides/sandbox/minecraft.mdx": (f"cua-sandbox {SANDBOX_VERSION}",),
    "how-to-guides/sandbox/prepare-and-reference-a-fleet-image.mdx": (
        f"`cua-sandbox` {SANDBOX_VERSION}",
        f"`cua` {CUA_VERSION}",
    ),
    "how-to-guides/sandbox/run-omarchy-on-cloud-fleet.mdx": (
        f'"cua-sandbox=={SANDBOX_VERSION}"',
    ),
    "how-to-guides/sandbox/share-service-with-signed-url.mdx": (
        f"`@trycua/fleet` {TYPESCRIPT_FLEET_VERSION}",
        f"`cua-sandbox` {SANDBOX_VERSION}",
    ),
    "how-to-guides/sandbox/troubleshoot-fleet-pools.mdx": (
        f"`@trycua/fleet@{TYPESCRIPT_FLEET_VERSION}`",
    ),
    "how-to-guides/sandbox/tunneling.mdx": (
        f"`cua-sandbox`\n{SANDBOX_VERSION}",
        f"`cua=={CUA_VERSION}`",
    ),
    "reference/cua-cli/cli-reference.mdx": (f"`cua-cli` **{CLI_VERSION}**",),
    "reference/sandbox-sdk/image.mdx": (f"`cua-sandbox` {SANDBOX_VERSION}",),
    "reference/sandbox-sdk/index.mdx": (
        f"`cua-sandbox` {SANDBOX_VERSION}",
        f"`cua` {CUA_VERSION}",
        f"`@trycua/fleet` {TYPESCRIPT_FLEET_VERSION}",
    ),
    "reference/sandbox-sdk/interfaces.mdx": (
        f"`cua-sandbox` {SANDBOX_VERSION}",
    ),
    "reference/sandbox-sdk/pool.mdx": (
        f"`cua_sandbox` {SANDBOX_VERSION}",
        f"`cua-fleet` {PYTHON_FLEET_VERSION}",
    ),
    "reference/sandbox-sdk/typescript.mdx": (
        f"`@trycua/fleet` {TYPESCRIPT_FLEET_VERSION}",
        f"npm install @trycua/fleet@{TYPESCRIPT_FLEET_VERSION}",
    ),
}


def test_maintained_version_facts_match_package_manifests() -> None:
    for relative_path, expected_facts in EXPECTED_FACTS.items():
        text = (DOCS / relative_path).read_text()
        for fact in expected_facts:
            assert fact in text, f"{relative_path} must contain manifest fact {fact!r}"


def test_owned_pages_drop_superseded_maintained_versions_and_commands() -> None:
    stale_facts = (
        re.compile(r"cua-sandbox(?:`|\*\*)?[ =@]+0\.4\.3\b"),
        re.compile(r"cua-fleet(?:`|\*\*)?[ =@]+0\.1\.14\b"),
        re.compile(r"@trycua/fleet(?:`|\*\*)?[ =@]+0\.1\.1\b"),
        re.compile(r"cua-cli(?:`|\*\*)?[ =@]+0\.1\.14\b"),
    )
    obsolete_commands = (
        re.compile(r"\bcua (?:sb|sandbox) create\b"),
        re.compile(r"\bcua (?:sb|sandbox) start\b"),
    )

    for relative_path in EXPECTED_FACTS:
        text = (DOCS / relative_path).read_text()
        for pattern in (*stale_facts, *obsolete_commands):
            assert not pattern.search(text), f"{relative_path} contains {pattern.pattern!r}"


def test_minecraft_keeps_its_intentional_historical_version() -> None:
    text = (DOCS / "how-to-guides/sandbox/minecraft.mdx").read_text()
    assert "The local recipe was originally developed against 0.3.3" in text
    assert "needs cua-sandbox 0.3.3; before that" in text
