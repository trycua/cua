"""Contract tests for the Cua Driver release signature gate.

The fake tool runner below reproduces the real outputs of codesign, spctl,
stapler, and Get-AuthenticodeSignature observed on the published 0.28.2
(notarized) and 0.28.3 (unsigned CuaDriver.app, #4109) archives.
"""

from __future__ import annotations

import io
import json
from pathlib import Path
import subprocess
import tarfile
import zipfile

import pytest

import verify_cua_driver_release_signatures as gate
from verify_cua_driver_release_signatures import CommandResult, main, verify_macos, verify_windows


VERSION = "1.2.3"
TEAM = "YCK386LBJ7"
SIGNED_DETAILS = f"""Executable=/x
Identifier=com.trycua.driver
CodeDirectory v=20500 size=62557 flags=0x10000(runtime) hashes=1944+7 location=embedded
Authority=Developer ID Application: Cua AI, Inc. ({TEAM})
Authority=Developer ID Certification Authority
Authority=Apple Root CA
TeamIdentifier={TEAM}
"""
ADHOC_DETAILS = """Identifier=cua_driver-98a9f20745f8bdbc
CodeDirectory v=20400 size=264984 flags=0x20002(adhoc,linker-signed) hashes=8277+0 location=embedded
TeamIdentifier=not set
"""
ACCEPTED = f"""x: accepted
source=Notarized Developer ID
origin=Developer ID Application: Cua AI, Inc. ({TEAM})
"""


def _add(tar: tarfile.TarFile, name: str, data: bytes = b"\xcf\xfa\xed\xfe") -> None:
    info = tarfile.TarInfo(name)
    info.size = len(data)
    info.mode = 0o755
    tar.addfile(info, io.BytesIO(data))


def make_darwin_artifacts(root: Path, version: str = VERSION, *, omit: str = "") -> Path:
    root.mkdir(parents=True, exist_ok=True)
    for label in ("darwin-arm64", "darwin-x86_64", "darwin-universal"):
        stage = f"cua-driver-rs-{version}-{label}"
        with tarfile.open(root / f"{stage}.tar.gz", "w:gz") as tar:
            for name in ("cua-driver", "cua-cursor-theme", "libcua_driver_sdk.dylib"):
                if name != omit:
                    _add(tar, f"{stage}/{name}")
            _add(tar, f"{stage}/cua_driver_node_runtime.node")
            if omit != "CuaDriver.app":
                _add(tar, f"{stage}/CuaDriver.app/Contents/Info.plist", b"<plist/>")
                _add(tar, f"{stage}/CuaDriver.app/Contents/MacOS/cua-driver")
                _add(tar, f"{stage}/CuaDriver.app/Contents/MacOS/cua-cursor-theme")
    with tarfile.open(root / f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz", "w:gz") as tar:
        for name in ("cua-driver", "cua-cursor-theme", "libcua_driver_sdk.dylib"):
            _add(tar, name)
    return root


class FakeMacTools:
    """codesign/spctl/stapler outputs for a signed, ad hoc, or partially valid release."""

    def __init__(
        self,
        *,
        signed: bool = True,
        notarized: bool = True,
        stapled: bool = True,
        adhoc_names: tuple[str, ...] = (),
        team: str = TEAM,
    ) -> None:
        self.signed = signed
        self.notarized = notarized
        self.stapled = stapled
        self.adhoc_names = adhoc_names
        self.team = team
        self.calls: list[list[str]] = []

    def __call__(self, argv) -> CommandResult:
        argv = list(argv)
        self.calls.append(argv)
        tool, target = argv[0], Path(argv[-1])
        if tool == "tar":
            with tarfile.open(argv[2]) as tar:
                tar.extractall(argv[4])
            return CommandResult(0, "")
        adhoc = not self.signed or target.name in self.adhoc_names
        if tool == "codesign" and "--verify" in argv:
            if not self.signed:
                return CommandResult(1, f"{target}: code object is not signed at all")
            return CommandResult(0, f"{target}: valid on disk")
        if tool == "codesign":
            details = ADHOC_DETAILS if adhoc else SIGNED_DETAILS.replace(TEAM, self.team)
            return CommandResult(0, details)
        if tool == "spctl":
            if not self.notarized:
                return CommandResult(3, f"{target}: rejected\nsource=Unnotarized Developer ID\n")
            return CommandResult(0, ACCEPTED.replace(TEAM, self.team))
        if argv[:3] == ["xcrun", "stapler", "validate"]:
            if not self.stapled:
                return CommandResult(65, f"{target.name} does not have a ticket stapled to it.")
            return CommandResult(0, "The validate action worked!")
        raise AssertionError(f"unexpected command {argv}")


def test_notarized_release_passes(tmp_path: Path) -> None:
    tools = FakeMacTools()
    report = verify_macos(make_darwin_artifacts(tmp_path / "a"), VERSION, run=tools)

    assert report.failures == []
    spctl = [call for call in tools.calls if call[0] == "spctl"]
    assert len(spctl) == 3
    assert all(call[:5] == ["spctl", "-a", "-vvv", "-t", "exec"] for call in spctl)
    deep = [call for call in tools.calls if "--deep" in call]
    assert all(Path(call[-1]).name == "CuaDriver.app" for call in deep) and len(deep) == 3
    # Every archive's standalone Mach-O copies are checked too.
    verified = {Path(call[-1]).name for call in tools.calls if "--verify" in call}
    assert {"cua-driver", "cua-cursor-theme", "libcua_driver_sdk.dylib"} <= verified
    assert "cua_driver_node_runtime.node" not in verified


def test_unsigned_release_like_0_28_3_fails(tmp_path: Path) -> None:
    report = verify_macos(
        make_darwin_artifacts(tmp_path / "a"), VERSION, run=FakeMacTools(signed=False)
    )

    assert any(
        "darwin-universal.tar.gz:CuaDriver.app: codesign --verify failed" in failure
        and "not signed at all" in failure
        for failure in report.failures
    )
    assert any("darwin-universal-binary.tar.gz:cua-driver" in f for f in report.failures)


@pytest.mark.parametrize(
    ("tools", "message"),
    [
        (FakeMacTools(notarized=False), "Gatekeeper source is not 'Notarized Developer ID'"),
        (FakeMacTools(stapled=False), "no valid stapled notarization ticket"),
        (FakeMacTools(team="ZZZZZZZZZZ"), "team identifier is not YCK386LBJ7"),
        (FakeMacTools(adhoc_names=("CuaDriver.app",)), "signature is ad hoc"),
        (FakeMacTools(adhoc_names=("libcua_driver_sdk.dylib",)), "hardened runtime is not enabled"),
    ],
)
def test_each_macos_signature_requirement_is_enforced(
    tmp_path: Path, tools: FakeMacTools, message: str
) -> None:
    report = verify_macos(make_darwin_artifacts(tmp_path / "a"), VERSION, run=tools)
    assert any(message in failure for failure in report.failures), report.failures


@pytest.mark.parametrize("omit", ["CuaDriver.app", "cua-driver", "libcua_driver_sdk.dylib"])
def test_missing_bundle_or_binary_fails(tmp_path: Path, omit: str) -> None:
    report = verify_macos(
        make_darwin_artifacts(tmp_path / "a", omit=omit), VERSION, run=FakeMacTools()
    )
    assert any(f": {omit} is missing" in failure or f"{omit} is missing" in failure for failure in report.failures)


def test_missing_darwin_archive_fails(tmp_path: Path) -> None:
    root = make_darwin_artifacts(tmp_path / "a")
    (root / f"cua-driver-rs-{VERSION}-darwin-x86_64.tar.gz").unlink()
    report = verify_macos(root, VERSION, run=FakeMacTools())
    assert f"missing darwin archive cua-driver-rs-{VERSION}-darwin-x86_64.tar.gz" in report.failures


def test_pkg_and_dmg_assets_must_be_notarized(tmp_path: Path) -> None:
    root = make_darwin_artifacts(tmp_path / "a")
    (root / "CuaDriver.pkg").write_bytes(b"pkg")
    (root / "CuaDriver.dmg").write_bytes(b"dmg")
    tools = FakeMacTools()
    assert verify_macos(root, VERSION, run=tools).failures == []
    assessments = [call[4] for call in tools.calls if call[0] == "spctl"]
    assert assessments.count("install") == 1 and assessments.count("open") == 1

    report = verify_macos(root, VERSION, run=FakeMacTools(notarized=False))
    assert any(f.startswith("CuaDriver.pkg:") for f in report.failures)
    assert any(f.startswith("CuaDriver.dmg:") for f in report.failures)


def make_windows_artifacts(root: Path, version: str = VERSION) -> Path:
    for arch in ("x86_64", "arm64"):
        stage = f"cua-driver-rs-{version}-windows-{arch}"
        folder = root / f"cua-driver-rs-windows-{arch}"
        folder.mkdir(parents=True, exist_ok=True)
        names = ("cua-driver.exe", "cua-cursor-theme.exe", "cua-driver-uia.exe", "cua_driver_sdk.dll", "cua_driver_node_runtime.node")
        with zipfile.ZipFile(folder / f"{stage}.zip", "w") as bundle:
            for name in names:
                bundle.writestr(f"{stage}/{name}", b"MZ")
            bundle.writestr(f"{stage}/LICENSE", b"license")
        with zipfile.ZipFile(folder / f"{stage}-binary.zip", "w") as bundle:
            for name in names:
                bundle.writestr(name, b"MZ")
    return root


class FakeAuthenticode:
    def __init__(self, overrides: dict[str, dict] | None = None) -> None:
        self.overrides = overrides or {}
        self.paths: list[str] = []

    def __call__(self, argv) -> CommandResult:
        argv = list(argv)
        assert Path(argv[-2]).name == "authenticode.ps1"
        assert "Get-AuthenticodeSignature -LiteralPath" in Path(argv[-2]).read_text()
        self.paths = Path(argv[-1]).read_text().split()
        rows = []
        for path in self.paths:
            row = {
                "Path": path,
                "Status": "Valid",
                "StatusMessage": "Signature verified.",
                "Subject": "CN=\"Cua AI, Inc.\", O=\"Cua AI, Inc.\", L=San Francisco, C=US",
                "Timestamped": True,
            }
            row.update(self.overrides.get(Path(path).name, {}))
            rows.append(row)
        return CommandResult(0, json.dumps(rows))


def test_signed_windows_release_passes(tmp_path: Path) -> None:
    tools = FakeAuthenticode()
    report = verify_windows(make_windows_artifacts(tmp_path / "a"), VERSION, run=tools)
    assert report.failures == []
    assert len(tools.paths) == 20
    assert all(Path(path).suffix in {".exe", ".dll", ".node"} for path in tools.paths)


@pytest.mark.parametrize(
    ("override", "message"),
    [
        ({"Status": "NotSigned", "StatusMessage": "The file is not signed."}, "status NotSigned"),
        ({"Status": "HashMismatch"}, "status HashMismatch"),
        ({"Subject": "CN=Someone Else"}, "unexpected signer CN=Someone Else"),
        ({"Timestamped": False}, "not timestamped"),
    ],
)
def test_each_windows_signature_requirement_is_enforced(
    tmp_path: Path, override: dict, message: str
) -> None:
    tools = FakeAuthenticode({"cua_driver_sdk.dll": override})
    report = verify_windows(make_windows_artifacts(tmp_path / "a"), VERSION, run=tools)
    assert len(report.failures) == 4
    assert all("cua_driver_sdk.dll" in f and message in f for f in report.failures)


def test_missing_windows_archive_fails(tmp_path: Path) -> None:
    root = make_windows_artifacts(tmp_path / "a")
    (root / "cua-driver-rs-windows-arm64" / f"cua-driver-rs-{VERSION}-windows-arm64-binary.zip").unlink()
    report = verify_windows(root, VERSION, run=FakeAuthenticode())
    assert report.failures == [f"missing Windows archive cua-driver-rs-{VERSION}-windows-arm64-binary.zip"]


def test_powershell_failure_fails_closed(tmp_path: Path) -> None:
    report = verify_windows(
        make_windows_artifacts(tmp_path / "a"),
        VERSION,
        run=lambda argv: CommandResult(1, "Get-AuthenticodeSignature is not available"),
    )
    assert report.failures and "Get-AuthenticodeSignature failed" in report.failures[0]


def test_cli_exit_status_and_annotations(tmp_path: Path, monkeypatch, capsys) -> None:
    root = make_darwin_artifacts(tmp_path / "a")
    monkeypatch.setattr(gate, "run_command", FakeMacTools())
    assert main(["macos", "--artifacts", str(root), "--version", VERSION]) == 0
    monkeypatch.setattr(gate, "run_command", FakeMacTools(signed=False))
    assert main(["macos", "--artifacts", str(root), "--version", VERSION]) == 1
    err = capsys.readouterr().err
    assert "::error title=Cua Driver release signature::" in err
    assert "must not be published or installed" in err
    assert main(["macos", "--artifacts", str(root), "--version", VERSION, "--no-annotations"]) == 1
    err = capsys.readouterr().err
    assert "::error" not in err
    assert "must not be published or installed" in err


def test_cli_fails_when_no_archive_matches(tmp_path: Path) -> None:
    (tmp_path / "empty").mkdir()
    assert main(["windows", "--artifacts", str(tmp_path / "empty"), "--version", VERSION]) == 1


REPO_ROOT = Path(__file__).resolve().parents[3]
EXPECT = REPO_ROOT / ".github/scripts/expect_verification_result.sh"


@pytest.mark.parametrize(
    ("expect", "status", "exit_code", "text"),
    [
        ("pass", "0", 0, "passed as required"),
        ("pass", "1", 1, "publication and installer baking are blocked"),
        ("fail", "1", 0, "rejected as expected"),
        ("fail", "0", 1, "cannot detect a broken release"),
        ("maybe", "0", 2, "unknown expected outcome"),
    ],
)
def test_expected_outcome_helper(expect: str, status: str, exit_code: int, text: str) -> None:
    result = subprocess.run(
        ["bash", str(EXPECT), expect, status, "macOS signatures"],
        capture_output=True,
        text=True,
    )
    assert result.returncode == exit_code
    assert text in result.stdout
