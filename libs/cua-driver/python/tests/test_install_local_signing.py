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
