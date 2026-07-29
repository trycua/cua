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

import json
import os
import re
import shutil
import subprocess
import textwrap
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterator

import pytest

# _install-rust.sh only ever runs on macOS and Linux, so the behavioral tests
# below are posix-only. Gating on os.name as well as bash's presence matters:
# on Windows, `bash.exe` is usually the WSL launcher, which resolves via
# shutil.which but cannot execute anything when no distro is installed.
requires_posix_bash = pytest.mark.skipif(
    os.name != "posix" or shutil.which("bash") is None,
    reason="requires a posix host with bash",
)
POWERSHELL = shutil.which("pwsh") or shutil.which("powershell")
requires_powershell = pytest.mark.skipif(
    POWERSHELL is None,
    reason="requires PowerShell",
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


def _extract_powershell_function(source: str, name: str) -> str:
    start = source.index(f"function {name}")
    next_function = source.find("\nfunction ", start + 1)
    assert next_function != -1, f"could not locate the end of PowerShell function {name}"
    return source[start:next_function].strip()


def _run_powershell(tmp_path: Path, body: str) -> subprocess.CompletedProcess[str]:
    script = tmp_path / "installer-harness.ps1"
    script.write_text(
        "$ErrorActionPreference = 'Stop'\n" + body,
        encoding="utf-8",
    )
    return subprocess.run(
        [str(POWERSHELL), "-NoProfile", "-NonInteractive", "-File", str(script)],
        capture_output=True,
        text=True,
        check=True,
    )


@contextmanager
def _powershell_download_server(
    scenario: str,
) -> Iterator[tuple[str, list[str]]]:
    requests: list[str] = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            requests.append(self.path)
            if scenario == "network":
                self.connection.close()
                return
            status = {
                "missing": 404,
                "server": 503,
                "auth": 401,
            }[scenario]
            self.send_response(status)
            self.end_headers()

        def log_message(self, _format: str, *args: object) -> None:
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        yield f"http://{host}:{port}", requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_windows_download_failure_is_recoverable_rather_than_fatal() -> None:
    """Get-ReleaseZip must report failure by returning, not by exiting.

    An `exit 1` inside the download helper makes any retry unreachable, which
    is precisely how the original defect became unrecoverable.
    """
    source = _windows_source()
    body = source[
        source.index("function Get-ReleaseZip") : source.index("function Get-ReleaseAsset")
    ]

    assert "Missing = $true" in body
    assert "ErrorMessage = $_.Exception.Message" in body
    assert "exit 1" not in body


def test_windows_installer_falls_back_when_baked_release_is_unpublished() -> None:
    source = _windows_source()

    # The baked branch must record its provenance for the download path to key on.
    assert "$Script:CuaDriverRsVersionSource = 'baked'" in source

    fallback = source.index(
        "if ($download.Missing -and $Script:CuaDriverRsVersionSource -eq 'baked')"
    )
    # The recovery must consult the API and retry the download, in that order.
    api_call = source.index("$apiVersion = Get-LatestVersionFromApi", fallback)
    retry = source.index(
        "$download = Get-ReleaseZip $resolvedVersion $archLabel $destDir", api_call
    )
    fatal = source.index("if ($download.Missing) {", retry)
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


def test_windows_installer_rechecks_an_already_installed_fallback_before_copy() -> None:
    """A running fallback binary is locked on Windows and must not be overwritten."""
    source = _windows_source()
    adopt = source.index("if ($asset.Version -ne $version) {")
    recheck = source.index(
        "if (Test-Path -LiteralPath (Join-Path $versionedDir $BinaryName))", adopt
    )
    copy = source.index(
        "Copy-Item -LiteralPath (Join-Path $stageDir $BinaryName)", recheck
    )
    assert adopt < recheck < copy


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


def test_windows_api_resolver_accepts_published_stable_tags_marked_prerelease() -> None:
    """Cua Driver releases use GitHub's prerelease flag despite stable x.y.z tags."""
    source = _windows_source()
    body = source[
        source.index("function Get-LatestVersionFromApi") : source.index(
            "function Resolve-Version"
        )
    ]
    assert "(-not $_.draft)" in body
    assert "(-not $_.prerelease)" not in body
    assert "[0-9]+\\.[0-9]+\\.[0-9]+" in body


@requires_powershell
@pytest.mark.parametrize(
    ("scenario", "missing", "attempts", "has_error"),
    [
        ("missing", True, 1, False),
        ("server", False, 3, True),
        ("network", False, 3, True),
        ("auth", False, 1, True),
    ],
)
def test_windows_download_classifies_and_retries_failures_behaviorally(
    tmp_path: Path,
    scenario: str,
    missing: bool,
    attempts: int,
    has_error: bool,
) -> None:
    source = _windows_source()
    with _powershell_download_server(scenario) as (base_url, requests):
        functions = "\n\n".join(
            _extract_powershell_function(source, name)
            for name in (
                "Get-HttpStatusCode",
                "Test-TransientDownloadFailure",
                "Get-ReleaseZip",
            )
        )
        functions = functions.replace(
            "https://github.com/$Repo/releases/download/",
            f"{base_url}/",
        ).replace(
            "Start-Sleep -Seconds $delaySeconds",
            "$null = $delaySeconds",
        )
        result = _run_powershell(
            tmp_path,
            f"""
$Repo = 'trycua/cua'
$TagPrefix = 'cua-driver-rs-v'
function Write-Step {{ param([string]$Message) }}
function Write-WarningStep {{ param([string]$Message) }}

{functions}

$download = Get-ReleaseZip '1.2.3' 'windows-x86_64' '{tmp_path.as_posix()}'
[pscustomobject]@{{
    Missing = [bool]$download.Missing
    Attempts = [int]$download.Attempts
    HasError = [bool]$download.ErrorMessage
}} | ConvertTo-Json -Compress
""",
        )
    observed = json.loads(result.stdout)
    assert observed == {
        "Missing": missing,
        "Attempts": attempts,
        "HasError": has_error,
    }
    if scenario == "network":
        # Invoke-WebRequest has its own transport retry on a dropped socket;
        # the installer-level Attempts count must still remain bounded at three.
        assert len(requests) >= attempts
    else:
        assert len(requests) == attempts


@requires_powershell
def test_windows_api_resolver_filters_drafts_but_accepts_stable_prereleases(
    tmp_path: Path,
) -> None:
    source = _windows_source()
    functions = "\n\n".join(
        _extract_powershell_function(source, name)
        for name in ("Get-GitHubApiHeaders", "Get-LatestVersionFromApi")
    )
    result = _run_powershell(
        tmp_path,
        f"""
$Repo = 'trycua/cua'
$TagPrefix = 'cua-driver-rs-v'
function Write-Step {{ param([string]$Message) }}
function Write-WarningStep {{ param([string]$Message) }}
function Invoke-RestMethod {{
    param(
        [string]$Uri,
        [hashtable]$Headers,
        [switch]$UseBasicParsing
    )
    @(
        [pscustomobject]@{{ tag_name = 'cua-driver-rs-v9.0.0'; draft = $true; prerelease = $false }}
        [pscustomobject]@{{ tag_name = 'cua-driver-rs-v1.20.3'; draft = $false; prerelease = $true }}
        [pscustomobject]@{{ tag_name = 'cua-driver-rs-v8.0.0-rc.1'; draft = $false; prerelease = $false }}
    )
}}

{functions}

Get-LatestVersionFromApi
""",
    )
    assert result.stdout.strip() == "1.20.3"


# ---------- Unix ----------------------------------------------------------


def test_unix_installer_falls_back_when_baked_release_is_unpublished() -> None:
    source = _unix_source()

    assert 'VERSION_SOURCE="baked"' in source

    fallback = source.index('download_release_tarball "$VERSION" || DOWNLOAD_STATUS=$?')
    guard = source.index(
        'if [[ "$VERSION_SOURCE" != "baked" || "$DOWNLOAD_STATUS" != "44" ]]; then',
        fallback,
    )
    api_call = source.index('API_VERSION="$(resolve_latest_version_from_api)"', guard)
    adopt = source.index('VERSION="$API_VERSION"', api_call)
    retry = source.index('download_release_tarball "$VERSION" || DOWNLOAD_STATUS=$?', adopt)
    assert fallback < guard < api_call < adopt < retry


def test_unix_installer_does_not_fall_back_for_an_explicit_version_pin() -> None:
    source = _unix_source()

    assert 'VERSION_SOURCE="pin"' in source
    # The non-baked route exits before reaching the API recovery below it,
    # including when its requested asset is a confirmed 404.
    guard = source.index(
        'if [[ "$VERSION_SOURCE" != "baked" || "$DOWNLOAD_STATUS" != "44" ]]; then'
    )
    assert source.index("exit 1", guard) < source.index("resolve_latest_version_from_api", guard)


def test_unix_installer_recomputes_the_tarball_after_a_fallback() -> None:
    """Every release-derived name must be computed after VERSION settles."""
    source = _unix_source()

    assert source.index('TARBALL="$(release_tarball_name "$VERSION")"') < source.index(
        'tar -xzf "$TMP_DIR/$TARBALL"'
    )
    adopt = source.index('VERSION="$API_VERSION"')
    retag = source.index('TAG="${TAG_PREFIX}${VERSION}"', adopt)
    archive = source.index('TARBALL="$(release_tarball_name "$VERSION")"', retag)
    stage = source.index('STAGE="cua-driver-rs-${VERSION}-darwin-universal"', archive)
    assert adopt < retag < archive < stage


def test_unix_api_resolver_queries_bounded_full_pages() -> None:
    """The repo interleaves lume/Python/Swift releases with these.

    A short page can contain no cua-driver-rs-v* tag at all and make a healthy
    repo look empty — which would turn the new fallback into a dead end.
    """
    source = _unix_source()
    assert "per_page=100" in source
    assert "page=$page" in source
    assert "page<=10" in source
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
            {github_api_curl}
            {published_versions}
            {resolver}
            {epilogue}
            """).format(
            # `set -euo pipefail` when the resolver is expected to succeed;
            # `set -uo pipefail` when the test asserts on its non-zero return,
            # which -e would otherwise turn into an abort before the assertion.
            flags="e" if strict else "",
            fixture=fixture.as_posix(),
            github_api_curl=_extract_shell_function(_unix_source(), "github_api_curl"),
            published_versions=_extract_shell_function(
                _unix_source(), "extract_published_release_versions"
            ),
            resolver=_extract_shell_function(_unix_source(), "resolve_latest_version_from_api"),
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
        [
          {
            "tag_name": "cua-driver-v9.9.9",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v0.9.1",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v0.12.6",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v0.11.0",
            "draft": false
          },
          {
            "tag_name": "lume-v0.4.0",
            "draft": false
          }
        ]
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
        '[\n  {\n    "tag_name": "lume-v0.4.0",\n    "draft": false\n  }\n]',
        epilogue="if resolve_latest_version_from_api; then echo RESOLVED; else echo NOMATCH; fi",
        strict=False,
    )
    assert resolved == "NOMATCH"


@requires_posix_bash
def test_unix_api_resolver_rejects_prerelease_and_malformed_tags(tmp_path: Path) -> None:
    resolved = _run_resolver(
        tmp_path,
        """
        [
          {
            "tag_name": "cua-driver-rs-v9.0.0-rc.1",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v8.0.0+build.1",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v7.0.0.1",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v6.0",
            "draft": false
          },
          {
            "tag_name": "cua-driver-rs-v5.4.3",
            "draft": false
          }
        ]
        """,
        epilogue="resolve_latest_version_from_api",
        strict=True,
    )
    assert resolved == "5.4.3"


@requires_posix_bash
def test_unix_api_resolver_rejects_drafts_but_accepts_stable_prereleases(
    tmp_path: Path,
) -> None:
    resolved = _run_resolver(
        tmp_path,
        """
        [
          {
            "tag_name": "cua-driver-rs-v9.0.0",
            "draft": true,
            "prerelease": false
          },
          {
            "tag_name": "cua-driver-rs-v1.20.3",
            "draft": false,
            "prerelease": true
          }
        ]
        """,
        epilogue="resolve_latest_version_from_api",
        strict=True,
    )
    assert resolved == "1.20.3"


@requires_posix_bash
def test_unix_api_resolver_paginates_and_sends_token_header(tmp_path: Path) -> None:
    calls = tmp_path / "calls"
    filler = ",\n".join(
        f'  {{\n    "tag_name": "lume-v1.0.{i}",\n    "draft": false\n  }}'
        for i in range(100)
    )
    page_one = tmp_path / "page-1.json"
    page_two = tmp_path / "page-2.json"
    page_one.write_text(f"[\n{filler}\n]", encoding="utf-8")
    page_two.write_text(
        (
            '[\n  {\n    "tag_name": "cua-driver-rs-v1.20.3",\n'
            '    "draft": false,\n    "prerelease": true\n  }\n]'
        ),
        encoding="utf-8",
    )
    script = tmp_path / "run-pages.sh"
    script.write_text(
        textwrap.dedent(
            f"""\
            set -euo pipefail
            REPO="trycua/cua"
            TAG_PREFIX="cua-driver-rs-v"
            GH_TOKEN="test-token"
            curl() {{
                printf '%s\\n' "$*" >> "{calls.as_posix()}"
                case "$*" in
                    *"&page=1"*) cat "{page_one.as_posix()}" ;;
                    *"&page=2"*) cat "{page_two.as_posix()}" ;;
                    *) return 99 ;;
                esac
            }}
            {_extract_shell_function(_unix_source(), "github_api_curl")}
            {_extract_shell_function(_unix_source(), "extract_published_release_versions")}
            {_extract_shell_function(_unix_source(), "resolve_latest_version_from_api")}
            resolve_latest_version_from_api
            """
        ),
        encoding="utf-8",
    )
    result = subprocess.run(
        ["bash", script.as_posix()], capture_output=True, text=True, check=True
    )

    assert result.stdout == "1.20.3"
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    assert len(call_lines) == 2
    assert all("Authorization: Bearer test-token" in line for line in call_lines)
    assert all("page=3" not in line for line in call_lines)


def _run_download(tmp_path: Path, scenario: str, token_env: str = "") -> tuple[int, list[str], str]:
    """Run the real download helper with deterministic HTTP/curl behavior."""
    calls = tmp_path / "download-calls"
    script = tmp_path / "run-download.sh"
    script.write_text(
        textwrap.dedent(
            f"""\
            set -uo pipefail
            REPO="trycua/cua"
            TAG_PREFIX="cua-driver-rs-v"
            LABEL="linux-x86_64"
            TMP_DIR="{tmp_path.as_posix()}"
            {token_env}
            log() {{ printf '==> %s\\n' "$*"; }}
            err() {{ printf 'error: %s\\n' "$*" >&2; }}
            sleep() {{ :; }}
            curl() {{
                printf '%s\\n' "$*" >> "{calls.as_posix()}"
                local output="" previous="" arg
                for arg in "$@"; do
                    if [[ "$previous" == "-o" ]]; then output="$arg"; fi
                    previous="$arg"
                done
                case "{scenario}" in
                    success)
                        printf 'archive' > "$output"
                        printf '200'
                        return 0
                        ;;
                    missing)
                        printf '404'
                        return 0
                        ;;
                    transient)
                        printf '503'
                        return 0
                        ;;
                    auth)
                        printf '401'
                        return 0
                        ;;
                    network)
                        printf '000'
                        return 7
                        ;;
                esac
            }}
            {_extract_shell_function(_unix_source(), "release_tarball_name")}
            {_extract_shell_function(_unix_source(), "download_release_tarball")}
            download_release_tarball "1.2.3"
            """
        ),
        encoding="utf-8",
    )
    result = subprocess.run(["bash", script.as_posix()], capture_output=True, text=True)
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    return result.returncode, call_lines, result.stderr


@requires_posix_bash
def test_unix_download_reports_confirmed_404_without_retry(tmp_path: Path) -> None:
    returncode, calls, _ = _run_download(tmp_path, "missing")
    assert returncode == 44
    assert len(calls) == 1


@requires_posix_bash
@pytest.mark.parametrize("scenario", ["transient", "network"])
def test_unix_download_retries_transient_failure_without_fallback(
    tmp_path: Path, scenario: str
) -> None:
    returncode, calls, stderr = _run_download(tmp_path, scenario)
    assert returncode == 1
    assert len(calls) == 3
    assert "refusing to fall back to an older release" in stderr


@requires_posix_bash
def test_unix_download_does_not_retry_auth_failure_or_fallback(tmp_path: Path) -> None:
    returncode, calls, stderr = _run_download(tmp_path, "auth")
    assert returncode == 1
    assert len(calls) == 1
    assert "HTTP 401" in stderr
    assert "refusing to fall back to an older release" in stderr


@requires_posix_bash
def test_unix_public_asset_download_does_not_forward_api_token(tmp_path: Path) -> None:
    returncode, calls, stderr = _run_download(tmp_path, "success", 'GITHUB_TOKEN="api-token"')
    assert returncode == 0
    assert len(calls) == 1
    assert "Authorization:" not in calls[0]
    assert "api-token" not in stderr


def _run_fallback_flow(
    tmp_path: Path, *, version_source: str, download_statuses: list[int]
) -> tuple[subprocess.CompletedProcess[str], list[str]]:
    """Execute the installer's real post-download fallback control flow."""
    source = _unix_source()
    start = source.index("DOWNLOAD_STATUS=0\n")
    end = source.index('TARBALL="$(release_tarball_name "$VERSION")"', start)
    flow = source[start:end]
    statuses = " ".join(str(status) for status in download_statuses)
    calls = tmp_path / "flow-calls"
    script = tmp_path / "run-flow.sh"
    script.write_text(
        textwrap.dedent(
            f"""\
            set -uo pipefail
            VERSION_SOURCE="{version_source}"
            VERSION="1.2.3"
            TAG_PREFIX="cua-driver-rs-v"
            TAG="${{TAG_PREFIX}}${{VERSION}}"
            LABEL="linux-x86_64"
            REPO="trycua/cua"
            DOWNLOAD_RESULTS=({statuses})
            DOWNLOAD_INDEX=0
            err() {{ printf 'error: %s\\n' "$*" >&2; }}
            download_release_tarball() {{
                printf 'download:%s\\n' "$1" >> "{calls.as_posix()}"
                local result="${{DOWNLOAD_RESULTS[$DOWNLOAD_INDEX]}}"
                DOWNLOAD_INDEX=$((DOWNLOAD_INDEX + 1))
                return "$result"
            }}
            resolve_latest_version_from_api() {{
                printf 'api\\n' >> "{calls.as_posix()}"
                printf '1.2.2'
            }}
            {flow}
            printf 'resolved:%s:%s\\n' "$VERSION" "$TAG"
            """
        ),
        encoding="utf-8",
    )
    result = subprocess.run(["bash", script.as_posix()], capture_output=True, text=True)
    call_lines = calls.read_text(encoding="utf-8").splitlines()
    return result, call_lines


@requires_posix_bash
def test_unix_baked_404_falls_back_and_adopts_downloaded_version(tmp_path: Path) -> None:
    result, calls = _run_fallback_flow(
        tmp_path, version_source="baked", download_statuses=[44, 0]
    )
    assert result.returncode == 0
    assert calls == ["download:1.2.3", "api", "download:1.2.2"]
    assert result.stdout == "resolved:1.2.2:cua-driver-rs-v1.2.2\n"
    assert "temporary publish lag" in result.stderr


@requires_posix_bash
@pytest.mark.parametrize(
    ("version_source", "download_status"),
    [("pin", 44), ("pin", 1), ("baked", 1)],
)
def test_unix_non_fallback_download_failure_makes_zero_api_calls(
    tmp_path: Path, version_source: str, download_status: int
) -> None:
    result, calls = _run_fallback_flow(
        tmp_path, version_source=version_source, download_statuses=[download_status]
    )
    assert result.returncode == 1
    assert calls == ["download:1.2.3"]
