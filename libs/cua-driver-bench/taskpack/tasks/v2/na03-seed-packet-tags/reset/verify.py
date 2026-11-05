"""Verify the reset of NA-03: files as in the fixture, tags as listed in reset/tags.json and nothing else."""

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, finder  # noqa: E402

TAGS = common.read_json(BUNDLE / "reset" / "tags.json")


def files(workspace: Path) -> list[str]:
    folder = workspace / TAGS["folder"]
    problems = []
    for item in sorted(folder.iterdir()):
        want = set(TAGS["initial"].get(item.name, []))
        if finder.tags(item) != want:
            problems.append(f"{item.name}: tags {sorted(finder.tags(item))}")
    return problems


if __name__ == "__main__":
    common.app_verify_main(BUNDLE, "NA-03", files, None)
