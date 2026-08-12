from __future__ import annotations

from pathlib import Path
import re
import subprocess
import textwrap

import pytest


ROOT = Path(__file__).resolve().parents[3]
LUME_INSTALLER = ROOT / "libs/lume/scripts/install.sh"


def shell_function(source: str, name: str) -> str:
    start = source.index(f"{name}() {{")
    depth = 0
    for index in range(start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise AssertionError(f"could not find end of {name}")


def run_lume_resolver(tmp_path: Path, version: str, baked: str = "") -> subprocess.CompletedProcess[str]:
    source = LUME_INSTALLER.read_text()
    script = tmp_path / "lume-resolver.sh"
    script.write_text(
        textwrap.dedent(
            f"""\
            set -e
            RED=""
            NORMAL=""
            BOLD=""
            GITHUB_REPO="trycua/cua"
            LUME_VERSION="{version}"
            LUME_BAKED_VERSION="{baked}"
            {shell_function(source, "get_latest_lume_tag")}
            get_latest_lume_tag
            """
        )
    )
    return subprocess.run(["bash", str(script)], capture_output=True, text=True)


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("1.2.3", "lume-v1.2.3"),
        ("lume-v1.2.3", "lume-v1.2.3"),
        (
            "1.2.4-nightly.20260812.42",
            "nightly-lume-v1.2.4-nightly.20260812.42",
        ),
        (
            "nightly-lume-v1.2.4-nightly.20260812.42",
            "nightly-lume-v1.2.4-nightly.20260812.42",
        ),
    ],
)
def test_lume_exact_pin_selects_only_its_stable_or_nightly_namespace(
    tmp_path: Path, value: str, expected: str
):
    result = run_lume_resolver(tmp_path, value)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == expected


@pytest.mark.parametrize(
    "value",
    [
        "nightly-cua-driver-rs-v1.2.4-nightly.20260812.42",
        "lume-v1.2.4-nightly.20260812.42",
        "nightly-lume-v1.2.4-rc.1",
        "1.2",
    ],
)
def test_lume_exact_pin_rejects_cross_component_and_malformed_tags(
    tmp_path: Path, value: str
):
    result = run_lume_resolver(tmp_path, value)
    assert result.returncode != 0
    assert "exact x.y.z stable version or canonical nightly tag" in result.stderr


def test_lume_stable_api_filter_is_exact_draft_aware_and_nightly_blind():
    body = shell_function(LUME_INSTALLER.read_text(), "get_latest_lume_tag")
    assert "tag ~ /^lume-v[0-9]+\\.[0-9]+\\.[0-9]+$/" in body
    assert '"draft"[[:space:]]*:[[:space:]]*false' in body
    assert "nightly-lume" not in body.split('if [ -n "$LUME_BAKED_VERSION" ]', 1)[1]


def test_driver_download_uses_the_resolved_tag_not_the_stable_prefix():
    source = (ROOT / "libs/cua-driver/scripts/_install-rust.sh").read_text()
    body = shell_function(source, "download_release_tarball")
    assert "releases/download/${TAG}/" in body
    assert "releases/download/${TAG_PREFIX}" not in body


def test_windows_driver_has_disjoint_exact_pin_grammars():
    source = (ROOT / "libs/cua-driver/scripts/install.ps1").read_text(encoding="utf-8-sig")
    assert '$NightlyTagPrefix = "nightly-cua-driver-rs-v"' in source
    stable = re.search(
        r"\^\(\?:cua-driver-rs-v\|v\)\?\(\[0-9\]\+.*?\$", source
    )
    assert stable
    assert "^nightly-cua-driver-rs-v" in source
    assert "releases/download/$Script:CuaDriverRsReleaseTag/" in source
