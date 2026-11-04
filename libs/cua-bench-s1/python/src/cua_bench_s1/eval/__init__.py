"""Generic, model-agnostic scoring/evaluation harness for cua-bench-s1.

Any architecture implementing `ModelAdapter.predict(task, modality) ->
{option_key: probability}` can be scored by this package without further
integration work.
"""
from __future__ import annotations
