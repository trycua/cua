"""UX invariant monitor — polls at 5 ms via a background thread.

Signals checked on every tick:
  - Real cursor position: NSEvent.mouseLocation() — violation if moved > 4 px
  - Frontmost app PID: NSWorkspace.sharedWorkspace().frontmostApplication() — violation if != sentinel_pid
  - Agent overlay z-order: CGWindowListCopyWindowInfo — violation if a cua-driver
    overlay window layer >= frontmost non-agent window layer

Usage::

    mon = UXMonitor(sentinel_pid=focus_monitor_pid)
    mon.start()
    # ... run test ...
    mon.stop()
    mon.assert_clean()   # raises AssertionError if any violation was recorded
"""

from __future__ import annotations

import threading
import time
from dataclasses import dataclass, field
from typing import Optional


# Try to import PyObjC; fall back to no-op monitor on systems without it.
try:
    from AppKit import NSWorkspace
    from AppKit import NSEvent
    from Quartz import (
        CGWindowListCopyWindowInfo,
        kCGWindowListOptionOnScreenOnly,
        kCGNullWindowID,
        kCGWindowLayer,
        kCGWindowOwnerPID,
        kCGWindowName,
    )
    _PYOBJC_AVAILABLE = True
except ImportError:
    _PYOBJC_AVAILABLE = False


# Cursor movement tolerance in logical screen pixels.
_CURSOR_TOLERANCE_PX = 4.0

# Polling interval in seconds.
_POLL_INTERVAL_S = 0.005  # 5 ms


@dataclass
class Violation:
    kind: str   # "cursor" | "frontmost" | "overlay_z"
    detail: str
    timestamp: float = field(default_factory=time.monotonic)

    def __str__(self) -> str:
        return f"[{self.kind}] {self.detail} (t={self.timestamp:.3f})"


class UXMonitor:
    """5 ms background poller that records UX invariant violations."""

    def __init__(
        self,
        sentinel_pid: Optional[int] = None,
        cursor_tolerance_px: float = _CURSOR_TOLERANCE_PX,
        check_cursor: bool = True,
        check_frontmost: bool = True,
        check_overlay_z: bool = True,
    ) -> None:
        self.sentinel_pid = sentinel_pid
        self.cursor_tolerance_px = cursor_tolerance_px
        self.check_cursor = check_cursor and _PYOBJC_AVAILABLE
        self.check_frontmost = check_frontmost and _PYOBJC_AVAILABLE
        self.check_overlay_z = check_overlay_z and _PYOBJC_AVAILABLE

        self._violations: list[Violation] = []
        self._lock = threading.Lock()
        self._stop_event = threading.Event()
        self._thread: Optional[threading.Thread] = None

        # Baseline cursor position captured at start().
        self._baseline_x: float = 0.0
        self._baseline_y: float = 0.0

        # Grace period: only record frontmost violations after the sentinel has
        # been observed as frontmost at least once.  This prevents false positives
        # when the monitor starts before the activation fixture runs.
        self._sentinel_seen: bool = False

    # ── public API ───────────────────────────────────────────────────────────

    def start(self) -> "UXMonitor":
        if not _PYOBJC_AVAILABLE:
            return self
        self._baseline_x, self._baseline_y = self._cursor_pos()
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._run, daemon=True, name="ux-monitor")
        self._thread.start()
        return self

    def stop(self) -> None:
        if self._thread is None:
            return
        self._stop_event.set()
        self._thread.join(timeout=1.0)
        self._thread = None

    def violations(self) -> list[Violation]:
        with self._lock:
            return list(self._violations)

    def assert_clean(self) -> None:
        """Raise AssertionError if any violations were recorded."""
        with self._lock:
            vs = list(self._violations)
        if vs:
            lines = "\n".join(f"  {v}" for v in vs)
            raise AssertionError(f"UX violations detected ({len(vs)}):\n{lines}")

    def reset_baseline(self) -> None:
        """Reset cursor baseline to current position (e.g. after intentional move)."""
        if _PYOBJC_AVAILABLE:
            self._baseline_x, self._baseline_y = self._cursor_pos()

    # ── internals ────────────────────────────────────────────────────────────

    def _run(self) -> None:
        while not self._stop_event.wait(timeout=_POLL_INTERVAL_S):
            self._tick()

    def _tick(self) -> None:
        try:
            if self.check_cursor:
                self._check_cursor()
            if self.check_frontmost and self.sentinel_pid is not None:
                self._check_frontmost()
            if self.check_overlay_z:
                self._check_overlay_z()
        except Exception:
            pass  # never crash the monitor thread

    def _record(self, kind: str, detail: str) -> None:
        with self._lock:
            self._violations.append(Violation(kind=kind, detail=detail))

    # ── signal checkers ──────────────────────────────────────────────────────

    @staticmethod
    def _cursor_pos() -> tuple[float, float]:
        loc = NSEvent.mouseLocation()
        return float(loc.x), float(loc.y)

    def _check_cursor(self) -> None:
        x, y = self._cursor_pos()
        dx = x - self._baseline_x
        dy = y - self._baseline_y
        dist = (dx * dx + dy * dy) ** 0.5
        if dist > self.cursor_tolerance_px:
            self._record(
                "cursor",
                f"moved {dist:.1f}px from baseline "
                f"({self._baseline_x:.0f},{self._baseline_y:.0f}) "
                f"→ ({x:.0f},{y:.0f})",
            )
            # Update baseline so we don't spam the same violation.
            self._baseline_x, self._baseline_y = x, y

    def _check_frontmost(self) -> None:
        app = NSWorkspace.sharedWorkspace().frontmostApplication()
        if app is None:
            return
        pid = int(app.processIdentifier())
        if pid == self.sentinel_pid:
            # Sentinel is frontmost — record this so we exit the grace period.
            self._sentinel_seen = True
            return
        if not self._sentinel_seen:
            # Grace period: don't flag until sentinel has been frontmost at least once.
            return
        name = str(app.localizedName() or "?")
        self._record(
            "frontmost",
            f"frontmost changed to pid={pid} ({name}), "
            f"expected sentinel pid={self.sentinel_pid}",
        )
        # Don't spam — pause checking until next start
        self.check_frontmost = False

    def _check_overlay_z(self) -> None:
        """Check that no cua-driver overlay window is above foreground windows."""
        wins = CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly, kCGNullWindowID
        )
        if not wins:
            return

        # Separate overlay windows from non-overlay windows
        overlay_layers: list[int] = []
        non_overlay_min_layer: int = 10000

        for w in wins:
            pid = int(w.get(kCGWindowOwnerPID, 0))
            name = str(w.get(kCGWindowName, "") or "")
            layer = int(w.get(kCGWindowLayer, 0))

            is_overlay = name in ("cua-agent-cursor", "cua-overlay") or \
                         "cua-driver" in str(name).lower()

            if is_overlay:
                overlay_layers.append(layer)
            elif layer >= 0:  # normal on-screen window
                non_overlay_min_layer = min(non_overlay_min_layer, layer)

        if not overlay_layers:
            return

        max_overlay = max(overlay_layers)
        if max_overlay >= non_overlay_min_layer and non_overlay_min_layer < 10000:
            self._record(
                "overlay_z",
                f"cua-driver overlay layer={max_overlay} >= "
                f"foreground window layer={non_overlay_min_layer}",
            )
