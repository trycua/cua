from pathlib import Path
import os
import subprocess


SCRIPTS_DIR = Path(__file__).resolve().parents[2] / "scripts"
SIGNING_HELPER = SCRIPTS_DIR / "_local-signing.sh"


def run_signing_policy(shell_body: str) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env["SIGNING_HELPER"] = str(SIGNING_HELPER)
    return subprocess.run(
        ["/bin/bash", "-c", 'set -euo pipefail; source "$SIGNING_HELPER"; ' + shell_body],
        check=False,
        capture_output=True,
        text=True,
        env=env,
    )


def test_local_installer_accepts_untrusted_self_signed_identity() -> None:
    """The installer-created self-signed cert is usable even before trust-chain validation."""
    script = (SIGNING_HELPER).read_text()

    assert "security find-identity -v -p codesigning" not in script
    assert script.count("security find-identity -p codesigning") >= 2


def test_strict_local_signing_fails_before_ad_hoc_fallback() -> None:
    result = run_signing_policy(
        r"""
        OS=Darwin
        RED= GREEN= YELLOW= NORMAL=
        CUA_DRIVER_REQUIRE_STABLE_SIGNING=1
        ensure_local_signing_identity() { printf '%s' -; }
        clean_partial_bundle_signature() { :; }
        codesign_bounded() { echo "unexpected ad-hoc signing" >&2; return 99; }
        if sign_staged_local_app /staged.app /live.app; then exit 90; fi
        """
    )

    assert result.returncode == 0, result.stderr
    assert "stable macOS signing is required" in result.stderr
    assert "live installation was not changed" in result.stderr
    assert "--require-stable-signing" in result.stderr
    assert "unexpected ad-hoc signing" not in result.stderr


def test_ad_hoc_fallback_is_prominent_and_reports_cdhash() -> None:
    result = run_signing_policy(
        r"""
        OS=Darwin
        RED= GREEN= YELLOW= NORMAL=
        CUA_DRIVER_REQUIRE_STABLE_SIGNING=0
        ensure_local_signing_identity() { printf '%s' -; }
        clean_partial_bundle_signature() { :; }
        codesign_bounded() { return 0; }
        codesign() {
            if [ "$1" = "-d" ]; then
                echo '# designated => cdhash H"0123456789ABCDEF"'
            fi
            return 0
        }
        sign_staged_local_app /staged.app /missing-live.app
        """
    )

    assert result.returncode == 0, result.stderr
    assert "WARNING: CuaDriverLocal.app was signed ad-hoc" in result.stderr
    assert "WILL become invalid on the next rebuild" in result.stderr
    assert "designated requirement uses cdhash" in result.stderr


def test_certificate_requirement_is_classified_as_stable() -> None:
    result = run_signing_policy(
        r"""
        OS=Darwin
        RED= GREEN= YELLOW= NORMAL=
        CUA_DRIVER_REQUIRE_STABLE_SIGNING=1
        ensure_local_signing_identity() { printf '%s' ABCDEF; }
        codesign_bounded() { return 0; }
        codesign() {
            if [ "$1" = "-d" ]; then
                echo 'designated => anchor trusted and certificate leaf[subject.CN] = "CuaDriver Local Signing (cua-driver-rs)"' >&2
            fi
            return 0
        }
        sign_staged_local_app /staged.app /missing-live.app
        """
    )

    assert result.returncode == 0, result.stderr
    assert "stable local identity" in result.stdout


def test_installer_verifies_the_copied_designated_requirement() -> None:
    script = (SCRIPTS_DIR / "_install-local-rust.sh").read_text()

    assert 'INSTALLED_REQUIREMENT="$(designated_requirement "$APP_DEST")"' in script
    assert '[ "$INSTALLED_REQUIREMENT" = "$STAGED_REQUIREMENT" ]' in script
    assert "verified installed designated requirement: certificate-backed" in script
    assert "verified installed designated requirement: ad-hoc cdhash" in script


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
    assert "BIN_DIR/cua-driver-local" in script
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
    script = (Path(__file__).resolve().parents[2] / "scripts" / "install-local.ps1").read_text()

    assert "IsNullOrWhiteSpace($env:CUA_DRIVER_SOURCE_SHA)" in script
    assert "rev-parse --verify 'HEAD^{commit}'" in script
    assert "status --porcelain --untracked-files=normal" in script
    assert '"$detectedSourceSha-dirty"' in script


def test_windows_local_installer_uses_separate_paths_and_autostart() -> None:
    script = (Path(__file__).resolve().parents[2] / "scripts" / "install-local.ps1").read_text()

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
