"""
Factory for creating scheduling strategies.
Eliminates if-else statements in strategy creation.
"""

from typing import Dict, Type
from .base import SchedulingStrategy
from .manual import ManualSchedulingStrategy
from .every_action import EveryActionSchedulingStrategy
from .run_boundaries import (
    RunStartSchedulingStrategy,
    RunEndSchedulingStrategy,
    RunBoundariesSchedulingStrategy
)


class SchedulingStrategyFactory:
    """
    Factory for creating scheduling strategies without if-else statements.
    Uses polymorphism and registry pattern.
    """

    _STRATEGIES: Dict[str, Type[SchedulingStrategy]] = {
        "manual": ManualSchedulingStrategy,
        "every_action": EveryActionSchedulingStrategy,
        "run_start": RunStartSchedulingStrategy,
        "run_end": RunEndSchedulingStrategy,
        "run_boundaries": RunBoundariesSchedulingStrategy
    }

    @classmethod
    def create(cls, strategy_name: str) -> SchedulingStrategy:
        """
        Create a scheduling strategy by name.
        No if-else statements - uses polymorphic dispatch.

        Args:
            strategy_name: Name of the strategy to create

        Returns:
            Concrete strategy instance

        Raises:
            ValueError: If strategy name is not recognized
        """
        strategy_class = cls._STRATEGIES.get(strategy_name)

        if not strategy_class:
            available = list(cls._STRATEGIES.keys())
            raise ValueError(f"Unknown strategy '{strategy_name}'. Available: {available}")

        return strategy_class()

    @classmethod
    def get_available_strategies(cls) -> list[str]:
        """Get list of available strategy names."""
        return list(cls._STRATEGIES.keys())

    @classmethod
    def register_strategy(cls, name: str, strategy_class: Type[SchedulingStrategy]) -> None:
        """Register a new strategy type."""
        cls._STRATEGIES[name] = strategy_class