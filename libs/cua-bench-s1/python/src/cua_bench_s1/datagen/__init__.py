"""Task generators and external-dataset converters for cua-bench-s1.

`generator.py` is the synthetic GUI-task generator (form-filling, consent
checkboxes, login, multi-step forms, pagination, search/filter). The other
modules convert real external datasets and environments (AndroidControl,
GUI-360, chess via python-chess/Stockfish, ViZDoom, and an external
typed-bounded-decision benchmark) into the same `CuaTask` schema.
"""
from __future__ import annotations
