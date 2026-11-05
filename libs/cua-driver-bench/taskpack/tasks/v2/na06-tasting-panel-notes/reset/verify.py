"""Verify the reset of NA-06 (in the VM: the folder holds exactly the two fixture notes)."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, mac  # noqa: E402

STATE = common.read_json(BUNDLE / "reset" / "app_state.json")


def lines(text: str) -> list[str]:
    return [" ".join(l.split()) for l in (text or "").splitlines() if l.strip()]


def app_state(workspace: Path) -> list[str]:
    return _own(workspace) + mac.notes_strays(list(STATE["create"]), STATE["purge"])


def _own(workspace: Path) -> list[str]:
    data = mac.notes_export(list(STATE["create"]))
    got = sorted((n["name"], tuple(lines(n["plaintext"]))) for n in data["notes"])
    want = sorted((k, tuple(v)) for k, v in STATE["lines"].items())
    return [] if got == want and data["folders"].count("Tasting panel") == 1 else [f"notes {got!r}"]


if __name__ == "__main__":
    common.app_verify_main(BUNDLE, "NA-06", None, app_state)
