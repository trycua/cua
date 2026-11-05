"""Verify the reset of NA-04: files as in the fixture; only the treasurer's report locked; one comment."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, finder  # noqa: E402

META = common.read_json(BUNDLE / "reset" / "meta.json")


def files(workspace: Path) -> list[str]:
    folder = workspace / META["folder"]
    problems = []
    for item in sorted(folder.iterdir()):
        if finder.locked(item) != (item.name in META["locked"]):
            problems.append(f"{item.name}: locked {finder.locked(item)}")
        if finder.comment(item) != META["comments"].get(item.name, ""):
            problems.append(f"{item.name}: comment {finder.comment(item)!r}")
    return problems


if __name__ == "__main__":
    common.app_verify_main(BUNDLE, "NA-04", files, None)
