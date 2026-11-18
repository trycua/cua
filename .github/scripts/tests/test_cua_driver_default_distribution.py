from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/cd-rust-cua-driver.yml"
PERCEPTION_WORKFLOW = ROOT / ".github/workflows/ci-cua-perception-release.yml"
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

REQUIRED_DRIVER_PACKAGES = frozenset(
    ("cua-driver", "cursor-theme-cli", "cua-driver-sdk")
)
ALLOWED_BUILD_PACKAGE_SETS = (
    REQUIRED_DRIVER_PACKAGES,
    REQUIRED_DRIVER_PACKAGES | {"cua-driver-uia"},
)


def _cargo_build_packages(command: str) -> frozenset[str]:
    return frozenset(
        match.group(1)
        for match in re.finditer(r"(?:^|\s)(?:-p|--package)(?:\s+|=)([\w-]+)", command)
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
    for command in build_commands:
        assert not re.search(r"(?:^|\s)(?:--workspace|--all)(?:\s|$)", command)
        packages = _cargo_build_packages(command)
        assert REQUIRED_DRIVER_PACKAGES <= packages
        assert packages in ALLOWED_BUILD_PACKAGE_SETS
    assert all(
        token not in line.lower()
        for line in package_commands
        for token in FORBIDDEN_DISTRIBUTION_TOKENS
    )


def test_dependency_graph_gate_runs_for_driver_manifest_changes() -> None:
    source = PERCEPTION_WORKFLOW.read_text()

    assert source.count('      - "libs/cua-driver/rust/**/Cargo.toml"') == 2
    assert source.count('      - "libs/cua-driver/rust/Cargo.lock"') == 2
    assert "cargo tree --locked -p cua-driver --edges normal,build --prefix none" in source
    assert "grep -q '^cua-perception v'" in source
    assert ".github/scripts/tests/test_cua_driver_default_distribution.py" in source
    assert ".github/scripts/tests/test_verify_cua_driver_release_archives.py" in source


def test_ordinary_installers_have_no_optional_perception_download_path() -> None:
    for installer in (UNIX_INSTALLER, WINDOWS_INSTALLER):
        source = installer.read_text(encoding="utf-8-sig").lower()
        assert all(token not in source for token in FORBIDDEN_DISTRIBUTION_TOKENS), installer

    unix = UNIX_INSTALLER.read_text()
    windows = WINDOWS_INSTALLER.read_text(encoding="utf-8-sig")
    assert "releases/download/${TAG}/$tarball" in unix
    assert '"cua-driver-rs-$version-$archLabel.zip"' in windows
    assert "releases/download/$Script:CuaDriverRsReleaseTag/$zipName" in windows
