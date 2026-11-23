from typing import Any, Dict, List

from ..usage import normalize_response_cost
from .base import AsyncCallbackHandler


class BudgetExceededError(Exception):
    """Exception raised when a spending limit is exceeded or cannot be enforced."""

    pass


UNKNOWN_COST_MESSAGE = (
    "Cannot enforce budget: response cost is unknown. "
    "Configure model pricing before continuing a budgeted run."
)


class BudgetManagerCallback(AsyncCallbackHandler):
    """Budget manager callback that tracks usage costs and can stop execution when budget is exceeded."""

    def __init__(
        self, max_budget: float, reset_after_each_run: bool = True, raise_error: bool = False
    ):
        """
        Initialize BudgetManagerCallback.

        Args:
            max_budget: Maximum budget allowed
            reset_after_each_run: Whether to reset budget after each run
            raise_error: Whether to raise an error when budget is exceeded
        """
        self.max_budget = max_budget
        self.reset_after_each_run = reset_after_each_run
        self.raise_error = raise_error
        self.total_cost = 0.0
        self._cost_unknown = False

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Reset budget if configured to do so."""
        if self.reset_after_each_run:
            self.total_cost = 0.0
            self._cost_unknown = False

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Track usage costs."""
        cost = normalize_response_cost(usage.get("response_cost"))
        if cost is None:
            first_unknown = not self._cost_unknown
            self._cost_unknown = True
            if self.raise_error:
                raise BudgetExceededError(UNKNOWN_COST_MESSAGE)
            if first_unknown:
                print(UNKNOWN_COST_MESSAGE)
        else:
            self.total_cost += cost

    async def on_run_continue(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> bool:
        """Check if budget allows continuation."""
        if self._cost_unknown:
            if self.raise_error:
                raise BudgetExceededError(UNKNOWN_COST_MESSAGE)
            return False
        if self.total_cost >= self.max_budget:
            if self.raise_error:
                raise BudgetExceededError(
                    f"Budget exceeded: ${self.total_cost} >= ${self.max_budget}"
                )
            else:
                print(f"Budget exceeded: ${self.total_cost} >= ${self.max_budget}")
            return False
        return True
