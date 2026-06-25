"""Safe coordinate parsing helpers for model-supplied GUI action strings."""

from __future__ import annotations

import ast
import math
from numbers import Real
from typing import Tuple


def parse_uitars_coordinates(raw: str) -> Tuple[float, float, float, float]:
    """Parse a UITARS point or box literal without executing code.

    Accepts only list/tuple literals containing 2 or 4 numeric values. A
    two-value point is duplicated into box form for existing call sites.
    Raises ValueError for malformed input, non-numeric values, booleans, or
    executable payloads.
    """
    try:
        value = ast.literal_eval(raw)
    except (SyntaxError, ValueError, TypeError, MemoryError) as exc:
        raise ValueError("coordinate value must be a literal list or tuple") from exc

    if not isinstance(value, (list, tuple)):
        raise ValueError("coordinate value must be a list or tuple")
    if len(value) not in (2, 4):
        raise ValueError("coordinate value must contain exactly 2 or 4 numbers")

    coords = []
    for item in value:
        if isinstance(item, bool) or not isinstance(item, Real):
            raise ValueError("coordinate values must be numeric")
        coord = float(item)
        if not math.isfinite(coord):
            raise ValueError("coordinate values must be finite")
        coords.append(coord)

    if len(coords) == 2:
        coords = [coords[0], coords[1], coords[0], coords[1]]
    return (coords[0], coords[1], coords[2], coords[3])
