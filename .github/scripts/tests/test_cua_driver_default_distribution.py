from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/cd-rust-cua-driver.yml"
UNIX_INSTALLER = ROOT / "libs/cua-driver/scripts/_install-rust.sh"
WINDOWS_INSTALLER = ROOT / "libs/cua-driver/scripts/install.ps1"

FORBIDDEN_DISTRIBUTION_TOKENS = (
    "cua-perception",
    "onnxruntime",
    ".onnx",
    ".ort",
    "signed-catalog",
    "catalog-payload",
    "agpl",
)


def test_driver_package_commands_only_stage_driver_runtime() -> None:
    source = WORKFLOW.read_text()
    build_commands = [line for line in source.splitlines() if "cargo build" in line]
    package_commands = [
        line
        for line in source.splitlines()
        if re.search(r"\b(cp|tar|Copy-Item|Compress-Archive)\b", line)
    ]

    assert build_commands
    assert package_commands
    assert all("-p cua-perception" not in line for line in build_commands)
    assert all(
        token not in line.lower()
        for line in package_commands
        for token in FORBIDDEN_DISTRIBUTION_TOKENS
    )


def test_ordinary_installers_have_no_optional_perception_download_path() -> None:
    for installer in (UNIX_INSTALLER, WINDOWS_INSTALLER):
        source = installer.read_text(encoding="utf-8-sig").lower()
        assert all(token not in source for token in FORBIDDEN_DISTRIBUTION_TOKENS), installer

    unix = UNIX_INSTALLER.read_text()
    windows = WINDOWS_INSTALLER.read_text(encoding="utf-8-sig")
    assert "releases/download/${TAG}/$tarball" in unix
    assert '"cua-driver-rs-$version-$archLabel.zip"' in windows
    assert "releases/download/$Script:CuaDriverRsReleaseTag/$zipName" in windows
