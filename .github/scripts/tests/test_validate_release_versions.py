from pathlib import Path
import re
import shutil

import pytest

from validate_release_versions import VersionError, validate


REPO_ROOT = Path(__file__).resolve().parents[3]


def copy_release_sources(destination: Path) -> None:
    shutil.copy(REPO_ROOT / ".release-please-manifest.json", destination)
    release_state = ".github/release-state/cua-driver-rs-published-version"
    (destination / release_state).parent.mkdir(parents=True, exist_ok=True)
    shutil.copy(REPO_ROOT / release_state, destination / release_state)
    for relative in ("libs/cua-driver", "libs/lume"):
        source = REPO_ROOT / relative
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, target)
    for product in ("cua-driver", "lume"):
        docs = f"docs/content/docs/reference/{product}"
        shutil.copytree(REPO_ROOT / docs, destination / docs)


def test_current_release_versions_agree():
    validate(REPO_ROOT, "all")


def test_version_drift_fails_with_the_source_name(tmp_path: Path):
    copy_release_sources(tmp_path)
    path = tmp_path / "libs/lume/src/Main.swift"
    current = (tmp_path / "libs/lume/VERSION").read_text().strip()
    path.write_text(path.read_text().replace(f'"{current}"', '"9.9.9"'))
    with pytest.raises(VersionError, match="src/Main.swift=9.9.9"):
        validate(tmp_path, "lume")


def set_driver_installer_versions(
    root: Path, *, shell_version: str, powershell_version: str
) -> None:
    state = root / ".github/release-state/cua-driver-rs-published-version"
    state.write_text(f"{shell_version}\n")
    shell = root / "libs/cua-driver/scripts/_install-rust.sh"
    shell_source = shell.read_text()
    shell.write_text(
        re.sub(
            r'^CUA_DRIVER_RS_BAKED_VERSION="[^"]+"',
            f'CUA_DRIVER_RS_BAKED_VERSION="{shell_version}"',
            shell_source,
            count=1,
            flags=re.MULTILINE,
        )
    )
    powershell = root / "libs/cua-driver/scripts/install.ps1"
    powershell_source = powershell.read_text()
    powershell.write_text(
        re.sub(
            r'^\$Script:CuaDriverRsBakedVersion\s*=\s*"[^"]+"',
            f'$Script:CuaDriverRsBakedVersion = "{powershell_version}"',
            powershell_source,
            count=1,
            flags=re.MULTILINE,
        )
    )


def test_driver_validation_allows_installer_to_lag_until_publication(tmp_path: Path):
    copy_release_sources(tmp_path)
    set_driver_installer_versions(
        tmp_path, shell_version="0.0.0", powershell_version="0.0.0"
    )

    validate(tmp_path, "driver")


def test_driver_validation_rejects_disagreeing_installer_versions(tmp_path: Path):
    copy_release_sources(tmp_path)
    set_driver_installer_versions(
        tmp_path, shell_version="0.0.0", powershell_version="0.0.1"
    )

    with pytest.raises(VersionError, match="baked installers"):
        validate(tmp_path, "driver")


def test_driver_validation_rejects_installer_version_ahead_of_source(tmp_path: Path):
    copy_release_sources(tmp_path)
    set_driver_installer_versions(
        tmp_path, shell_version="9.9.9", powershell_version="9.9.9"
    )

    with pytest.raises(VersionError, match="ahead of source release"):
        validate(tmp_path, "driver")
