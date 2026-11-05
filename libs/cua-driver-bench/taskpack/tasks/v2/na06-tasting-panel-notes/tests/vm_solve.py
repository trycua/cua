"""Scripted solution for NA-06, used only by `_kit/devtools/vm_validate.py` in a macOS VM.

It changes the app's data or files directly (not through the GUI), to prove the evaluator passes a solved state.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_kit"))

from v2kit import mac  # noqa: E402

SCRIPT = r"""
const app = Application("Notes");
const f = app.defaultAccount().folders.byName("Tasting panel");
const r3 = f.notes().find(n => n.name() === "Round 3 scores");
r3.body = "<div><b>Round 3 scores</b></div><div>Apple &amp; ginger: 7</div><div>Pear &amp; cardamom: 8</div><div>Plum &amp; star anise: 9</div><div>Quince &amp; bay: 8</div>";
f.notes.push(app.Note({body: "<div>Round 4 shortlist</div><div>Pear &amp; cardamom</div><div>Plum &amp; star anise</div>"}));
JSON.stringify(true);
"""


def solve(workspace: Path) -> None:
    mac.jxa(SCRIPT)
