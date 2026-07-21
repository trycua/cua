from pathlib import Path


def test_local_installer_accepts_untrusted_self_signed_identity() -> None:
    """The installer-created self-signed cert is usable even before trust-chain validation."""
    script = (
        Path(__file__).resolve().parents[2]
        / "scripts"
        / "_install-local-rust.sh"
    ).read_text()

    assert "security find-identity -v -p codesigning" not in script
    assert script.count("security find-identity -p codesigning") >= 2


def test_local_installer_uses_a_separate_macos_identity() -> None:
    """Local rebuilds never replace or reset the release app identity (#2230)."""
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "_install-local-rust.sh"
    ).read_text()

    assert 'APP_DEST="/Applications/CuaDriverLocal.app"' in script
    assert 'CFBundleIdentifier -string "com.trycua.driver.local"' in script
    assert 'CFBundleExecutable -string "cua-driver-local"' in script
    assert "tccutil reset" not in script


def test_unix_local_installer_uses_separate_paths_and_autostart() -> None:
    """Local install state, command, and service names coexist with release."""
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "_install-local-rust.sh"
    ).read_text()

    assert 'HOME_DIR="${CUA_DRIVER_LOCAL_HOME:-$HOME/.cua-driver-local}"' in script
    assert 'BIN_DIR/cua-driver-local' in script
    assert "com.trycua.cua-driver-local.plist" in script
    assert "cua-driver-local.service" in script
    assert "stop_cua_driver_daemons" not in script


def test_unix_local_installer_always_embeds_source_provenance() -> None:
    """Local builds derive Git provenance but preserve VM snapshot overrides."""
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "_install-local-rust.sh"
    ).read_text()

    assert 'if [ -z "${CUA_DRIVER_SOURCE_SHA:-}" ]; then' in script
    assert "rev-parse --verify 'HEAD^{commit}'" in script
    assert "status --porcelain --untracked-files=normal" in script
    assert 'CUA_DRIVER_SOURCE_SHA="${CUA_DRIVER_SOURCE_SHA}-dirty"' in script
    assert "export CUA_DRIVER_SOURCE_SHA" in script


def test_windows_local_installer_always_embeds_source_provenance() -> None:
    """The Windows developer installer follows the same provenance contract."""
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "install-local.ps1"
    ).read_text()

    assert "IsNullOrWhiteSpace($env:CUA_DRIVER_SOURCE_SHA)" in script
    assert "rev-parse --verify 'HEAD^{commit}'" in script
    assert "status --porcelain --untracked-files=normal" in script
    assert '"$detectedSourceSha-dirty"' in script


def test_windows_local_installer_uses_separate_paths_and_autostart() -> None:
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "install-local.ps1"
    ).read_text()

    assert '$BinaryName  = "cua-driver-local.exe"' in script
    assert '"Programs\\Cua\\cua-driver-local\\bin"' in script
    assert '".cua-driver-local"' in script
    assert '"cua-driver-local-serve"' in script
    assert "Repair-CuaDriverStaleDaemon" not in script


def test_release_installers_do_not_target_local_product_artifacts() -> None:
    scripts_dir = Path(__file__).resolve().parents[2] / "scripts"
    for name in ("_install-rust.sh", "install.ps1"):
        script = (scripts_dir / name).read_text()
        assert "CuaDriverLocal" not in script
        assert ".cua-driver-local" not in script
        assert "cua-driver-local-serve" not in script
