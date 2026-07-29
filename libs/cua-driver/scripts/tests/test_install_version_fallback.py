"""Installer release-resolution guards.

Every release opens a window where the baked version constant points at assets
nobody can download yet. Merging the Release Please pull request bumps the
constant on `main` and creates the tag, but the release it creates is a draft;
its assets only become fetchable once the CD workflow finishes building and
publishes it. cua.ai serves both installers straight from `main`, so the public
one-liner advertises an undownloadable version for the length of that build --
observed windows on cua-driver-rs releases run from ~12 minutes to ~3.5 hours.

The baked value must therefore degrade to the GitHub Releases API rather than
fail the install. An explicit pin (CUA_DRIVER_RS_VERSION, -Release) is the
opposite case: the user named one version, so a missing asset must stay fatal
rather than silently installing a different one.

Version *agreement* across the checked-in sources is not tested here --
.github/scripts/validate_release_versions.py already owns that. It cannot cover
these invariants, because at merge time the release genuinely is unpublished.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest

# _install-rust.sh only ever runs on macOS and Linux, so the behavioral tests
# below are posix-only. Gating on os.name as well as bash's presence matters:
# on Windows, `bash.exe` is usually the WSL launcher, which resolves via
# shutil.which but cannot execute anything when no distro is installed.
requires_posix_bash = pytest.mark.skipif(
    os.name != "posix" or shutil.which("bash") is None,
    reason="requires a posix host with bash",
)

REPO_ROOT = Path(__file__).resolve().parents[4]
SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"

WINDOWS_INSTALLER = SCRIPTS / "install.ps1"
UNIX_INSTALLER = SCRIPTS / "_install-rust.sh"


def _windows_source() -> str:
    return WINDOWS_INSTALLER.read_text(encoding="utf-8-sig")


def _unix_source() -> str:
    return UNIX_INSTALLER.read_text(encoding="utf-8")


# ---------- Windows -------------------------------------------------------


def test_windows_download_failure_is_recoverable_rather_than_fatal() -> None:
    """Get-ReleaseZip must report failure by returning, not by exiting.

    An `exit 1` inside the download helper makes any retry unreachable, which
    is precisely how the original defect became unrecoverable.
    """
    source = _windows_source()
    body = source[
        source.index("function Get-ReleaseZip") : source.index("function Get-ReleaseAsset")
    ]

    assert "return $null" in body
    assert "exit 1" not in body


def test_windows_installer_falls_back_when_baked_release_is_unpublished() -> None:
    source = _windows_source()

    # The baked branch must record its provenance for the download path to key on.
    assert "$Script:CuaDriverRsVersionSource = 'baked'" in source

    fallback = source.index("if (-not $zipPath -and $Script:CuaDriverRsVersionSource -eq 'baked')")
    # The recovery must consult the API and retry the download, in that order.
    api_call = source.index("$apiVersion = Get-LatestVersionFromApi", fallback)
    retry = source.index("$zipPath = Get-ReleaseZip $resolvedVersion $archLabel $destDir", api_call)
    fatal = source.index("if (-not $zipPath) {", retry)
    assert fallback < api_call < retry < fatal


def test_windows_installer_does_not_fall_back_for_an_explicit_version_pin() -> None:
    source = _windows_source()

    assert "$Script:CuaDriverRsVersionSource = 'env'" in source
    assert "$Script:CuaDriverRsVersionSource = 'release-arg'" in source
    # Exactly one fallback site, and it is gated on the baked provenance, so
    # neither pin route can reach it.
    assert source.count("$Script:CuaDriverRsVersionSource -eq 'baked'") == 1


def test_windows_installer_adopts_the_version_that_actually_downloaded() -> None:
    """A fallback changes the install path and the cursor-theme capability check.

    Get-ReleaseAsset therefore returns the resolved version, and the caller must
    re-derive $versionedDir from it before staging anything.
    """
    source = _windows_source()

    assert "return @{ StageDir = $stageDir; Version = $version }" in source

    adopt = source.index("if ($asset.Version -ne $version) {")
    assert source.index("$version = $asset.Version", adopt) < source.index(
        "$versionedDir = Join-Path $ReleasesDir", adopt
    )
    # The retarget has to precede the directory creation that consumes it.
    assert adopt < source.index("New-Item -ItemType Directory -Force -Path $versionedDir", adopt)


def test_windows_api_resolver_avoids_the_automatic_matches_variable() -> None:
    """$matches is clobbered by every `-match`; a local of that name is a trap."""
    source = _windows_source()
    body = source[
        source.index("function Get-LatestVersionFromApi") : source.index("function Resolve-Version")
    ]
    code = "\n".join(line for line in body.splitlines() if not line.lstrip().startswith("#"))

    assert "$releaseMatches" in code
    # Word-boundary match so $releaseMatches does not count as a hit.
    assert not re.search(r"\$matches\b", code)


# ---------- Unix ----------------------------------------------------------


def test_unix_installer_falls_back_when_baked_release_is_unpublished() -> None:
    source = _unix_source()

    assert 'VERSION_SOURCE="baked"' in source

    fallback = source.index('if ! download_release_tarball "$VERSION"; then')
    guard = source.index('if [[ "$VERSION_SOURCE" != "baked" ]]; then', fallback)
    api_call = source.index('API_VERSION="$(resolve_latest_version_from_api)"', guard)
    adopt = source.index('VERSION="$API_VERSION"', api_call)
    retry = source.index('if ! download_release_tarball "$VERSION"; then', adopt)
    assert fallback < guard < api_call < adopt < retry


def test_unix_installer_does_not_fall_back_for_an_explicit_version_pin() -> None:
    source = _unix_source()

    assert 'VERSION_SOURCE="pin"' in source
    # The non-baked route exits before reaching the API recovery below it.
    guard = source.index('if [[ "$VERSION_SOURCE" != "baked" ]]; then')
    assert source.index("exit 1", guard) < source.index("resolve_latest_version_from_api", guard)


def test_unix_installer_recomputes_the_tarball_after_a_fallback() -> None:
    """TARBALL feeds `tar -xzf`, so it must be derived after VERSION settles."""
    source = _unix_source()

    assert source.index('TARBALL="$(release_tarball_name "$VERSION")"') < source.index(
        'tar -xzf "$TMP_DIR/$TARBALL"'
    )
    assert source.index('VERSION="$API_VERSION"') < source.index(
        'TARBALL="$(release_tarball_name "$VERSION")"'
    )


def test_unix_api_resolver_queries_a_full_page() -> None:
    """The repo interleaves lume/Python/Swift releases with these.

    A short page can contain no cua-driver-rs-v* tag at all and make a healthy
    repo look empty — which would turn the new fallback into a dead end.
    """
    source = _unix_source()
    assert "per_page=100" in source
    assert "per_page=40" not in source


def _extract_shell_function(source: str, name: str) -> str:
    match = re.search(rf"^{re.escape(name)}\(\) \{{.*?^\}}", source, re.MULTILINE | re.DOTALL)
    assert match, f"could not locate shell function {name}()"
    return match.group(0)


def _run_resolver(tmp_path: Path, releases_json: str, epilogue: str, strict: bool) -> str:
    """Runs the real resolver function with curl shadowed by a shell function.

    A function shim beats a PATH shim here: it needs no exec bit and no PATH
    entry, so the harness behaves the same whether the test host is Linux,
    macOS, or a Windows checkout whose drive letters would otherwise collide
    with the ':' PATH separator.
    """
    fixture = tmp_path / "releases.json"
    fixture.write_text(releases_json, encoding="utf-8")

    script = tmp_path / "run.sh"
    script.write_text(
        textwrap.dedent("""\
            set -{flags}uo pipefail
            REPO="trycua/cua"
            TAG_PREFIX="cua-driver-rs-v"
            curl() {{ cat "{fixture}"; }}
            {function}
            {epilogue}
            """).format(
            # `set -euo pipefail` when the resolver is expected to succeed;
            # `set -uo pipefail` when the test asserts on its non-zero return,
            # which -e would otherwise turn into an abort before the assertion.
            flags="e" if strict else "",
            fixture=fixture.as_posix(),
            function=_extract_shell_function(_unix_source(), "resolve_latest_version_from_api"),
            epilogue=epilogue,
        ),
        encoding="utf-8",
    )

    result = subprocess.run(["bash", script.as_posix()], capture_output=True, text=True, check=True)
    return result.stdout.strip()


@requires_posix_bash
def test_unix_api_resolver_picks_the_highest_semver(tmp_path: Path) -> None:
    """Behavioral check on the grep/sed/sort pipeline, with curl stubbed.

    0.9.1 sorts above 0.12.6 lexicographically, so this fails if the numeric
    sort keys regress. The unrelated cua-driver-v* and lume-v* tags must be
    filtered out entirely.
    """
    resolved = _run_resolver(
        tmp_path,
        """
        [{"tag_name": "cua-driver-v9.9.9"},
         {"tag_name": "cua-driver-rs-v0.9.1"},
         {"tag_name": "cua-driver-rs-v0.12.6"},
         {"tag_name": "cua-driver-rs-v0.11.0"},
         {"tag_name": "lume-v0.4.0"}]
        """,
        epilogue="resolve_latest_version_from_api",
        strict=True,
    )
    assert resolved == "0.12.6"


@requires_posix_bash
def test_unix_api_resolver_fails_when_no_tag_matches(tmp_path: Path) -> None:
    """A miss must return non-zero so the caller can report it, not print junk."""
    resolved = _run_resolver(
        tmp_path,
        '[{"tag_name": "lume-v0.4.0"}]',
        epilogue="if resolve_latest_version_from_api; then echo RESOLVED; else echo NOMATCH; fi",
        strict=False,
    )
    assert resolved == "NOMATCH"
