"""Wait for a sandbox's desktop to be drawn and settled before task setup.

A sandbox is ready when its daemon answers, which is seconds before the
desktop session (window manager, panels, autostarted apps) has drawn
anything. A task that opens its app window in that gap loses it: the window
maps before the window manager manages it, or an autostarted app maps on
top of it, and the oracle's or agent's clicks land elsewhere. So before
setup, poll screenshots until the screen is not blank and stays the same,
apart from a small region such as a clock or a caret, for ``SETTLED_SHOTS``
screenshots in a row (the desktop is idle). The wait is bounded by
``CUA_BENCH_DESKTOP_READY_S`` (default 60 s; ``0`` turns it off). Sessions
without a screen are skipped.
"""

from __future__ import annotations

import asyncio
import os
import time
from typing import Any, Awaitable, Callable, Optional

DEFAULT_TIMEOUT_S = 60.0
POLL_INTERVAL_S = 1.0
SHOT_TIMEOUT_S = 10.0


def _timeout_s(environ: Optional[dict] = None) -> float:
    raw = (environ if environ is not None else os.environ).get("CUA_BENCH_DESKTOP_READY_S", "")
    try:
        return max(0.0, float(raw)) if str(raw).strip() else DEFAULT_TIMEOUT_S
    except ValueError:
        return DEFAULT_TIMEOUT_S


#: Two screenshots whose difference fits in this fraction of the screen
#: (a ticking clock, a blinking caret) count as the same desktop.
SETTLED_FRACTION = 0.01
#: Consecutive non-blank screenshots that must match (about 2 s at 1 s polls).
SETTLED_SHOTS = 3


def _gray(png: bytes):
    import io

    from PIL import Image

    with Image.open(io.BytesIO(png)) as image:
        return image.convert("L")


def is_blank(png: bytes) -> bool:
    """True for a screenshot with a single colour (nothing drawn yet)."""
    try:
        low, high = _gray(png).getextrema()
    except Exception:  # noqa: BLE001 - undecodable: do not block on it
        return False
    return low == high


def is_settled(before: bytes, after: bytes) -> bool:
    """True when two screenshots differ in at most a small region."""
    if before == after:
        return True
    try:
        from PIL import ImageChops

        a, b = _gray(before), _gray(after)
        if a.size != b.size:
            return False
        box = ImageChops.difference(a, b).getbbox()
    except Exception:  # noqa: BLE001
        return False
    if box is None:
        return True
    area = (box[2] - box[0]) * (box[3] - box[1])
    return area <= SETTLED_FRACTION * a.size[0] * a.size[1]


async def wait_for_desktop(
    session: Any,
    *,
    timeout_s: Optional[float] = None,
    interval_s: float = POLL_INTERVAL_S,
    clock: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], Awaitable[Any]] = asyncio.sleep,
) -> str:
    """Poll ``session.screenshot()`` until the desktop is drawn and idle.

    Returns ``"ready"``, ``"timeout"`` (the setup proceeds anyway),
    ``"no-screen"`` or ``"off"``. Never raises.
    """
    limit = _timeout_s() if timeout_s is None else timeout_s
    if limit <= 0:
        return "off"
    if getattr(session, "_cb_no_screen", False) or not hasattr(session, "screenshot"):
        return "no-screen"
    deadline = clock() + limit
    previous: Optional[bytes] = None
    same = 0
    polls = 0
    max_polls = int(limit / max(interval_s, 0.01)) + 2  # hard bound, whatever the clock does
    while polls < max_polls:
        polls += 1
        try:
            shot = await asyncio.wait_for(session.screenshot(), SHOT_TIMEOUT_S)
        except Exception:  # noqa: BLE001 - no screen API: nothing to wait for
            return "no-screen"
        if not shot:
            return "no-screen"
        if is_blank(shot):
            previous, same = None, 0
        else:
            same = same + 1 if previous is not None and is_settled(previous, shot) else 1
            if same >= SETTLED_SHOTS:
                return "ready"
            previous = shot
        if clock() >= deadline:
            break
        await sleep(interval_s)
    print(f"Warning: the desktop did not settle within {limit:.0f}s; running setup anyway")
    return "timeout"


__all__ = ["is_blank", "is_settled", "wait_for_desktop"]
