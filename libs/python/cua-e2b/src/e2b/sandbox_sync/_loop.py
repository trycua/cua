"""The synchronous API runs the asynchronous implementation on one background
event-loop thread. Output callbacks, watch handlers and everything else the
async side calls back run on that thread."""

from __future__ import annotations

import asyncio
import functools
import threading
from typing import Any, Callable, Optional


class Loop:
    """One background event loop shared by every synchronous sandbox."""

    _lock = threading.Lock()
    _loop: Optional[asyncio.AbstractEventLoop] = None

    @classmethod
    def run(cls, coro):
        with cls._lock:
            if cls._loop is None or cls._loop.is_closed():
                loop = asyncio.new_event_loop()
                threading.Thread(target=loop.run_forever, name="cua-e2b-loop", daemon=True).start()
                cls._loop = loop
        return asyncio.run_coroutine_threadsafe(coro, cls._loop).result()


def proxy(inner_attr: str, async_fn: Callable) -> Callable:
    """A synchronous method that runs `async_fn` on the object held in
    `self.<inner_attr>`; `inspect.signature` shows the async signature."""

    @functools.wraps(async_fn)
    def method(self, *args: Any, **kwargs: Any):
        return Loop.run(async_fn(getattr(self, inner_attr), *args, **kwargs))

    return method
