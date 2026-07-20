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


def test_tcc_reset_compares_live_designated_requirement() -> None:
    """The TCC-reset decision inspects the live app's requirement, not just the marker (#2230)."""
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "_install-local-rust.sh"
    ).read_text()

    # A classifier collapses a designated requirement to a stable identity class.
    assert "classify_designated_requirement()" in script
    # The old live requirement is captured before the app is replaced and drives
    # the comparison (so a Developer ID -> local transition is detected).
    assert "OLD_LIVE_IDENTITY=" in script
    assert 'OLD_IDENTITY="$OLD_LIVE_IDENTITY"' in script


def test_tcc_reset_gates_marker_write_on_success() -> None:
    """A failed `tccutil reset` must not advance the identity marker, so it retries (#2230)."""
    script = (
        Path(__file__).resolve().parents[2] / "scripts" / "_install-local-rust.sh"
    ).read_text()

    # The reset exit status is tracked instead of being discarded with `|| true`.
    assert "reset_succeeded=0" in script
    # The marker is only written when no reset was needed or the reset succeeded.
    assert '[ "$needs_reset" = 0 ] || [ "$reset_succeeded" = 1 ]' in script


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
