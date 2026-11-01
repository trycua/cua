from typing import Awaitable, Callable, TypeVar, Union

T = TypeVar("T")
OutputHandler = Union[Callable[[T], None], Callable[[T], Awaitable[None]]]
