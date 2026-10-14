"""Keep TypeSafe provider credentials completely outside GitHub Actions."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
ACTION_SURFACES = (ROOT / ".github/workflows", ROOT / ".github/actions")


def test_actions_never_reference_the_typesafe_credential() -> None:
    credential_name = "TYPESAFE" + "_API_KEY"
    matches = []
    for surface in ACTION_SURFACES:
        if not surface.exists():
            continue
        for path in sorted(surface.rglob("*")):
            if path.is_file() and credential_name in path.read_text(encoding="utf-8"):
                matches.append(path.relative_to(ROOT))
    assert matches == []
