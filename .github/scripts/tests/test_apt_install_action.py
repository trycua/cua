"""The apt-install action must not depend on a single Ubuntu mirror."""
from pathlib import Path

ACTION = Path(__file__).resolve().parents[2] / "actions" / "apt-install" / "action.yml"


def test_falls_back_to_the_canonical_archive_after_the_first_failure():
    text = ACTION.read_text()
    assert 'if [ "$attempt" = 1 ]; then' in text
    assert "azure\\.archive\\.ubuntu\\.com" in text
    assert "http://archive.ubuntu.com/ubuntu/" in text
