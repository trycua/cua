from typing import Any, Dict, List

from .base import AsyncCallbackHandler


class BudgetExceededError(Exception):
    """Exception raised when budget is exceeded."""

    pass


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

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Reset budget if configured to do so."""
        if self.reset_after_each_run:
            self.total_cost = 0.0

    async def on_usage(self, usage: Dict[str, Any]) -> None:
        """Track usage costs."""
        if "response_cost" in usage:
            self.total_cost += usage["response_cost"]

    async def on_run_continue(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> bool:
        """Check if budget allows continuation."""
        if self.total_cost >= self.max_budget:
            if self.raise_error:
                raise BudgetExceededError(
                    f"Budget exceeded: ${self.total_cost} >= ${self.max_budget}"
                )
            else:
                print(f"Budget exceeded: ${self.total_cost} >= ${self.max_budget}")
            return False
        return True
