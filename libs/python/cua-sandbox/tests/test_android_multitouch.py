"""
Android multi-touch integration tests.

Verifies the full touch action space — single-touch gestures, SDK pinch helpers,
and true two-finger MT Protocol B injection — against the TouchTest APK running
on an Android VM.

Layout
──────
  TestAndroidMultitouchLocal   bare-metal AndroidEmulatorRuntime via sandbox()
  TestAndroidMultitouchCloud   cloud Android VM via sandbox()  [no-op, TBD]

Usage
─────
  # Local only (default):
  pytest tests/test_android_multitouch.py -v

  # Cloud (once implemented):
  CUA_TEST_API_KEY=sk-... pytest tests/test_android_multitouch.py -v -k cloud

Environment variables
─────────────────────
  CUA_ANDROID_TEST_APK   path or URL to a pre-built TouchTest debug APK
                         default: latest release from
                         https://github.com/trycua/android-touch-test-app
  CUA_TEST_API_KEY       Anthropic API key (cloud tests only)
  CUA_ANDROID_API_LEVEL  Android API level to boot  (default: 34)
  CUA_ANDROID_AVD_NAME   AVD name                   (default: cua-multitouch-test)

APK source
──────────
  The TouchTest APK is built and released from:
  https://github.com/trycua/android-touch-test-app

  Latest release download:
  https://github.com/trycua/android-touch-test-app/releases/latest/download/app-debug.apk
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import urllib.request
from pathlib import Path

import pytest
import pytest_asyncio
from cua_sandbox.image import Image
from cua_sandbox.runtime.android_emulator import AndroidEmulatorRuntime
from cua_sandbox.sandbox import Sandbox, sandbox
from cua_sandbox.transport.adb import ADBTransport

# ── Config ─────────────────────────────────────────────────────────────────

_APK_RELEASE_URL = (
    "https://github.com/trycua/android-touch-test-app" "/releases/latest/download/app-debug.apk"
)
_APK_ENV = os.environ.get("CUA_ANDROID_TEST_APK", "")
# If env var is a local path use it directly; otherwise download from releases.
_APK_PATH = Path(_APK_ENV) if _APK_ENV and not _APK_ENV.startswith("http") else None
_APK_PACKAGE = "com.cuatest.touchtest"
_APK_ACTIVITY = "com.cuatest.touchtest/.MainActivity"
_LOG_TAG = "TouchTest"
_RESET_ACTION = "com.cuatest.touchtest.RESET_LOG"

_API_LEVEL = int(os.environ.get("CUA_ANDROID_API_LEVEL", "34"))
_AVD_NAME = os.environ.get("CUA_ANDROID_AVD_NAME", "cua-multitouch-test")
_API_KEY = os.environ.get("CUA_TEST_API_KEY")
_SETTLE_S = 0.6
_LAUNCH_WAIT_S = 3.0
_AXIS_MAX = 32767  # raw axis range on virtio/goldfish touch devices


# ── Helpers ────────────────────────────────────────────────────────────────


def _get_apk() -> Path:
    """Return path to the TouchTest APK, downloading from GitHub Releases if needed."""
    if _APK_PATH is not None:
        if _APK_PATH.exists():
            return _APK_PATH
        raise FileNotFoundError(f"CUA_ANDROID_TEST_APK set but file not found: {_APK_PATH}")

    # Download from latest release
    cache = Path("/tmp/cua-touch-test-app-debug.apk")
    if not cache.exists():
        url = os.environ.get("CUA_ANDROID_TEST_APK", _APK_RELEASE_URL)
        print(f"\n[setup] Downloading TouchTest APK from {url} …")
        urllib.request.urlretrieve(url, cache)
    return cache


def _px_to_raw(px: int, screen_dim: int) -> int:
    return max(0, min(_AXIS_MAX, int(px * _AXIS_MAX / screen_dim)))


def _parse_touch_events(logcat_stdout: str) -> list[dict]:
    events = []
    for line in logcat_stdout.splitlines():
        m = re.search(r"TouchTest:\s+(\{.*\})", line)
        if m:
            try:
                obj = json.loads(m.group(1))
                if "action" in obj:
                    events.append(obj)
            except json.JSONDecodeError:
                pass
    return events


def _max_pointer_count(events: list[dict]) -> int:
    return max((e.get("pointer_count", 0) for e in events), default=0)


def _has_action(events: list[dict], action: str) -> bool:
    return any(e.get("action") == action for e in events)


def _build_two_finger_script(
    dev: str,
    x1s: int,
    y1s: int,
    x2s: int,
    y2s: int,
    x1e: int,
    y1e: int,
    x2e: int,
    y2e: int,
    screen_w: int,
    screen_h: int,
    steps: int = 12,
    step_delay: float = 0.025,
) -> str:
    """
    Single chained shell command that injects a true two-finger gesture via
    Linux MT Protocol B sendevent calls.  Both finger positions share one
    SYN_REPORT frame at every step, so Android sees pointer_count == 2.
    """
    EV_SYN, EV_KEY, EV_ABS = 0, 1, 3
    SYN_REPORT = 0
    BTN_TOUCH = 330
    SLOT, TID, MTX, MTY, PRESS = 47, 57, 53, 54, 58
    TID_NONE = 4294967295

    def rx(px: int) -> int:
        return _px_to_raw(px, screen_w)

    def ry(px: int) -> int:
        return _px_to_raw(px, screen_h)

    def se(t: int, c: int, v: int) -> str:
        return f"sendevent {dev} {t} {c} {v}"

    def sync() -> str:
        return se(EV_SYN, SYN_REPORT, 0)

    cmds: list[str] = [
        se(EV_ABS, SLOT, 0),
        se(EV_ABS, TID, 0),
        se(EV_ABS, MTX, rx(x1s)),
        se(EV_ABS, MTY, ry(y1s)),
        se(EV_ABS, PRESS, 64),
        se(EV_ABS, SLOT, 1),
        se(EV_ABS, TID, 1),
        se(EV_ABS, MTX, rx(x2s)),
        se(EV_ABS, MTY, ry(y2s)),
        se(EV_ABS, PRESS, 64),
        se(EV_KEY, BTN_TOUCH, 1),
        sync(),
    ]
    for i in range(1, steps + 1):
        t = i / steps
        cmds += [
            se(EV_ABS, SLOT, 0),
            se(EV_ABS, MTX, rx(int(x1s + (x1e - x1s) * t))),
            se(EV_ABS, MTY, ry(int(y1s + (y1e - y1s) * t))),
            se(EV_ABS, SLOT, 1),
            se(EV_ABS, MTX, rx(int(x2s + (x2e - x2s) * t))),
            se(EV_ABS, MTY, ry(int(y2s + (y2e - y2s) * t))),
            sync(),
            f"sleep {step_delay:.3f}",
        ]
    cmds += [
        se(EV_ABS, SLOT, 0),
        se(EV_ABS, TID, TID_NONE),
        se(EV_ABS, SLOT, 1),
        se(EV_ABS, TID, TID_NONE),
        se(EV_KEY, BTN_TOUCH, 0),
        sync(),
    ]
    return " && ".join(cmds)


async def _read_events(sb: Sandbox) -> list[dict]:
    result = await sb.shell.run(f"logcat -d -s {_LOG_TAG}", timeout=10)
    return _parse_touch_events(result.stdout)


async def _find_touch_device(sb: Sandbox) -> str:
    result = await sb.shell.run(
        "for f in /dev/input/event*; do "
        '  getevent -p "$f" 2>/dev/null | grep -q \'0035\' && echo "$f" && break; '
        "done"
    )
    dev = result.stdout.strip()
    return dev if dev else "/dev/input/event1"


async def _reset(sb: Sandbox) -> None:
    await sb.shell.run("logcat -c")
    await sb.shell.run(f"am broadcast -a {_RESET_ACTION}")
    await asyncio.sleep(0.2)


# ── Session-scoped event loop (required for session-scoped async fixtures) ──


@pytest_asyncio.fixture(scope="session")
def event_loop():
    loop = asyncio.new_event_loop()
    yield loop
    loop.close()


# ── Session fixture: one emulator for the entire local test run ─────────────


@pytest_asyncio.fixture(scope="session", loop_scope="session")
async def local_android_sb():
    """
    Boot a single AndroidEmulatorRuntime via sandbox(), install the TouchTest
    APK, escalate to root, launch the app, and yield the ready Sandbox.
    Shared across all local tests so we only pay the boot cost once.
    """
    apk = _get_apk()

    runtime = AndroidEmulatorRuntime(
        api_level=_API_LEVEL,
        memory_mb=4096,
        headless=True,
        no_boot_anim=True,
    )

    image = Image.android(str(_API_LEVEL)).apk_install(str(apk))

    async with sandbox(runtime=runtime, image=image, name=_AVD_NAME) as sb:
        transport: ADBTransport = sb._transport

        # Escalate adbd to root so sendevent can write to /dev/input/*
        transport._adb_cmd("root", timeout=15)
        await asyncio.sleep(1.5)

        # Launch app
        await sb.shell.run(f"am start -n {_APK_ACTIVITY}")
        await asyncio.sleep(_LAUNCH_WAIT_S)

        yield sb


@pytest_asyncio.fixture(autouse=True)
async def _reset_between_tests(request, local_android_sb: Sandbox):
    """Clear logcat + app event counter before every local test."""
    if "local_android_sb" not in request.fixturenames:
        yield
        return
    await _reset(local_android_sb)
    yield


# ══════════════════════════════════════════════════════════════════════════════
# Shared test logic (mixin)
# ══════════════════════════════════════════════════════════════════════════════


class _MultitouchTests:
    """
    Full touch action suite.  Subclasses must expose a ``sb`` fixture that
    yields a ready Sandbox with the TouchTest APK installed and running.
    """

    # ── Single-touch ──────────────────────────────────────────────────────

    async def test_tap(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.tap(w // 2, h // 2)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events received"
        assert _has_action(te, "ACTION_DOWN")
        assert _has_action(te, "ACTION_UP")
        assert _max_pointer_count(te) == 1

    async def test_long_press(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.long_press(w // 2, h // 2, duration_ms=800)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN")
        assert _has_action(te, "ACTION_UP")
        assert _max_pointer_count(te) == 1

    async def test_double_tap(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.double_tap(w // 2, h // 2, delay=0.12)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        downs = [e for e in te if e.get("action") == "ACTION_DOWN"]
        ups = [e for e in te if e.get("action") == "ACTION_UP"]
        assert len(downs) >= 2, f"Expected 2 ACTION_DOWNs, got {len(downs)}"
        assert len(ups) >= 2, f"Expected 2 ACTION_UPs,   got {len(ups)}"
        assert _max_pointer_count(te) == 1

    async def test_swipe(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.swipe(w // 2, h * 3 // 4, w // 2, h // 4, duration_ms=300)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN")
        assert _has_action(te, "ACTION_MOVE"), "Swipe produced no MOVE events"
        assert _has_action(te, "ACTION_UP")
        assert _max_pointer_count(te) == 1

    async def test_fling(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.fling(w // 2, h * 3 // 4, w // 2, h // 4)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN")
        assert _has_action(te, "ACTION_UP")
        assert _max_pointer_count(te) == 1

    async def test_scroll_up(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.scroll_up(w // 2, h // 2, distance=400, duration_ms=300)
        await asyncio.sleep(_SETTLE_S)
        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN") and _has_action(te, "ACTION_UP")

    async def test_scroll_down(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.scroll_down(w // 2, h // 2, distance=400, duration_ms=300)
        await asyncio.sleep(_SETTLE_S)
        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN") and _has_action(te, "ACTION_UP")

    async def test_scroll_left(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.scroll_left(w // 2, h // 2, distance=300, duration_ms=300)
        await asyncio.sleep(_SETTLE_S)
        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN") and _has_action(te, "ACTION_UP")

    async def test_scroll_right(self, sb: Sandbox):
        w, h = await sb.get_dimensions()
        await sb.mobile.scroll_right(w // 2, h // 2, distance=300, duration_ms=300)
        await asyncio.sleep(_SETTLE_S)
        te = await _read_events(sb)
        assert _has_action(te, "ACTION_DOWN") and _has_action(te, "ACTION_UP")

    # ── SDK pinch (MT Protocol B via sendevent) ────────────────────────────

    async def test_pinch_in(self, sb: Sandbox):
        """sb.mobile.pinch_in delivers genuine pointer_count == 2 events."""
        w, h = await sb.get_dimensions()
        await sb.mobile.pinch_in(w // 2, h // 2, spread=200, duration_ms=400)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No events received"
        assert (
            _max_pointer_count(te) >= 2
        ), f"pinch_in: expected pointer_count >= 2, got {_max_pointer_count(te)}"
        assert _has_action(
            te, "ACTION_POINTER_DOWN"
        ), f"Missing ACTION_POINTER_DOWN — actions: {[e.get('action') for e in te]}"

    async def test_pinch_out(self, sb: Sandbox):
        """sb.mobile.pinch_out delivers genuine pointer_count == 2 events."""
        w, h = await sb.get_dimensions()
        await sb.mobile.pinch_out(w // 2, h // 2, spread=200, duration_ms=400)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No events received"
        assert (
            _max_pointer_count(te) >= 2
        ), f"pinch_out: expected pointer_count >= 2, got {_max_pointer_count(te)}"
        assert _has_action(te, "ACTION_POINTER_DOWN")

    # ── SDK gesture() — arbitrary multi-finger paths ──────────────────────

    async def test_gesture_pinch_out(self, sb: Sandbox):
        """sb.mobile.gesture() with two finger paths produces pointer_count == 2."""
        w, h = await sb.get_dimensions()
        cx, cy, spread = w // 2, h // 2, 200
        await sb.mobile.gesture(
            (cx - 20, cy),
            (cx - spread, cy),  # finger 0: start → end
            (cx + 20, cy),
            (cx + spread, cy),  # finger 1: start → end
        )
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No events received from gesture()"
        assert _max_pointer_count(te) >= 2
        assert _has_action(te, "ACTION_POINTER_DOWN")

    async def test_gesture_two_finger_swipe(self, sb: Sandbox):
        """Two parallel fingers moving down together."""
        w, h = await sb.get_dimensions()
        offset = 150
        await sb.mobile.gesture(
            (w // 2 - offset, h // 4),
            (w // 2 - offset, h * 3 // 4),
            (w // 2 + offset, h // 4),
            (w // 2 + offset, h * 3 // 4),
        )
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No events received"
        move_two = [
            e for e in te if e.get("action") == "ACTION_MOVE" and e.get("pointer_count", 0) >= 2
        ]
        assert move_two, "No ACTION_MOVE events with pointer_count >= 2"

    # ── True multi-touch via raw sendevent ────────────────────────────────

    async def test_true_multitouch_pinch_in(self, sb: Sandbox):
        """Raw MT Protocol B pinch-in: both fingers in one SYN_REPORT frame."""
        dev = await _find_touch_device(sb)
        w, h = await sb.get_dimensions()
        cx, cy, spread = w // 2, h // 2, 250

        script = _build_two_finger_script(
            dev,
            x1s=cx - spread,
            y1s=cy,
            x2s=cx + spread,
            y2s=cy,
            x1e=cx - 30,
            y1e=cy,
            x2e=cx + 30,
            y2e=cy,
            screen_w=w,
            screen_h=h,
        )
        await sb.shell.run(script, timeout=60)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events — check sendevent permissions"
        assert _max_pointer_count(te) >= 2
        assert _has_action(te, "ACTION_POINTER_DOWN")

    async def test_true_multitouch_pinch_out(self, sb: Sandbox):
        dev = await _find_touch_device(sb)
        w, h = await sb.get_dimensions()
        cx, cy, spread = w // 2, h // 2, 250

        script = _build_two_finger_script(
            dev,
            x1s=cx - 30,
            y1s=cy,
            x2s=cx + 30,
            y2s=cy,
            x1e=cx - spread,
            y1e=cy,
            x2e=cx + spread,
            y2e=cy,
            screen_w=w,
            screen_h=h,
        )
        await sb.shell.run(script, timeout=60)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events"
        assert _max_pointer_count(te) >= 2

    async def test_true_multitouch_two_finger_swipe(self, sb: Sandbox):
        """Two parallel fingers moving in the same direction."""
        dev = await _find_touch_device(sb)
        w, h = await sb.get_dimensions()
        offset = 150

        script = _build_two_finger_script(
            dev,
            x1s=w // 2 - offset,
            y1s=h * 3 // 4,
            x2s=w // 2 + offset,
            y2s=h * 3 // 4,
            x1e=w // 2 - offset,
            y1e=h // 4,
            x2e=w // 2 + offset,
            y2e=h // 4,
            screen_w=w,
            screen_h=h,
            steps=15,
            step_delay=0.02,
        )
        await sb.shell.run(script, timeout=60)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events"
        move_with_two = [
            e for e in te if e.get("action") == "ACTION_MOVE" and e.get("pointer_count", 0) >= 2
        ]
        assert move_with_two, "No ACTION_MOVE events with pointer_count >= 2"

    async def test_pointer_ids_are_distinct(self, sb: Sandbox):
        """Each finger in a two-finger gesture must carry a unique pointer id."""
        dev = await _find_touch_device(sb)
        w, h = await sb.get_dimensions()
        cx, cy = w // 2, h // 2

        script = _build_two_finger_script(
            dev,
            x1s=cx - 200,
            y1s=cy,
            x2s=cx + 200,
            y2s=cy,
            x1e=cx - 50,
            y1e=cy,
            x2e=cx + 50,
            y2e=cy,
            screen_w=w,
            screen_h=h,
            steps=8,
        )
        await sb.shell.run(script, timeout=60)
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        two_ptr = [e for e in te if e.get("pointer_count", 0) >= 2]
        assert two_ptr, "No two-pointer events found"
        for ev in two_ptr:
            ids = {p["id"] for p in ev.get("pointers", [])}
            assert len(ids) >= 2, f"Duplicate pointer ids in event: {ev}"


# ══════════════════════════════════════════════════════════════════════════════
# Local tests
# ══════════════════════════════════════════════════════════════════════════════


@pytest.mark.asyncio
class TestAndroidMultitouchLocal(_MultitouchTests):
    """Full touch action suite against a bare-metal AndroidEmulatorRuntime."""

    @pytest_asyncio.fixture
    async def sb(self, local_android_sb: Sandbox):
        await _reset(local_android_sb)
        return local_android_sb


# ══════════════════════════════════════════════════════════════════════════════
# Cloud tests
# ══════════════════════════════════════════════════════════════════════════════

skip_no_api_key = pytest.mark.skipif(not _API_KEY, reason="CUA_TEST_API_KEY not set")


@pytest_asyncio.fixture(scope="session", loop_scope="session")
async def cloud_android_sb():
    """
    Spin up an ephemeral cloud Android VM, install the TouchTest APK, escalate
    to root, launch the app, and yield the ready Sandbox.
    Shared across all cloud tests so we only pay the boot cost once.
    """
    if not _API_KEY:
        pytest.skip("CUA_TEST_API_KEY not set — cloud tests skipped")

    async with sandbox(image=Image.android("14"), api_key=_API_KEY) as sb:
        # Download and install the APK directly on the device
        await sb.shell.run(
            f"curl -fsSL -o /data/local/tmp/touchtest.apk '{_APK_RELEASE_URL}'",
            timeout=120,
        )
        result = await sb.shell.run("pm install -r /data/local/tmp/touchtest.apk", timeout=60)
        assert "Success" in result.stdout, f"APK install failed: {result.stdout}"

        # Escalate to root so sendevent can write to /dev/input/*
        await sb.shell.run("su root id", timeout=15)
        await asyncio.sleep(1.5)

        # Launch app
        await sb.shell.run(f"am start -n {_APK_ACTIVITY}")
        await asyncio.sleep(_LAUNCH_WAIT_S)

        yield sb


@pytest.mark.asyncio
@pytest.mark.skipif(not _API_KEY, reason="CUA_TEST_API_KEY not set")
class TestAndroidMultitouchCloud(_MultitouchTests):
    """Full touch action suite against a cloud-hosted Android VM."""

    @pytest_asyncio.fixture
    async def sb(self, cloud_android_sb: Sandbox):
        await _reset(cloud_android_sb)
        return cloud_android_sb
