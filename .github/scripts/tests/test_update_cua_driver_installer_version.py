from pathlib import Path

import pytest

from update_cua_driver_installer_version import (
    InstallerVersionError,
    POWERSHELL_VERSION,
    SHELL_VERSION,
    read_version,
    update_installer_versions,
)


REPO_ROOT = Path(__file__).resolve().parents[3]


def copy_installers(tmp_path: Path) -> tuple[Path, Path, Path]:
    source = REPO_ROOT / "libs/cua-driver/scripts"
    shell = tmp_path / "_install-rust.sh"
    powershell = tmp_path / "install.ps1"
    shell.write_bytes((source / shell.name).read_bytes())
    powershell.write_bytes((source / powershell.name).read_bytes())
    state = tmp_path / "published-version"
    state.write_bytes(
        (
            REPO_ROOT
            / ".github/release-state/cua-driver-rs-published-version"
        ).read_bytes()
    )
    return shell, powershell, state


def test_updates_both_installer_sentinels_and_is_idempotent(tmp_path: Path) -> None:
    shell, powershell, state = copy_installers(tmp_path)

    changed = update_installer_versions(
        shell, powershell, "999.998.997", state_path=state
    )

    assert changed == (shell, powershell, state)
    assert read_version(shell, SHELL_VERSION) == "999.998.997"
    assert read_version(powershell, POWERSHELL_VERSION) == "999.998.997"
    assert state.read_text() == "999.998.997\n"
    assert update_installer_versions(
        shell, powershell, "999.998.997", state_path=state
    ) == ()


def test_refuses_to_downgrade_baked_installers(tmp_path: Path) -> None:
    shell, powershell, state = copy_installers(tmp_path)
    update_installer_versions(
        shell, powershell, "999.998.997", state_path=state
    )

    with pytest.raises(InstallerVersionError, match="refusing to move"):
        update_installer_versions(
            shell, powershell, "999.998.996", state_path=state
        )

    assert update_installer_versions(
        shell,
        powershell,
        "999.998.996",
        state_path=state,
        allow_newer=True,
    ) == ()
    assert read_version(shell, SHELL_VERSION) == "999.998.997"
    assert read_version(powershell, POWERSHELL_VERSION) == "999.998.997"


@pytest.mark.parametrize("version", ["v1.2.3", "1.2", "1.2.3-rc.1", "latest"])
def test_rejects_non_stable_versions(tmp_path: Path, version: str) -> None:
    shell, powershell, _ = copy_installers(tmp_path)

    with pytest.raises(InstallerVersionError, match="exact stable"):
        update_installer_versions(shell, powershell, version)


def test_refuses_mismatched_installer_state(tmp_path: Path) -> None:
    shell, powershell, state = copy_installers(tmp_path)
    shell.write_text(
        SHELL_VERSION.sub(r"\g<1>1.2.3\g<3>", shell.read_text(), count=1)
    )

    with pytest.raises(InstallerVersionError, match="versions disagree"):
        update_installer_versions(
            shell, powershell, "999.998.997", state_path=state
        )


def test_accepts_legacy_release_please_sentinels_for_tag_recovery(
    tmp_path: Path,
) -> None:
    shell, powershell, _ = copy_installers(tmp_path)
    shell.write_text(
        shell.read_text().replace(
            "# published-installer-version", "# x-release-please-version"
        )
    )
    powershell.write_text(
        powershell.read_text().replace(
            "# published-installer-version", "# x-release-please-version"
        )
    )

    changed = update_installer_versions(shell, powershell, "999.998.997")

    assert changed == (shell, powershell)
    assert "# x-release-please-version" in shell.read_text()
    assert "# x-release-please-version" in powershell.read_text()
