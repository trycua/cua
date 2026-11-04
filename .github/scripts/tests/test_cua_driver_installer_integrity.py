"""The Cua Driver release installers check every download before installing it.

The shell installer runs end to end on Linux against a fake GitHub release
served by a `curl` shim, with a temporary HOME, install directory, and a stub
for the daemon-cleanup helpers, so nothing outside the temporary tree is
touched. The PowerShell installer is checked structurally here; its behaviour
is covered by the published-release installer runs in cd-rust-cua-driver.yml.
"""

from __future__ import annotations

import hashlib
import io
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPTS = REPO_ROOT / "libs/cua-driver/scripts"
RELEASE_URL = "https://github.com/trycua/cua/releases/download/"
WORKFLOW_IDENTITY = "https://github.com/trycua/cua/.github/workflows/cd-rust-cua-driver.yml@refs/tags/"

linux_only = pytest.mark.skipif(
    sys.platform != "linux", reason="runs the Linux install path against a temporary HOME"
)

CURL_SHIM = f"""#!{sys.executable}
import os, shutil, sys
args = sys.argv[1:]
out = None
write = None
url = None
fail = False
i = 0
while i < len(args):
    a = args[i]
    if a == "-o":
        out = args[i + 1]; i += 2; continue
    if a == "-w":
        write = args[i + 1]; i += 2; continue
    if a.startswith("-") and not a.startswith("--"):
        fail = fail or "f" in a
        i += 1; continue
    if a.startswith("--"):
        i += 1; continue
    url = a; i += 1
root = os.environ["FAKE_RELEASES"]
path = None
if url and url.startswith({RELEASE_URL!r}):
    path = os.path.join(root, url[len({RELEASE_URL!r}):])
if path and os.path.isfile(path):
    if out:
        shutil.copyfile(path, out)
    else:
        sys.stdout.write(open(path).read())
    if write:
        sys.stdout.write("200")
    sys.exit(0)
if write:
    sys.stdout.write("404")
sys.exit(22 if fail else 0)
"""

COSIGN_SHIM = """#!/usr/bin/env bash
# Accepts a bundle whose content is "valid-for <identity>" for the identity
# the installer asks for, like cosign verify-blob --certificate-identity.
identity="" bundle=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --certificate-identity) identity="$2"; shift 2 ;;
    --bundle) bundle="$2"; shift 2 ;;
    *) shift ;;
  esac
done
[[ "$(cat "$bundle")" == "valid-for $identity" ]]
"""

UNAME_SHIM = """#!/usr/bin/env bash
case "${1:-}" in -m) echo x86_64 ;; *) echo Linux ;; esac
"""

COMMON_STUB = """stop_cua_driver_daemons() { :; }
show_cua_driver_daemon_survivors() { :; }
"""


def tarball(version: str) -> tuple[str, bytes]:
    name = f"cua-driver-rs-{version}-linux-x86_64-binary.tar.gz"
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for member in ("cua-driver", "cua-cursor-theme"):
            body = b"#!/bin/sh\nexit 0\n"
            info = tarfile.TarInfo(member)
            info.size = len(body)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(body))
    return name, buffer.getvalue()


def write_sums(directory: Path, name: str, data: bytes, sums: str) -> None:
    """``good``/``bad`` write SHA256SUMS; ``txt-good``/``txt-bad`` write the
    older release workflow's checksums.txt (the same lines in a Markdown fence)."""
    digest = hashlib.sha256(data if sums.endswith("good") else b"tampered").hexdigest()
    if sums.startswith("txt-"):
        (directory / "checksums.txt").write_text(
            f"## SHA256 Checksums\n```\n{digest}  {name}\n```\n"
        )
    else:
        (directory / "SHA256SUMS").write_text(f"{digest}  {name}\n")


class Fixture:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.releases = root / "releases"
        self.shims = root / "shims"
        self.home = root / "home"
        self.bin = root / "bin"
        self.scripts = root / "scripts"
        for directory in (self.releases, self.shims, self.home, self.bin, self.scripts):
            directory.mkdir(parents=True)
        shutil.copy(SCRIPTS / "_install-rust.sh", self.scripts / "_install-rust.sh")
        (self.scripts / "_install-common.sh").write_text(COMMON_STUB)
        for name, body in (("curl", CURL_SHIM), ("uname", UNAME_SHIM)):
            shim = self.shims / name
            shim.write_text(body)
            shim.chmod(0o755)

    def publish(
        self,
        version: str,
        *,
        sums: str | None = "good",
        bundle: str | None = "good",
    ) -> str:
        tag = f"cua-driver-rs-v{version}"
        directory = self.releases / tag
        directory.mkdir()
        name, data = tarball(version)
        (directory / name).write_bytes(data)
        if sums is not None:
            write_sums(directory, name, data, sums)
        if bundle is not None:
            identity = WORKFLOW_IDENTITY + (tag if bundle == "good" else "cua-driver-rs-v0.0.1")
            (directory / f"{name}.sigstore.json").write_text(f"valid-for {identity}")
        return name

    def with_cosign(self) -> None:
        shim = self.shims / "cosign"
        shim.write_text(COSIGN_SHIM)
        shim.chmod(0o755)

    def install(self, version: str, *args: str) -> subprocess.CompletedProcess[str]:
        env = {
            "PATH": f"{self.shims}:/usr/bin:/bin",
            "HOME": str(self.home),
            "FAKE_RELEASES": str(self.releases),
            "CUA_DRIVER_RS_VERSION": version,
            "CUA_DRIVER_RS_INSTALL_DIR": str(self.bin),
            "CUA_DRIVER_RS_NO_MODIFY_PATH": "1",
            "CUA_DRIVER_RS_TELEMETRY_ENABLED": "0",
            "CUA_TELEMETRY": "0",
            "DO_NOT_TRACK": "1",
        }
        return subprocess.run(
            ["bash", str(self.scripts / "_install-rust.sh"), *args],
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
        )

    def installed(self) -> bool:
        return (self.home / ".cua-driver/packages/current/cua-driver").exists()


@pytest.fixture
def fixture(tmp_path: Path) -> Fixture:
    return Fixture(tmp_path)


@linux_only
def test_a_verified_release_installs(fixture: Fixture) -> None:
    fixture.with_cosign()
    name = fixture.publish("0.31.0")
    result = fixture.install("0.31.0")
    assert result.returncode == 0, result.stderr + result.stdout
    assert f"verified {name} against SHA256SUMS" in result.stdout
    assert f"verified the Sigstore signature of {name}" in result.stdout
    assert fixture.installed()


@linux_only
def test_a_checksum_mismatch_is_refused(fixture: Fixture) -> None:
    fixture.publish("0.31.0", sums="bad")
    result = fixture.install("0.31.0")
    assert result.returncode != 0
    assert "does not match cua-driver-rs-v0.31.0's SHA256SUMS" in result.stderr
    assert not fixture.installed()


@linux_only
def test_a_release_that_must_publish_sums_is_refused_without_them(fixture: Fixture) -> None:
    fixture.publish("0.31.0", sums=None)
    result = fixture.install("0.31.0")
    assert result.returncode != 0
    assert "has no SHA256SUMS" in result.stderr
    assert not fixture.installed()


@linux_only
def test_a_release_that_must_publish_a_bundle_is_refused_without_one(fixture: Fixture) -> None:
    fixture.publish("0.31.1", bundle=None)
    result = fixture.install("0.31.1")
    assert result.returncode != 0
    assert "has no Sigstore bundle" in result.stderr
    assert not fixture.installed()


@linux_only
def test_a_patch_release_from_the_older_workflow_installs_without_a_bundle(fixture: Fixture) -> None:
    # A 0.31.x release cut before the signing workflow reached main publishes
    # checksums.txt only. Refusing it would break every default install once
    # that version is baked.
    name = fixture.publish("0.31.1", sums="txt-good", bundle=None)
    result = fixture.install("0.31.1")
    assert result.returncode == 0, result.stderr + result.stdout
    assert f"verified {name} against checksums.txt" in result.stdout
    assert fixture.installed()


@linux_only
def test_the_signing_floor_needs_a_bundle_even_without_sha256sums(fixture: Fixture) -> None:
    fixture.publish("0.32.0", sums="txt-good", bundle=None)
    result = fixture.install("0.32.0")
    assert result.returncode != 0
    assert "has no Sigstore bundle" in result.stderr
    assert not fixture.installed()


@linux_only
def test_0_31_0_is_verified_against_its_checksums_txt(fixture: Fixture) -> None:
    # 0.31.0 came from the older release workflow: checksums.txt, no bundles.
    name = fixture.publish("0.31.0", sums="txt-good", bundle=None)
    result = fixture.install("0.31.0")
    assert result.returncode == 0, result.stderr + result.stdout
    assert f"verified {name} against checksums.txt" in result.stdout
    assert fixture.installed()


@linux_only
def test_a_checksums_txt_mismatch_is_refused(fixture: Fixture) -> None:
    fixture.publish("0.31.0", sums="txt-bad", bundle=None)
    result = fixture.install("0.31.0")
    assert result.returncode != 0
    assert "does not match cua-driver-rs-v0.31.0's checksums.txt" in result.stderr
    assert not fixture.installed()


@linux_only
def test_an_older_release_without_sums_installs_with_a_warning(fixture: Fixture) -> None:
    fixture.publish("0.30.2", sums=None, bundle=None)
    result = fixture.install("0.30.2")
    assert result.returncode == 0, result.stderr + result.stdout
    assert "predates published SHA256SUMS" in result.stderr
    assert fixture.installed()


@linux_only
def test_a_signature_for_another_tag_is_refused(fixture: Fixture) -> None:
    fixture.with_cosign()
    fixture.publish("0.31.0", bundle="other-tag")
    result = fixture.install("0.31.0")
    assert result.returncode != 0
    assert "did not verify" in result.stderr
    assert not fixture.installed()


@linux_only
def test_require_signature_needs_cosign(fixture: Fixture) -> None:
    fixture.publish("0.31.0")
    result = fixture.install("0.31.0", "--require-signature")
    assert result.returncode != 0
    assert "--require-signature needs cosign" in result.stderr
    assert not fixture.installed()


@linux_only
def test_without_cosign_a_signed_release_warns_and_installs(fixture: Fixture) -> None:
    fixture.publish("0.31.0")
    result = fixture.install("0.31.0")
    assert result.returncode == 0, result.stderr + result.stdout
    assert "cosign is not installed" in result.stderr
    assert fixture.installed()


def test_installers_share_the_checksum_floor_and_workflow_identity() -> None:
    shell = (SCRIPTS / "_install-rust.sh").read_text(encoding="utf-8")
    powershell = (SCRIPTS / "install.ps1").read_text(encoding="utf-8")
    assert 'CHECKSUMS_REQUIRED_FROM="0.31.0"' in shell
    assert '$ChecksumsRequiredFrom = [version]"0.31.0"' in powershell
    assert 'SIGSTORE_REQUIRED_FROM="0.32.0"' in shell
    assert '$SigstoreRequiredFrom = [version]"0.32.0"' in powershell
    identity = ".github/workflows/cd-rust-cua-driver.yml@refs/tags/"
    assert identity in shell and identity in powershell
    assert "https://token.actions.githubusercontent.com" in shell
    assert "https://token.actions.githubusercontent.com" in powershell
    # Windows checks Authenticode on every staged executable, as CD does.
    assert "Get-AuthenticodeSignature" in powershell
    assert "CN=\"?Cua AI, Inc\\.\"?" in powershell


def test_release_publishes_sums_and_bundles_with_oidc_only_on_the_release_job() -> None:
    workflow = (REPO_ROOT / ".github/workflows/cd-rust-cua-driver.yml").read_text(encoding="utf-8")
    release = workflow.split("\n  release:\n", 1)[1].split("\n  verify-published-signatures:\n", 1)[0]
    assert "    permissions:\n      contents: write\n      id-token: write\n" in release
    assert "name: Publish SHA256SUMS and Sigstore bundles" in release
    assert 'sha256sum "${ASSETS[@]}" | sort -k2 > SHA256SUMS' in release
    assert 'cosign sign-blob --yes --bundle "$f.sigstore.json" "$f"' in release
    # Signing happens before the draft is published with every staged asset.
    assert release.index("Publish SHA256SUMS and Sigstore bundles") < release.index(
        "Publish the verified Release Please draft"
    )


PWSH_HARNESS = r"""
param([string]$Script, [string]$Releases, [string]$Tag, [string]$Zip, [string]$Version,
      [string]$Cosign, [string]$Require)
$ErrorActionPreference = 'Stop'
# Load only the installer's function definitions and constants it needs;
# its body (download, install, autostart) never runs here.
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Script, [ref]$null, [ref]$null)
foreach ($fn in $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $false)) {
    . ([scriptblock]::Create($fn.Extent.Text))
}
foreach ($assign in $ast.EndBlock.Statements | Where-Object {
    $_ -is [System.Management.Automation.Language.AssignmentStatementAst] -and
    $_.Left.Extent.Text -in @('$Repo', '$TagPrefix', '$ChecksumsRequiredFrom', '$SigstoreRequiredFrom', '$AuthenticodeRequiredFrom', '$CosignOidcIssuer')
}) {
    . ([scriptblock]::Create($assign.Extent.Text))
}
$RequireSignature = $Require -eq '1'
$Script:CuaDriverRsReleaseTag = $Tag
function Invoke-WebRequest([string]$Uri, [string]$OutFile, [switch]$UseBasicParsing) {
    $prefix = 'https://github.com/trycua/cua/releases/download/'
    $path = Join-Path $Releases $Uri.Substring($prefix.Length)
    if (-not (Test-Path -LiteralPath $path)) {
        $response = [pscustomobject]@{ StatusCode = 404 }
        $exception = [System.Exception]::new('404')
        $exception | Add-Member -NotePropertyName Response -NotePropertyValue $response -Force
        throw $exception
    }
    Copy-Item -LiteralPath $path -Destination $OutFile
}
if ($Cosign) {
    function cosign { param([Parameter(ValueFromRemainingArguments = $true)]$Rest)
        $identity = $Rest[[array]::IndexOf($Rest, '--certificate-identity') + 1]
        $bundle = $Rest[[array]::IndexOf($Rest, '--bundle') + 1]
        $global:LASTEXITCODE = if ((Get-Content -Raw $bundle).Trim() -eq "valid-for $identity") { 0 } else { 1 }
    }
}
$work = Join-Path ([System.IO.Path]::GetTempPath()) ([guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
Copy-Item -LiteralPath (Join-Path (Join-Path $Releases $Tag) $Zip) -Destination $work
Assert-ReleaseZipIntegrity (Join-Path $work $Zip) $Version
Write-Host 'INTEGRITY-OK'
"""


def run_pwsh(fixture: Fixture, version: str, name: str, *, cosign: bool, require: bool = False):
    harness = fixture.root / "harness.ps1"
    harness.write_text(PWSH_HARNESS)
    return subprocess.run(
        [
            shutil.which("pwsh") or "pwsh",
            "-NoProfile",
            "-File",
            str(harness),
            "-Script",
            str(SCRIPTS / "install.ps1"),
            "-Releases",
            str(fixture.releases),
            "-Tag",
            f"cua-driver-rs-v{version}",
            "-Zip",
            name,
            "-Version",
            version,
            "-Cosign",
            "1" if cosign else "",
            "-Require",
            "1" if require else "0",
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )


needs_pwsh = pytest.mark.skipif(shutil.which("pwsh") is None, reason="needs PowerShell 7")


def publish_zip(fixture: Fixture, version: str, *, sums: str | None, bundle: str | None) -> str:
    # The Windows asset name; its content does not matter to the integrity check.
    tag = f"cua-driver-rs-v{version}"
    directory = fixture.releases / tag
    directory.mkdir()
    name = f"cua-driver-rs-{version}-x86_64.zip"
    data = b"zip-bytes-" + version.encode()
    (directory / name).write_bytes(data)
    if sums is not None:
        write_sums(directory, name, data, sums)
    if bundle is not None:
        identity = WORKFLOW_IDENTITY + (tag if bundle == "good" else "cua-driver-rs-v0.0.1")
        (directory / f"{name}.sigstore.json").write_text(f"valid-for {identity}")
    return name


@needs_pwsh
@pytest.mark.parametrize(
    ("version", "sums", "bundle", "cosign", "require", "ok", "message"),
    [
        ("0.31.0", "good", "good", True, False, True, "verified the Sigstore signature"),
        ("0.31.0", "bad", "good", True, False, False, "does not match cua-driver-rs-v0.31.0's SHA256SUMS"),
        ("0.31.0", None, "good", True, False, False, "has no SHA256SUMS"),
        ("0.31.1", "good", None, True, False, False, "has no Sigstore bundle"),
        ("0.31.1", "txt-good", None, False, False, True, "against checksums.txt"),
        ("0.32.0", "txt-good", None, False, False, False, "has no Sigstore bundle"),
        ("0.31.0", "txt-good", None, False, False, True, "against checksums.txt"),
        ("0.31.0", "txt-bad", None, False, False, False, "does not match cua-driver-rs-v0.31.0's checksums.txt"),
        ("0.31.0", "good", "other-tag", True, False, False, "did not verify"),
        ("0.31.0", "good", "good", False, True, False, "-RequireSignature needs cosign"),
        ("0.30.2", None, None, False, False, True, "predates published SHA256SUMS"),
    ],
)
def test_windows_installer_checks_the_zip_before_extracting(
    fixture: Fixture, version, sums, bundle, cosign, require, ok, message
) -> None:
    name = publish_zip(fixture, version, sums=sums, bundle=bundle)
    result = run_pwsh(fixture, version, name, cosign=cosign, require=require)
    output = result.stdout + result.stderr
    assert ("INTEGRITY-OK" in output) == ok, output
    assert message in output, output
