from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
MACOS_ENTRY = ROOT / ".github/workflows/e2e-rust-macos.yml"


def test_direct_lume_certification_stays_outside_actions() -> None:
    text = MACOS_ENTRY.read_text(encoding="utf-8")
    assert "run-all.sh --standalone-browser" not in text
    assert "authorized-live-jev-macos-evidence.yml" not in text
