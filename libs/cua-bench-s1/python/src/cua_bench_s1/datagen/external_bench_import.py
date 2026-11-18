"""[fstandhartinger/jevbench](https://github.com/fstandhartinger/jevbench)
(MIT license) public dataset -> `CuaTask`-shaped object converter.

This does NOT feed one of this benchmark's GUI task families (jevbench's own
`Task` schema is pure text -- no screenshots, no elements, no GUI actions at
all), so this module lives in `datagen/` but its output is used only as an
OUT-OF-DOMAIN generalization axis, never mixed into `task.py`'s `FAMILIES`
GUI families.

jevbench's own `Task` (see its `jevbench/` package, and the public JSONL at
`datasets/public/{easy,hard,original}.jsonl`) is: `state` (a text scenario),
`question` (`instructions` + `criteria` describing what each label means),
`labels` (a fixed, closed list of answer strings -- 2 to 6 in the public
set), `expected` (the gold label), plus `family`/`group`/`split`/
`provenance`. This is the same "typed bounded-decision" pattern `task.py`'s
own module docstring says `CuaTask` generalizes -- so converting it back is a
short, honest round trip, not a stretch.

## Mapping onto CuaTask's (element, action) schema

Same convention as `game_control`/`chess`: each jevbench answer label becomes
one synthetic "Button" element (`role="Button"`, `label=<the label text>`,
no `frame` -- there is no image at all for these tasks). Every element
offers `skip`; only the element matching `expected` also offers `click`.
This works regardless of how many labels a question has (jevbench tops out
at 6 in the public set, but nothing here assumes that), unlike a
single-element-with-N-actions design which would be capped at
`len(task.ACTIONS) == 6` distinct actions.

`ax_tree` (the ONLY representation these tasks get -- `modality_available =
["text"]`, no screenshot) is a plain rendering of jevbench's own `state` +
`question.instructions` + `question.criteria`, so any text-only scoring
adapter implementing this package's `ModelAdapter` interface can read it
exactly like any other CuaTask's ax_tree, with no special-casing needed.

This is a real external benchmark's real published dataset, used honestly as
a held-out, out-of-domain generalization axis (a "does a GUI-trained model's
typed-bounded-decision skill transfer to a pure-text decision task" probe),
not a claim that this project is derived from or a replacement for it.
"""
from __future__ import annotations

import json
from pathlib import Path

from ..task import CuaTask, OptionSpec

EXTERNAL_BENCH_LICENSE = "MIT"
EXTERNAL_BENCH_SOURCE_URL = "https://github.com/fstandhartinger/jevbench"
EXTERNAL_BENCH_NAME = "fstandhartinger/jevbench"


def _ax_tree_text(state: str, question: dict) -> str:
    criteria = question.get("criteria") or {}
    # jevbench's own `criteria` field is a dict for most question types
    # (label -> description) but a bare list for others (e.g. `ordinal`) --
    # both are real jevbench shapes, handled honestly rather than assuming one.
    if isinstance(criteria, dict):
        criteria_lines = "\n".join(f"- {k}: {v}" for k, v in criteria.items())
    else:
        criteria_lines = "\n".join(f"- {c}" for c in criteria)
    parts = [f"Scenario:\n{state}", f"\nInstructions:\n{question.get('instructions', '')}"]
    if criteria_lines:
        parts.append(f"\nCriteria:\n{criteria_lines}")
    return "\n".join(parts)


def convert_external_bench_task(d: dict) -> CuaTask:
    """Converts one raw jevbench JSONL record (already `json.loads`-ed) into
    a `CuaTask`. Raises `ValueError` if `expected` isn't one of `labels`
    (would indicate a corrupt/misread source record, not something to
    silently paper over)."""
    labels: list[str] = [str(l) for l in d["labels"]]
    expected: str = str(d["expected"])
    if expected not in labels:
        raise ValueError(f"jevbench task {d['id']}: expected {expected!r} not in labels {labels!r}")

    elements, options, expected_map = [], [], {}
    for i, label in enumerate(labels):
        eid = f"opt_{i}"
        elements.append({"id": eid, "role": "Button", "label": label, "frame": None})
        options.append(OptionSpec(element_id=eid, role="Button", label=label, action="skip"))
        if label == expected:
            options.append(OptionSpec(element_id=eid, role="Button", label=label, action="click"))
            expected_map[eid] = "click"
        else:
            expected_map[eid] = "skip"

    return CuaTask(
        id=f"extbench_{d['id']}",
        family="general_decision",
        app="external_bench",
        modality_available=["text"],
        screenshot=None,
        ax_tree=_ax_tree_text(d["state"], d["question"]),
        ax_tree_source="synthetic",  # a text rendering of jevbench's own fields, not a captured a11y tree
        elements=elements,
        elements_source="synthetic_spec",
        entities=[],
        options=options,
        expected=expected_map,
        split="public",
        group=d.get("group"),
        provenance={
            "source": EXTERNAL_BENCH_NAME,
            "source_url": EXTERNAL_BENCH_SOURCE_URL,
            "license": EXTERNAL_BENCH_LICENSE,
            "external_id": d["id"],
            "external_family": d.get("family"),
            "external_labels": labels,
            "external_expected": expected,
            "external_provenance": d.get("provenance"),
        },
    )


def load_external_bench_jsonl(path: str | Path) -> list[CuaTask]:
    """Reads one of jevbench's public JSONL files (e.g.
    `<clone>/datasets/public/original.jsonl`) and converts every record."""
    tasks = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line:
            tasks.append(convert_external_bench_task(json.loads(line)))
    return tasks
