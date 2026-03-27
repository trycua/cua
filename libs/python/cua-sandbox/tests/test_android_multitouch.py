"""
Android multi-touch integration tests.

Verifies the full touch action space — single-touch gestures, SDK pinch helpers,
and true two-finger MT Protocol B injection — against the TouchTest APK running
on an Android VM.

Layout
──────
  TestAndroidMultitouchLocal   bare-metal AndroidEmulatorRuntime via Sandbox.ephemeral()
  TestAndroidMultitouchCloud   cloud Android VM via Sandbox.ephemeral()

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
from cua_sandbox.runtime.compat import skip_if_unsupported
from cua_sandbox.sandbox import Sandbox

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


async def _read_events(sb: Sandbox) -> list[dict]:
    result = await sb.shell.run(f"logcat -d -s {_LOG_TAG}", timeout=10)
    return _parse_touch_events(result.stdout)


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
    Boot a single AndroidEmulatorRuntime via Sandbox.ephemeral(), install the TouchTest
    APK, escalate to root, launch the app, and yield the ready Sandbox.
    Shared across all local tests so we only pay the boot cost once.
    """
    skip_if_unsupported(Image.android(str(_API_LEVEL)))

    apk = _get_apk()

    runtime = AndroidEmulatorRuntime(
        api_level=_API_LEVEL,
        memory_mb=4096,
        headless=True,
        no_boot_anim=True,
    )

    image = Image.android(str(_API_LEVEL)).apk_install(str(apk))

    async with Sandbox.ephemeral(image, runtime=runtime, name=_AVD_NAME) as sb:
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

    # ── True multi-touch via MT Protocol B (server-side injection) ───────────

    async def test_true_multitouch_pinch_in(self, sb: Sandbox):
        """MT Protocol B pinch-in via the multitouch_gesture server action."""
        w, h = await sb.get_dimensions()
        cx, cy, spread = w // 2, h // 2, 250
        await sb.mobile.gesture(
            (cx - spread, cy),
            (cx - 30, cy),
            (cx + spread, cy),
            (cx + 30, cy),
        )
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events"
        assert _max_pointer_count(te) >= 2
        assert _has_action(te, "ACTION_POINTER_DOWN")

    async def test_true_multitouch_pinch_out(self, sb: Sandbox):
        """MT Protocol B pinch-out via the multitouch_gesture server action."""
        w, h = await sb.get_dimensions()
        cx, cy, spread = w // 2, h // 2, 250
        await sb.mobile.gesture(
            (cx - 30, cy),
            (cx - spread, cy),
            (cx + 30, cy),
            (cx + spread, cy),
        )
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events"
        assert _max_pointer_count(te) >= 2

    async def test_true_multitouch_two_finger_swipe(self, sb: Sandbox):
        """Two parallel fingers moving in the same direction."""
        w, h = await sb.get_dimensions()
        offset = 150
        await sb.mobile.gesture(
            (w // 2 - offset, h * 3 // 4),
            (w // 2 - offset, h // 4),
            (w // 2 + offset, h * 3 // 4),
            (w // 2 + offset, h // 4),
            duration_ms=300,
            steps=15,
        )
        await asyncio.sleep(_SETTLE_S)

        te = await _read_events(sb)
        assert te, "No touch events"
        move_with_two = [
            e for e in te if e.get("action") == "ACTION_MOVE" and e.get("pointer_count", 0) >= 2
        ]
        assert move_with_two, "No ACTION_MOVE events with pointer_count >= 2"

    async def test_pointer_ids_are_distinct(self, sb: Sandbox):
        """Each finger in a two-finger gesture must carry a unique pointer id."""
        w, h = await sb.get_dimensions()
        cx, cy = w // 2, h // 2
        await sb.mobile.gesture(
            (cx - 200, cy),
            (cx - 50, cy),
            (cx + 200, cy),
            (cx + 50, cy),
            steps=8,
        )
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

    @pytest_asyncio.fixture(loop_scope="session")
    async def sb(self, local_android_sb: Sandbox):
        await _reset(local_android_sb)
        return local_android_sb


# ══════════════════════════════════════════════════════════════════════════════
# Cloud tests
# ══════════════════════════════════════════════════════════════════════════════

skip_no_api_key = pytest.mark.skipif(not _API_KEY, reason="CUA_TEST_API_KEY not set")
# Note: cloud Android tests are gated by API key only — hardware compat is the cloud's concern.


@pytest_asyncio.fixture(scope="session", loop_scope="session")
async def cloud_android_sb():
    """
    Spin up an ephemeral cloud Android VM, install the TouchTest APK, escalate
    to root, launch the app, and yield the ready Sandbox.
    Shared across all cloud tests so we only pay the boot cost once.
    """
    if not _API_KEY:
        pytest.skip("CUA_TEST_API_KEY not set — cloud tests skipped")

    image = Image.android("14").apk_install(_APK_RELEASE_URL)
    async with Sandbox.ephemeral(image, api_key=_API_KEY) as sb:
        # Launch app
        await sb.shell.run(f"am start -n {_APK_ACTIVITY}")
        await asyncio.sleep(_LAUNCH_WAIT_S)

        yield sb


@pytest.mark.asyncio(loop_scope="session")
@pytest.mark.skipif(not _API_KEY, reason="CUA_TEST_API_KEY not set")
class TestAndroidMultitouchCloud(_MultitouchTests):
    """Full touch action suite against a cloud-hosted Android VM."""

    @pytest_asyncio.fixture(loop_scope="session")
    async def sb(self, cloud_android_sb: Sandbox):
        await _reset(cloud_android_sb)
        return cloud_android_sb
