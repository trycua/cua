"""Typing names that moved into `typing` in Python 3.11, with a fallback for 3.10 so
that cua-e2b needs no dependency besides `cua`. At runtime on 3.10 the fallbacks are
transparent: `NotRequired[X]` is `X` and `Unpack[X]` is `X`."""

from __future__ import annotations

import sys
from typing import Any

if sys.version_info >= (3, 11):
    from typing import NotRequired, Unpack
else:  # pragma: no cover - exercised only on Python 3.10
    try:
        from typing_extensions import NotRequired, Unpack
    except ImportError:

        class _Transparent:
            def __class_getitem__(cls, item: Any) -> Any:
                return item

        NotRequired = _Transparent  # type: ignore[assignment,misc]
        Unpack = _Transparent  # type: ignore[assignment,misc]

__all__ = ["NotRequired", "Unpack"]
