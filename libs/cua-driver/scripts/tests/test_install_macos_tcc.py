from __future__ import annotations

import re
import subprocess
from pathlib import Path


INSTALLER = Path(__file__).resolve().parents[1] / "_install-rust.sh"


def extract_shell_function(name: str) -> str:
    source = INSTALLER.read_text()
    match = re.search(
        rf"(?ms)^{re.escape(name)}\(\) \{{\n.*?^\}}\n",
        source,
    )
    assert match, f"could not find shell function {name}"
    return match.group(0)


def run_policy(body: str) -> subprocess.CompletedProcess[str]:
    functions = "\n".join(
        extract_shell_function(name)
        for name in (
            "macos_requirement_compatibility",
            "macos_reset_tcc_after_requirement_change",
        )
    )
    return subprocess.run(
        ["/bin/bash", "-c", f"set -euo pipefail\n{functions}\n{body}"],
        check=False,
        capture_output=True,
        text=True,
    )


def test_semantically_compatible_requirement_preserves_tcc_rows() -> None:
    result = run_policy(
        r'''
        err() { printf 'error: %s\n' "$*" >&2; }
        log() { printf 'log: %s\n' "$*"; }
        codesign() {
            [[ "$1" == "--verify" ]]
            [[ "$4" == '-R' ]]
            [[ "$5" == '=identifier "com.trycua.driver" and anchor apple generic' ]]
            [[ "$6" == '/replacement.app' ]]
        }
        tccutil() { echo unexpected >&2; return 99; }
        compatibility="$(macos_requirement_compatibility \
            'identifier "com.trycua.driver" and anchor apple generic' \
            /replacement.app)"
        [[ "$compatibility" == compatible ]]
        macos_reset_tcc_after_requirement_change "$compatibility"
        '''
    )

    assert result.returncode == 0, result.stderr
    assert "unexpected" not in result.stderr


def test_incompatible_requirement_resets_only_driver_permissions() -> None:
    result = run_policy(
        r'''
        err() { printf 'error: %s\n' "$*" >&2; }
        log() { printf 'log: %s\n' "$*"; }
        codesign() { return 3; }
        calls=""
        tccutil() { calls="${calls}${1}:${2}:${3}"$'\n'; }
        compatibility="$(macos_requirement_compatibility 'old requirement' /replacement.app)"
        [[ "$compatibility" == incompatible ]]
        macos_reset_tcc_after_requirement_change "$compatibility"
        printf '%s' "$calls"
        '''
    )

    assert result.returncode == 0, result.stderr
    assert result.stdout == (
        "log: the app signing requirement changed; cleared stale Accessibility and Screen Recording rows\n"
        "log: macOS authorization is required again: cua-driver permissions grant\n"
        "reset:Accessibility:com.trycua.driver\n"
        "reset:ScreenCapture:com.trycua.driver\n"
    )


def test_unknown_previous_requirement_does_not_destroy_grants() -> None:
    result = run_policy(
        r'''
        err() { printf 'error: %s\n' "$*" >&2; }
        log() { printf 'log: %s\n' "$*"; }
        codesign() { echo unexpected >&2; return 99; }
        tccutil() { echo unexpected >&2; return 99; }
        compatibility="$(macos_requirement_compatibility '' /replacement.app)"
        [[ "$compatibility" == unknown ]]
        macos_reset_tcc_after_requirement_change "$compatibility"
        '''
    )

    assert result.returncode == 0, result.stderr
    assert "unexpected" not in result.stderr


def test_requirement_evaluation_error_does_not_destroy_grants() -> None:
    result = run_policy(
        r'''
        err() { printf 'error: %s\n' "$*" >&2; }
        log() { printf 'log: %s\n' "$*"; }
        codesign() { echo 'malformed requirement' >&2; return 1; }
        tccutil() { echo unexpected >&2; return 99; }
        compatibility="$(macos_requirement_compatibility 'malformed' /replacement.app)"
        [[ "$compatibility" == unknown ]]
        macos_reset_tcc_after_requirement_change "$compatibility"
        '''
    )

    assert result.returncode == 0, result.stderr
    assert "could not evaluate the previous code-signing requirement" in result.stderr
    assert "unexpected" not in result.stderr


def test_reset_failure_is_actionable_and_returns_failure() -> None:
    result = run_policy(
        r'''
        err() { printf 'error: %s\n' "$*" >&2; }
        log() { printf 'log: %s\n' "$*"; }
        tccutil() { [[ "$2" != ScreenCapture ]]; }
        if macos_reset_tcc_after_requirement_change incompatible; then
            exit 90
        fi
        '''
    )

    assert result.returncode == 0, result.stderr
    assert "could not reset these TCC services" in result.stderr
    assert "tccutil reset Accessibility com.trycua.driver" in result.stderr
    assert "tccutil reset ScreenCapture com.trycua.driver" in result.stderr


def test_installer_verifies_then_registers_before_any_tcc_reset() -> None:
    source = INSTALLER.read_text()
    install = source.index('if [[ "$OS" == "Darwin" && -n "$SRC_APP"')
    staged_verify = source.index('codesign --verify --deep --strict "$SRC_APP"', install)
    stop_daemon = source.index("stop_cua_driver_daemons", staged_verify)
    backup = source.index('mv "$APP_DEST" "$APP_BACKUP"', staged_verify)
    copy = source.index('ditto "$SRC_APP" "$APP_DEST"', backup)
    installed_verify = source.index('codesign --verify --deep --strict "$APP_DEST"', copy)
    register = source.index('"$LSREGISTER" -f "$APP_DEST"', installed_verify)
    reset = source.index("macos_reset_tcc_after_requirement_change", register)

    assert staged_verify < stop_daemon < backup < copy < installed_verify < register < reset


def test_only_the_release_bundle_identity_can_trigger_a_tcc_reset() -> None:
    source = INSTALLER.read_text()

    assert 'STAGED_BUNDLE_ID' in source
    assert 'STAGED_BUNDLE_ID" != "com.trycua.driver"' in source
    assert '[[ "$PREV_BUNDLE_ID" == "com.trycua.driver" ]]' in source
    assert 'INSTALLED_BUNDLE_ID" == "$STAGED_BUNDLE_ID"' in source
