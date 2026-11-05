"""Reset NA-03: the seed-packet folder with its starting Finder tags (and, in the VM, no Finder windows)."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, finder, mac  # noqa: E402

TAGS = common.read_json(BUNDLE / "reset" / "tags.json")


def files(workspace: Path) -> None:
    for name, names in TAGS["initial"].items():
        finder.set_tags(workspace / TAGS["folder"] / name, names)


def app_state(workspace: Path) -> None:
    mac.finder_close_windows()


if __name__ == "__main__":
    common.app_reset_main(BUNDLE, "NA-03", files, app_state)
