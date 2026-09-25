#!/usr/bin/env python3
"""Verify that Cua Driver release archives are signed before and after publication.

The installer refuses a macOS app that fails ``codesign --verify``. A release
whose archive ships an unsigned or unnotarized app therefore breaks every fresh
macOS install (see #4109). This verifier runs the checks the operating systems
run, on the exact archives that a release uploads or has uploaded:

macOS (``macos``, run on a macOS runner)
    Every ``CuaDriver.app`` inside a darwin directory archive must pass
    ``codesign --verify --deep --strict``, carry a hardened-runtime Developer ID
    signature from the pinned team, be accepted by ``spctl -a -t exec`` with
    source ``Notarized Developer ID``, and have a stapled notarization ticket
    (``xcrun stapler validate``). The standalone ``cua-driver``,
    ``cua-cursor-theme``, and ``libcua_driver_sdk.dylib`` copies must carry the
    same team's hardened-runtime signature. Any ``.pkg`` or ``.dmg`` asset must
    be notarized and stapled too.

Windows (``windows``, run on a Windows runner)
    Every PE file (``.exe``, ``.dll``, ``.node``) inside a Windows archive must
    have a ``Valid`` timestamped Authenticode signature whose signer is the
    expected organization.

The command exits non-zero with every failure listed when any check fails.
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass, field
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from typing import Callable, Sequence


RELEASE_TEAM_ID = "YCK386LBJ7"
RELEASE_SIGNER = "Cua AI, Inc."
DARWIN_LABELS = ("darwin-arm64", "darwin-x86_64", "darwin-universal")
WINDOWS_ARCHES = ("x86_64", "arm64")
# Mach-O files that the release pipeline signs with the Developer ID identity.
# cua_driver_node_runtime.node is loaded by Node and is not Developer ID signed
# by the current pipeline, so it is intentionally not listed here.
SIGNED_MACHO_FILES = ("cua-driver", "cua-cursor-theme", "libcua_driver_sdk.dylib")
APP_EXECUTABLES = ("Contents/MacOS/cua-driver", "Contents/MacOS/cua-cursor-theme")
PE_SUFFIXES = (".exe", ".dll", ".node")


@dataclass
class CommandResult:
    returncode: int
    output: str


Runner = Callable[[Sequence[str]], CommandResult]


def run_command(argv: Sequence[str]) -> CommandResult:
    completed = subprocess.run(
        list(argv),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )
    return CommandResult(completed.returncode, completed.stdout)


@dataclass
class Report:
    checked: list[str] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)

    def ok(self, message: str) -> None:
        self.checked.append(message)
        print(f"ok: {message}")

    def fail(self, message: str) -> None:
        self.failures.append(message)
        print(f"FAIL: {message}")


def _indent(output: str) -> str:
    return "\n".join(f"    {line}" for line in output.strip().splitlines())


def _team_pattern(team_id: str) -> re.Pattern[str]:
    return re.compile(
        rf"^origin=Developer ID Application: .+ \({re.escape(team_id)}\)$",
        re.MULTILINE,
    )


def check_developer_id_signature(
    path: Path, label: str, team_id: str, run: Runner, report: Report, *, deep: bool
) -> bool:
    """codesign --verify plus identity details shared by apps and bare Mach-O."""
    verify = ["codesign", "--verify", "--strict", "--verbose=2"]
    if deep:
        verify.insert(2, "--deep")
    result = run([*verify, str(path)])
    if result.returncode != 0:
        report.fail(f"{label}: codesign --verify failed\n{_indent(result.output)}")
        return False

    details = run(["codesign", "-dvv", str(path)])
    problems = []
    if details.returncode != 0:
        problems.append("codesign -dvv failed")
    if not re.search(rf"^TeamIdentifier={re.escape(team_id)}$", details.output, re.MULTILINE):
        problems.append(f"team identifier is not {team_id}")
    if not re.search(
        rf"^Authority=Developer ID Application: .+ \({re.escape(team_id)}\)$",
        details.output,
        re.MULTILINE,
    ):
        problems.append("leaf authority is not a Developer ID Application certificate")
    if not re.search(r"^CodeDirectory .*flags=0x[0-9a-f]+\([^)]*\bruntime\b", details.output, re.MULTILINE):
        problems.append("hardened runtime is not enabled")
    if re.search(r"^CodeDirectory .*\badhoc\b", details.output, re.MULTILINE):
        problems.append("signature is ad hoc")
    if problems:
        report.fail(f"{label}: " + "; ".join(problems) + f"\n{_indent(details.output)}")
        return False
    report.ok(f"{label}: Developer ID signature from {team_id}")
    return True


def check_notarized(
    path: Path, label: str, team_id: str, run: Runner, report: Report, *, assessment: str
) -> None:
    """Gatekeeper acceptance as a notarized Developer ID item plus a stapled ticket."""
    argv = ["spctl", "-a", "-vvv", "-t", assessment]
    if assessment == "open":
        argv += ["--context", "context:primary-signature"]
    result = run([*argv, str(path)])
    problems = []
    if result.returncode != 0 or not re.search(r": accepted$", result.output, re.MULTILINE):
        problems.append(f"spctl -t {assessment} did not accept it")
    if not re.search(r"^source=Notarized Developer ID$", result.output, re.MULTILINE):
        problems.append("Gatekeeper source is not 'Notarized Developer ID'")
    if not _team_pattern(team_id).search(result.output):
        problems.append(f"Gatekeeper origin is not the {team_id} Developer ID")
    if problems:
        report.fail(f"{label}: " + "; ".join(problems) + f"\n{_indent(result.output)}")
    else:
        report.ok(f"{label}: Gatekeeper accepts it as Notarized Developer ID ({team_id})")

    stapled = run(["xcrun", "stapler", "validate", str(path)])
    if stapled.returncode != 0:
        report.fail(f"{label}: no valid stapled notarization ticket\n{_indent(stapled.output)}")
    else:
        report.ok(f"{label}: stapled notarization ticket is valid")


def _extract_tar(archive: Path, destination: Path, run: Runner) -> bool:
    # System tar matches what the installer runs and preserves bundle metadata.
    result = run(["tar", "-xzf", str(archive), "-C", str(destination)])
    return result.returncode == 0


def verify_macos(
    artifacts: Path,
    version: str,
    team_id: str = RELEASE_TEAM_ID,
    *,
    run: Runner | None = None,
    workdir: Path | None = None,
) -> Report:
    run = run or run_command
    report = Report()
    archives = sorted(artifacts.rglob(f"cua-driver-rs-{version}-darwin-*.tar.gz"))
    by_name = {archive.name: archive for archive in archives}
    expected = [f"cua-driver-rs-{version}-{label}.tar.gz" for label in DARWIN_LABELS]
    expected.append(f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz")
    for name in expected:
        if name not in by_name:
            report.fail(f"missing darwin archive {name}")

    scratch = Path(tempfile.mkdtemp(prefix="cua-driver-signatures-", dir=workdir))
    try:
        for name in expected:
            archive = by_name.get(name)
            if archive is None:
                continue
            destination = scratch / name.removesuffix(".tar.gz")
            destination.mkdir(parents=True)
            if not _extract_tar(archive, destination, run):
                report.fail(f"{name}: could not be extracted")
                continue
            if name.endswith("-binary.tar.gz"):
                root = destination
            else:
                root = destination / name.removesuffix(".tar.gz")
                app = root / "CuaDriver.app"
                if not app.is_dir():
                    report.fail(f"{name}: CuaDriver.app is missing")
                else:
                    label = f"{name}:CuaDriver.app"
                    if check_developer_id_signature(
                        app, label, team_id, run, report, deep=True
                    ):
                        check_notarized(app, label, team_id, run, report, assessment="exec")
                    for executable in APP_EXECUTABLES:
                        inner = app / executable
                        if inner.is_file():
                            check_developer_id_signature(
                                inner, f"{label}/{executable}", team_id, run, report, deep=False
                            )
                        else:
                            report.fail(f"{label}: {executable} is missing")
            for file_name in SIGNED_MACHO_FILES:
                target = root / file_name
                if not target.is_file():
                    report.fail(f"{name}: {file_name} is missing")
                    continue
                check_developer_id_signature(
                    target, f"{name}:{file_name}", team_id, run, report, deep=False
                )
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    for installer in sorted(artifacts.rglob("*.pkg")):
        check_notarized(installer, installer.name, team_id, run, report, assessment="install")
    for image in sorted(artifacts.rglob("*.dmg")):
        check_notarized(image, image.name, team_id, run, report, assessment="open")
    return report


POWERSHELL_AUTHENTICODE = r"""
$ErrorActionPreference = 'Stop'
$paths = Get-Content -LiteralPath $args[0] | Where-Object { $_ }
$rows = foreach ($path in $paths) {
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    [pscustomobject]@{
        Path = $path
        Status = $signature.Status.ToString()
        StatusMessage = $signature.StatusMessage
        Subject = if ($signature.SignerCertificate) { $signature.SignerCertificate.Subject } else { '' }
        Timestamped = [bool]$signature.TimeStamperCertificate
    }
}
ConvertTo-Json -InputObject @($rows) -Depth 3
"""


def authenticode_signatures(paths: Sequence[Path], run: Runner, scratch: Path) -> list[dict]:
    listing = scratch / "authenticode-paths.txt"
    listing.write_text("\n".join(str(path) for path in paths) + "\n", encoding="utf-8")
    script = scratch / "authenticode.ps1"
    script.write_text(POWERSHELL_AUTHENTICODE, encoding="utf-8")
    shell = shutil.which("pwsh") or shutil.which("powershell") or "pwsh"
    result = run([shell, "-NoProfile", "-NonInteractive", "-File", str(script), str(listing)])
    if result.returncode != 0:
        raise RuntimeError(f"Get-AuthenticodeSignature failed:\n{_indent(result.output)}")
    return json.loads(result.output)


def verify_windows(
    artifacts: Path,
    version: str,
    signer: str = RELEASE_SIGNER,
    *,
    run: Runner | None = None,
    workdir: Path | None = None,
) -> Report:
    run = run or run_command
    report = Report()
    expected = []
    for arch in WINDOWS_ARCHES:
        expected += [
            f"cua-driver-rs-{version}-windows-{arch}.zip",
            f"cua-driver-rs-{version}-windows-{arch}-binary.zip",
        ]
    by_name = {
        archive.name: archive
        for archive in artifacts.rglob(f"cua-driver-rs-{version}-windows-*.zip")
    }
    subject = re.compile(rf"(^|,\s*)CN=\"?{re.escape(signer)}\"?(,|$)")

    scratch = Path(tempfile.mkdtemp(prefix="cua-driver-authenticode-", dir=workdir))
    try:
        pe_files: list[tuple[str, Path]] = []
        for name in expected:
            archive = by_name.get(name)
            if archive is None:
                report.fail(f"missing Windows archive {name}")
                continue
            destination = scratch / name.removesuffix(".zip")
            with zipfile.ZipFile(archive) as bundle:
                bundle.extractall(destination)
            found = sorted(
                path for path in destination.rglob("*")
                if path.is_file() and path.suffix.lower() in PE_SUFFIXES
            )
            if not any(path.name.lower() == "cua-driver.exe" for path in found):
                report.fail(f"{name}: cua-driver.exe is missing")
            pe_files += [(f"{name}:{path.relative_to(destination).as_posix()}", path) for path in found]

        if pe_files:
            try:
                rows = authenticode_signatures([path for _, path in pe_files], run, scratch)
            except (RuntimeError, json.JSONDecodeError) as error:
                report.fail(str(error))
                rows = []
            by_path = {str(row.get("Path")): row for row in rows}
            for label, path in pe_files:
                row = by_path.get(str(path))
                if row is None:
                    if rows:
                        report.fail(f"{label}: no Authenticode result")
                    continue
                problems = []
                if row.get("Status") != "Valid":
                    problems.append(f"status {row.get('Status')}: {row.get('StatusMessage')}")
                if not subject.search(str(row.get("Subject", ""))):
                    problems.append(f"unexpected signer {row.get('Subject') or '<none>'}")
                if not row.get("Timestamped"):
                    problems.append("signature is not timestamped")
                if problems:
                    report.fail(f"{label}: " + "; ".join(problems))
                else:
                    report.ok(f"{label}: valid timestamped Authenticode signature from {signer}")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    return report


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    subcommands = parser.add_subparsers(dest="platform", required=True)
    macos = subcommands.add_parser("macos", help="verify darwin archives (run on macOS)")
    macos.add_argument("--team-id", default=RELEASE_TEAM_ID)
    windows = subcommands.add_parser("windows", help="verify Windows archives (run on Windows)")
    windows.add_argument("--signer", default=RELEASE_SIGNER)
    for command in (macos, windows):
        command.add_argument("--artifacts", type=Path, required=True)
        command.add_argument("--version", required=True)
        command.add_argument(
            "--no-annotations",
            action="store_true",
            help="omit GitHub error annotations (for negative controls that must fail)",
        )
    args = parser.parse_args(argv)

    if not args.artifacts.is_dir():
        print(f"error: artifact directory {args.artifacts} does not exist", file=sys.stderr)
        return 2
    if args.platform == "macos":
        report = verify_macos(args.artifacts, args.version, args.team_id)
    else:
        report = verify_windows(args.artifacts, args.version, args.signer)

    if not report.checked and not report.failures:
        report.fail("no signature checks ran")
    if report.failures:
        print(
            f"\n{len(report.failures)} signature check(s) failed for Cua Driver "
            f"{args.version} ({args.platform}); this release must not be published or installed.",
            file=sys.stderr,
        )
        if not args.no_annotations:
            for failure in report.failures:
                print(
                    f"::error title=Cua Driver release signature::{failure.splitlines()[0]}",
                    file=sys.stderr,
                )
        return 1
    print(f"\nAll {len(report.checked)} {args.platform} signature checks passed for {args.version}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
