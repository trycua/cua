from __future__ import annotations

import json
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


def run_lume_resolver(
    tmp_path: Path,
    version: str,
    baked: str = "",
    channel: str = "stable",
    releases: list[dict] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run the installer's tag resolver. `curl` is stubbed: it records each URL
    in `curl.log` and serves `releases` as the GitHub releases list."""
    source = LUME_INSTALLER.read_text()
    prefix = {"stable": "lume-v", "nightly": "nightly-lume-v"}[channel]
    (tmp_path / "releases.json").write_text(json.dumps(releases or [], indent=2))
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
            LUME_CHANNEL="{channel}"
            LUME_TAG_PREFIX="{prefix}"
            curl() {{
              printf '%s\\n' "${{@: -1}}" >> "{tmp_path}/curl.log"
              cat "{tmp_path}/releases.json"
            }}
            {shell_function(source, "get_latest_lume_tag")}
            get_latest_lume_tag
            """
        )
    )
    return subprocess.run(["bash", str(script)], capture_output=True, text=True)


def curl_urls(tmp_path: Path) -> list[str]:
    log = tmp_path / "curl.log"
    return log.read_text().splitlines() if log.exists() else []


def release(tag: str, draft: bool = False, prerelease: bool = False) -> dict:
    return {"tag_name": tag, "draft": draft, "prerelease": prerelease}


# A monorepo release list where the repo-wide Latest is another component and
# drafts carry the highest Lume versions. The expected picks (1.10.0 over
# 1.9.0, build 10 over build 3) differ from lexical ordering.
MONOREPO_RELEASES = [
    release("cua-driver-rs-v9.0.0"),
    release("lume-v1.11.0", draft=True),
    release("nightly-lume-v1.12.0-nightly.20261101.3", prerelease=True),
    release("nightly-lume-v1.12.0-nightly.20261101.10", prerelease=True),
    release("nightly-lume-v1.12.0-nightly.20261031.20", prerelease=True),
    release("nightly-lume-v1.13.0-nightly.20261102.1", draft=True, prerelease=True),
    release("lume-v1.9.0"),
    release("lume-v1.10.0"),
    release("lume-v1.10.0-rc.1"),
    release("cua-spaces-v20.0.0"),
]


def test_lume_baked_version_resolves_its_exact_tag_without_the_api(tmp_path: Path):
    result = run_lume_resolver(tmp_path, "", baked="1.2.3", releases=MONOREPO_RELEASES)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "lume-v1.2.3"
    assert curl_urls(tmp_path) == []


def test_lume_version_override_wins_over_the_baked_version(tmp_path: Path):
    result = run_lume_resolver(tmp_path, "1.4.0", baked="1.2.3")
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "lume-v1.4.0"
    assert curl_urls(tmp_path) == []


@pytest.mark.parametrize(
    ("channel", "expected"),
    [
        ("stable", "lume-v1.10.0"),
        ("nightly", "nightly-lume-v1.12.0-nightly.20261101.10"),
    ],
)
def test_lume_api_fallback_picks_the_highest_lume_release_not_repo_latest(
    tmp_path: Path, channel: str, expected: str
):
    result = run_lume_resolver(tmp_path, "", channel=channel, releases=MONOREPO_RELEASES)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == expected
    urls = curl_urls(tmp_path)
    assert urls, "the fallback should list releases"
    for url in urls:
        assert url.startswith("https://api.github.com/repos/trycua/cua/releases?"), url
        assert "releases/latest" not in url


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


def test_lume_api_filter_is_exact_draft_aware_and_channel_scoped():
    body = shell_function(LUME_INSTALLER.read_text(), "get_latest_lume_tag")
    assert 'awk -v prefix="$LUME_TAG_PREFIX" -v channel="$LUME_CHANNEL"' in body
    assert 'channel == "stable"' in body
    assert 'channel == "nightly"' in body
    assert '"draft"[[:space:]]*:[[:space:]]*false' in body


def test_installers_make_channel_persistent_but_keep_exact_pins_one_shot():
    lume = LUME_INSTALLER.read_text()
    driver = (ROOT / "libs/cua-driver/scripts/_install-rust.sh").read_text()
    windows = (ROOT / "libs/cua-driver/scripts/install.ps1").read_text(encoding="utf-8-sig")

    assert 'RELEASE_CHANNEL_PATH="$LUME_HOME/release-channel"' in lume
    assert 'if [ "$LUME_CHANNEL_EXPLICIT" = true ]' in lume
    assert "--channel cannot be combined with LUME_VERSION" in lume
    assert 'if [ -n "$LUME_VERSION" ]; then' in lume
    assert lume.index('if [ -n "$LUME_VERSION" ]; then') < lume.index(
        'elif [ -f "$RELEASE_CHANNEL_PATH" ]; then'
    )
    assert 'CHANNEL_STATE_FILE="$HOME_DIR/release-channel"' in driver
    assert 'if [[ "$CHANNEL_EXPLICIT" == "1" ]]' in driver
    assert "--channel cannot be combined with CUA_DRIVER_RS_VERSION" in driver
    assert 'elif [[ -n "${CUA_DRIVER_RS_VERSION:-}" ]]; then' in driver
    assert '$ReleaseChannelPath = Join-Path $HomeDir "release-channel"' in windows
    assert "if ($ChannelWasExplicit)" in windows
    assert "-Channel cannot be combined with an exact release pin" in windows
    pin_precedence = "if ($env:CUA_DRIVER_RS_VERSION -or $Release -ne 'latest')"
    assert windows.index(pin_precedence) < windows.index(
        "if (Test-Path -LiteralPath $ReleaseChannelPath)"
    )


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
