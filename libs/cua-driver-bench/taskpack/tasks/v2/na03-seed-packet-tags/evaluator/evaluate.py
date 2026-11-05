"""Evaluate NA-03: Red on tomato and squash packets, Green on bean and pea packets, Yellow off the lettuce."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, finder  # noqa: E402

TASK_ID = "NA-03"
TAGS = common.read_json(BUNDLE / "reset" / "tags.json")
RED = ["tomato-amberlight.txt", "tomato-red-ember.txt", "squash-moonbowl.txt", "squash-ridgeback.txt"]
GREEN = ["bean-cobble-runner.txt", "bean-violet-pod.txt", "pea-tallow-sugar.txt"]
LETTUCE = "lettuce-frill-mint.txt"


def evaluate(workspace: Path, checks: common.Checks) -> None:
    folder = workspace / TAGS["folder"]
    fixture = BUNDLE / "fixture" / TAGS["folder"]
    present = {p.name for p in folder.iterdir() if p.name != ".DS_Store"} if folder.is_dir() else set()

    def got(name: str) -> set[str]:
        return finder.tags(folder / name) if name in present else set()

    checks.add("red_tags", all(got(n) == {"Red"} for n in RED), {n: sorted(got(n)) for n in RED})
    checks.add("green_tags", all(got(n) == {"Green"} for n in GREEN), {n: sorted(got(n)) for n in GREEN})
    checks.add("lettuce_yellow_removed", got(LETTUCE) == {"Blue"}, sorted(got(LETTUCE)))
    others = [p.name for p in sorted(fixture.iterdir()) if p.name not in RED + GREEN + [LETTUCE]]
    changed = {n: sorted(got(n)) for n in others if got(n) != set(TAGS["initial"].get(n, []))}
    checks.add("other_tags_unchanged", not changed, changed, kind="preservation")
    want_files = {p.name: p.read_bytes() for p in fixture.iterdir()}
    same = present == set(want_files) and all((folder / n).read_bytes() == b for n, b in want_files.items())
    checks.add("files_unchanged", same, f"missing {sorted(set(want_files) - present)}, extra {sorted(present - set(want_files))}",
               kind="preservation")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
