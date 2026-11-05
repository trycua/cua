"""Reset NA-06: the workspace, and (in the VM) the Notes folder "Tasting panel" with its two notes."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, mac  # noqa: E402

STATE = common.read_json(BUNDLE / "reset" / "app_state.json")


def app_state(workspace: Path) -> None:
    mac.quit_app("Notes")
    mac.notes_reset(STATE["drop"], STATE["create"], STATE["purge"])


if __name__ == "__main__":
    common.app_reset_main(BUNDLE, "NA-06", None, app_state)
