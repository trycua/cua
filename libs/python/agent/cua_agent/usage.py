"""Shared cost handling: None means pricing is unknown, not free."""

import math
from typing import Any, Optional


def normalize_response_cost(value: Any) -> Optional[float]:
    """Return a finite nonnegative cost, preserving missing pricing as None."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        cost = float(value)
    except OverflowError:
        return None
    return cost if math.isfinite(cost) and cost >= 0 else None


def add_response_cost(total: Optional[float], value: Any) -> Optional[float]:
    """A total stays unknown once any response has unknown pricing."""
    cost = normalize_response_cost(value)
    return None if total is None or cost is None else total + cost
