"""`class_method_variant`: a method callable on an instance or on the class."""

from __future__ import annotations

import functools
from typing import Any, Generic, Optional, Type, TypeVar, cast

T = TypeVar("T")


class class_method_variant(Generic[T]):
    """A method callable on an instance or on the class: `obj.kill()`,
    `AsyncSandbox.kill(obj)` and `AsyncSandbox.kill("sandbox-id")` (the
    `class_method_name` variant, a classmethod of the same class), as e2b's
    decorator of the same name allows."""

    method: Any

    def __init__(self, class_method_name: str):
        self.class_method_name = class_method_name

    # Older cua-e2b spelling of the attribute.
    @property
    def cls_method_name(self) -> str:
        return self.class_method_name

    def __call__(self, method: T) -> T:
        self.method = method
        functools.update_wrapper(self, method)  # type: ignore[arg-type]
        return cast(T, self)

    def __get__(self, obj, objtype: Optional[Type[Any]] = None):
        @functools.wraps(self.method)
        def _wrapper(*args, **kwargs):
            if obj is not None:
                return self.method(obj, *args, **kwargs)
            if len(args) > 0 and objtype is not None and isinstance(args[0], objtype):
                return self.method(args[0], *args[1:], **kwargs)
            return getattr(objtype, self.class_method_name)(*args, **kwargs)

        return _wrapper
