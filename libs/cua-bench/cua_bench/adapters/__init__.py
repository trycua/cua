"""Benchmark adapters: upstream benchmarks as cua-bench tasks.

See :class:`BenchAdapter`. Adapters live next to their tasks (for example
``tasks/winarena_adapter``); this package holds the shared base.
"""

from .base import BenchAdapter, Endpoints, ServerSpec, unmet_requirements

__all__ = ["BenchAdapter", "Endpoints", "ServerSpec", "unmet_requirements"]
