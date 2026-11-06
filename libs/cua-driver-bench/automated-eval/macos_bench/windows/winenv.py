# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Win32 reads shared by the Windows sentinel, the GUI-only guard and the session launcher (ctypes only).

Window identity is the executable's stem (`BenchLab`, `msedge`, `WindowsTerminal`), the Windows counterpart
of the bundle id (macOS) and WM_CLASS (Linux). Python processes are named after their window title when it
starts with BenchLab, so a BenchLab started from python.exe still reads as BenchLab.
"""

from __future__ import annotations

import ctypes
import math
import os
import time
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
dwmapi = ctypes.WinDLL("dwmapi")
ole32 = ctypes.WinDLL("ole32")

HWND = wintypes.HWND
user32.GetForegroundWindow.restype = HWND
user32.GetWindow.restype = HWND
user32.GetWindow.argtypes = [HWND, wintypes.UINT]
user32.GetWindowThreadProcessId.argtypes = [HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.IsWindowVisible.argtypes = [HWND]
user32.IsIconic.argtypes = [HWND]
user32.GetWindowRect.argtypes = [HWND, ctypes.POINTER(wintypes.RECT)]
user32.GetWindowTextW.argtypes = [HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetClassNameW.argtypes = [HWND, wintypes.LPWSTR, ctypes.c_int]
user32.SetForegroundWindow.argtypes = [HWND]
user32.BringWindowToTop.argtypes = [HWND]
user32.ShowWindow.argtypes = [HWND, ctypes.c_int]
user32.AttachThreadInput.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.BOOL]
user32.GetClipboardSequenceNumber.restype = wintypes.DWORD
user32.SetWindowPos.argtypes = [
    HWND, HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT
]  # fmt: skip
WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, HWND, wintypes.LPARAM)
user32.EnumWindows.argtypes = [WNDENUMPROC, wintypes.LPARAM]
kernel32.OpenProcess.restype = wintypes.HANDLE
kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
kernel32.QueryFullProcessImageNameW.argtypes = [
    wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)
]  # fmt: skip
kernel32.CloseHandle.argtypes = [wintypes.HANDLE]

GW_OWNER, GW_HWNDPREV = 4, 3
DWMWA_CLOAKED = 14
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
SW_RESTORE = 9
SWP_NOSIZE, SWP_NOMOVE, SWP_NOACTIVATE, SWP_NOZORDER = 0x1, 0x2, 0x10, 0x4


def text(hwnd: int) -> str:
    buf = ctypes.create_unicode_buffer(512)
    user32.GetWindowTextW(hwnd, buf, 512)
    return buf.value


def class_name(hwnd: int) -> str:
    buf = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, buf, 256)
    return buf.value


def window_pid(hwnd: int) -> int:
    pid = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    return pid.value


def window_thread(hwnd: int) -> int:
    return user32.GetWindowThreadProcessId(hwnd, None)


def process_path(pid: int) -> str:
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return ""
    try:
        size = wintypes.DWORD(1024)
        buf = ctypes.create_unicode_buffer(1024)
        if kernel32.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size)):
            return buf.value
        return ""
    finally:
        kernel32.CloseHandle(handle)


def stem(path: str) -> str:
    return os.path.splitext(os.path.basename(path))[0]


def identity(hwnd: int, own: int = 0, own_bid: str = "ai.cua.benchsentinel") -> dict:
    if not hwnd:
        return {"bid": "win32:0", "pid": -1}
    if own and hwnd == own:
        return {"bid": own_bid, "pid": os.getpid()}
    pid = window_pid(hwnd)
    name = stem(process_path(pid)) if pid else ""
    if name.lower() in ("python", "pythonw") and text(hwnd).startswith("BenchLab"):
        name = "BenchLab"
    return {"bid": name or f"win32:{hwnd}", "pid": pid or -1}


def rect(hwnd: int) -> tuple[int, int, int, int]:
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return r.left, r.top, r.right, r.bottom


def intersects(a: tuple[int, int, int, int], b: tuple[int, int, int, int]) -> bool:
    return a[0] < b[2] and b[0] < a[2] and a[1] < b[3] and b[1] < a[3]


def cloaked(hwnd: int) -> bool:
    value = wintypes.DWORD()
    dwmapi.DwmGetWindowAttribute(
        HWND(hwnd), wintypes.DWORD(DWMWA_CLOAKED), ctypes.byref(value), ctypes.sizeof(value)
    )
    return bool(value.value)


def shown(hwnd: int) -> bool:
    if not user32.IsWindowVisible(hwnd) or user32.IsIconic(hwnd) or cloaked(hwnd):
        return False
    left, top, right, bottom = rect(hwnd)
    return right > left and bottom > top


def windows_above(own: int) -> list[int]:
    """Shown top-level windows above `own` in the z-order that overlap it (a raise over the user window)."""
    out = []
    mine = rect(own)
    hwnd = user32.GetWindow(own, GW_HWNDPREV)
    seen = 0
    while hwnd and seen < 2000:
        seen += 1
        if shown(hwnd) and intersects(rect(hwnd), mine):
            out.append(hwnd)
        hwnd = user32.GetWindow(hwnd, GW_HWNDPREV)
    return out


def owned_dialog_shown(own: int) -> bool:
    """A shown top-level window owned by `own`: a dialog on the user's app."""
    found = []

    def each(hwnd, _):
        if hwnd != own and user32.GetWindow(hwnd, GW_OWNER) == own and shown(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(WNDENUMPROC(each), 0)
    return bool(found)


def top_level_windows() -> list[int]:
    found: list[int] = []

    def each(hwnd, _):
        found.append(hwnd)
        return True

    user32.EnumWindows(WNDENUMPROC(each), 0)
    return found


def cursor() -> list[int]:
    p = wintypes.POINT()
    user32.GetCursorPos(ctypes.byref(p))
    return [p.x, p.y]


def clipboard_seq() -> int:
    return int(user32.GetClipboardSequenceNumber())


class _GUID(ctypes.Structure):
    _fields_ = [
        ("d1", wintypes.DWORD), ("d2", wintypes.WORD), ("d3", wintypes.WORD), ("d4", ctypes.c_ubyte * 8)
    ]  # fmt: skip

    @classmethod
    def parse(cls, s: str) -> _GUID:
        g = cls()
        ole32.CLSIDFromString(ctypes.c_wchar_p(s), ctypes.byref(g))
        return g


class VirtualDesktops:
    """IVirtualDesktopManager (documented, Windows 10 and later) through a raw COM vtable."""

    CLSID = "{aa509086-5ca9-4c25-8f95-589d3c07b48a}"
    IID = "{a5cd92ff-29be-454c-8d04-d82879fb3f1b}"

    def __init__(self) -> None:
        ole32.CoInitializeEx(None, 2)  # apartment threaded, like the GUI thread that calls it
        self.ptr = ctypes.c_void_p()
        hr = ole32.CoCreateInstance(
            ctypes.byref(_GUID.parse(self.CLSID)), None, 1 | 4,
            ctypes.byref(_GUID.parse(self.IID)), ctypes.byref(self.ptr),
        )  # fmt: skip
        if hr != 0 or not self.ptr.value:
            self.ptr = None
            return
        vtable = ctypes.cast(self.ptr, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))).contents
        proto = ctypes.WINFUNCTYPE(
            ctypes.c_long, ctypes.c_void_p, HWND, ctypes.POINTER(wintypes.BOOL)
        )
        self._on_current = proto(vtable[3])

    def on_current(self, hwnd: int) -> bool | None:
        if not self.ptr:
            return None
        out = wintypes.BOOL()
        if self._on_current(self.ptr, hwnd, ctypes.byref(out)) != 0:
            return None
        return bool(out.value)


def force_foreground(hwnd: int) -> bool:
    """Bring `hwnd` to the front from a process that may not own the foreground (setup only, never armed)."""
    if user32.IsIconic(hwnd):
        user32.ShowWindow(hwnd, SW_RESTORE)
    front = user32.GetForegroundWindow()
    if front == hwnd:
        return True
    # A shell flyout (Search, Start) that holds the foreground refuses the takeover: dismiss it with Escape first.
    if front and process_path(window_pid(front)).lower().endswith(
        ("\\searchhost.exe", "\\startmenuexperiencehost.exe")
    ):
        user32.keybd_event(0x1B, 0, 0, 0)
        user32.keybd_event(0x1B, 0, 2, 0)
        time.sleep(0.5)
        front = user32.GetForegroundWindow()
        if front == hwnd:
            return True
    mine = kernel32.GetCurrentThreadId()
    theirs = window_thread(front) if front else 0
    attached = bool(theirs and theirs != mine and user32.AttachThreadInput(mine, theirs, True))
    try:
        user32.BringWindowToTop(hwnd)
        user32.SetForegroundWindow(hwnd)
    finally:
        if attached:
            user32.AttachThreadInput(mine, theirs, False)
    if user32.GetForegroundWindow() != hwnd:
        # The documented unlock: a keyboard event lets this process set the foreground.
        user32.keybd_event(0x12, 0, 0, 0)
        user32.keybd_event(0x12, 0, 2, 0)
        user32.SetForegroundWindow(hwnd)
    return user32.GetForegroundWindow() == hwnd


def move_window(hwnd: int, x: int, y: int, w: int, h: int) -> None:
    user32.SetWindowPos(hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER)


class _PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [
        ("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD), ("th32ProcessID", wintypes.DWORD),
        ("th32DefaultHeapID", ctypes.c_void_p), ("th32ModuleID", wintypes.DWORD),
        ("cntThreads", wintypes.DWORD), ("th32ParentProcessID", wintypes.DWORD),
        ("pcPriClassBase", ctypes.c_long), ("dwFlags", wintypes.DWORD), ("szExeFile", ctypes.c_wchar * 260),
    ]  # fmt: skip


kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_PROCESSENTRY32W)]
kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(_PROCESSENTRY32W)]


def processes() -> list[tuple[int, int, str]]:
    """(pid, parent pid, executable name) of every process."""
    snap = kernel32.CreateToolhelp32Snapshot(0x2, 0)
    rows: list[tuple[int, int, str]] = []
    if not snap or snap == wintypes.HANDLE(-1).value:
        return rows
    try:
        entry = _PROCESSENTRY32W()
        entry.dwSize = ctypes.sizeof(entry)
        ok = kernel32.Process32FirstW(snap, ctypes.byref(entry))
        while ok:
            rows.append((entry.th32ProcessID, entry.th32ParentProcessID, entry.szExeFile))
            ok = kernel32.Process32NextW(snap, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snap)
    return rows


kernel32.ProcessIdToSessionId.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]


def session_of(pid: int) -> int | None:
    """The Terminal Services session of a process (the interactive desktop is not session 0)."""
    out = wintypes.DWORD()
    if not kernel32.ProcessIdToSessionId(pid, ctypes.byref(out)):
        return None
    return int(out.value)


# ----------------------------------------------------------------------------- pointer input (UM-01)

# dwExtraInfo of every event the sentinel's mouse user posts, so its low-level hook can tell them from anyone
# else's ("CDBU").
MAGIC = 0x43444255
INPUT_MOUSE = 0
MOUSEEVENTF_MOVE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP = 0x1, 0x2, 0x4
MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_ABSOLUTE = 0x4000, 0x8000
SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN = 76, 77, 78, 79
WH_MOUSE_LL = 14
WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP = 0x200, 0x201, 0x202
WM_RBUTTONDOWN, WM_RBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP = 0x204, 0x205, 0x207, 0x208
WM_MOUSEWHEEL, WM_MOUSEHWHEEL = 0x20A, 0x20E
_DOWNS = (WM_LBUTTONDOWN, WM_RBUTTONDOWN, WM_MBUTTONDOWN)
_UPS = (WM_LBUTTONUP, WM_RBUTTONUP, WM_MBUTTONUP)


class _MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", wintypes.LONG), ("dy", wintypes.LONG), ("mouseData", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t),
    ]  # fmt: skip


class _KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", wintypes.WORD), ("wScan", wintypes.WORD), ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t),
    ]  # fmt: skip


class _HARDWAREINPUT(ctypes.Structure):
    _fields_ = [("uMsg", wintypes.DWORD), ("wParamL", wintypes.WORD), ("wParamH", wintypes.WORD)]


class _INPUTUNION(ctypes.Union):
    _fields_ = [("mi", _MOUSEINPUT), ("ki", _KEYBDINPUT), ("hi", _HARDWAREINPUT)]


class _INPUT(ctypes.Structure):
    _anonymous_ = ("u",)
    _fields_ = [("type", wintypes.DWORD), ("u", _INPUTUNION)]


class _MSLLHOOKSTRUCT(ctypes.Structure):
    _fields_ = [
        ("pt", wintypes.POINT), ("mouseData", wintypes.DWORD), ("flags", wintypes.DWORD),
        ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t),
    ]  # fmt: skip


HOOKPROC = ctypes.WINFUNCTYPE(ctypes.c_ssize_t, ctypes.c_int, wintypes.WPARAM, wintypes.LPARAM)
user32.SendInput.argtypes = [wintypes.UINT, ctypes.POINTER(_INPUT), ctypes.c_int]
user32.SendInput.restype = wintypes.UINT
user32.GetSystemMetrics.argtypes = [ctypes.c_int]
user32.ClientToScreen.argtypes = [HWND, ctypes.POINTER(wintypes.POINT)]
user32.SetWindowsHookExW.argtypes = [ctypes.c_int, HOOKPROC, wintypes.HINSTANCE, wintypes.DWORD]
user32.SetWindowsHookExW.restype = ctypes.c_void_p
user32.CallNextHookEx.argtypes = [ctypes.c_void_p, ctypes.c_int, wintypes.WPARAM, wintypes.LPARAM]
user32.CallNextHookEx.restype = ctypes.c_ssize_t
user32.UnhookWindowsHookEx.argtypes = [ctypes.c_void_p]
kernel32.GetModuleHandleW.argtypes = [wintypes.LPCWSTR]
kernel32.GetModuleHandleW.restype = wintypes.HMODULE


def virtual_screen() -> tuple[int, int, int, int]:
    """(left, top, width, height) of the virtual screen, physical pixels."""
    g = user32.GetSystemMetrics
    return g(SM_XVIRTUALSCREEN), g(SM_YVIRTUALSCREEN), g(SM_CXVIRTUALSCREEN), g(SM_CYVIRTUALSCREEN)


def _normalized(value: int, origin: int, extent: int) -> int:
    """The 0..65535 coordinate that Windows maps back to exactly pixel `value` (it truncates norm * extent / 65536)."""
    pixel = min(max(value - origin, 0), extent - 1)
    return min((pixel * 65536 + extent - 1) // extent, 65535)


def send_mouse(x: int, y: int, buttons: int = 0, extra: int = MAGIC) -> bool:
    """One SendInput mouse event at screen pixel (x, y): an absolute move over the virtual screen, with the
    button flags in `buttons`, tagged with `extra` in dwExtraInfo."""
    left, top, width, height = virtual_screen()
    flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | buttons
    event = _INPUT(type=INPUT_MOUSE)
    event.mi = _MOUSEINPUT(
        _normalized(x, left, width), _normalized(y, top, height), 0, flags, 0, extra
    )
    return user32.SendInput(1, ctypes.byref(event), ctypes.sizeof(_INPUT)) == 1


def client_origin(hwnd: int) -> tuple[int, int]:
    """Screen position of a window's client-area origin, physical pixels."""
    p = wintypes.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(p))
    return p.x, p.y


class MouseHook:
    """A WH_MOUSE_LL hook on the calling (GUI) thread. While `enabled`, events whose dwExtraInfo is not `magic`
    (the user's, or any other program's, injected or not) are queued in `foreign` as (ms since the epoch,
    kind), kind being move, drag, down, up or scroll (moves at most one per 20 ms); this process's own
    left button-downs and button-ups are queued in `own_downs` and `own_ups` (monotonic seconds). The callback does nothing else and always
    calls the next hook, and the owner drains the queues from its own timer."""

    def __init__(self, magic: int = MAGIC) -> None:
        self.magic = magic
        self.enabled = False
        self.foreign: list[tuple[float, str]] = []
        self.own_downs: list[float] = []
        self.own_ups: list[float] = []
        self._held = 0
        self._last_move = 0.0
        self._proc = HOOKPROC(self._call)  # kept alive for as long as the hook
        handle = user32.SetWindowsHookExW(
            WH_MOUSE_LL, self._proc, kernel32.GetModuleHandleW(None), 0
        )
        self.handle = handle or None

    def reset(self) -> None:
        self.foreign.clear()
        self.own_downs.clear()
        self.own_ups.clear()
        self._held = 0

    def _call(self, code: int, wparam: int, lparam: int) -> int:
        if code >= 0 and self.enabled:
            try:
                info = ctypes.cast(lparam, ctypes.POINTER(_MSLLHOOKSTRUCT)).contents
                now = time.monotonic()
                if info.dwExtraInfo == self.magic:
                    if wparam == WM_LBUTTONDOWN:
                        self.own_downs.append(now)
                    elif wparam == WM_LBUTTONUP:
                        self.own_ups.append(now)
                elif wparam == WM_MOUSEMOVE:
                    if now - self._last_move >= 0.02:
                        self._last_move = now
                        self.foreign.append((time.time() * 1000, "drag" if self._held else "move"))
                elif wparam in _DOWNS:
                    self._held += 1
                    self.foreign.append((time.time() * 1000, "down"))
                elif wparam in _UPS:
                    self._held = max(0, self._held - 1)
                    self.foreign.append((time.time() * 1000, "up"))
                elif wparam in (WM_MOUSEWHEEL, WM_MOUSEHWHEEL):
                    self.foreign.append((time.time() * 1000, "scroll"))
            except Exception:  # noqa: BLE001 - a hook must never raise
                pass
        return user32.CallNextHookEx(None, code, wparam, lparam)

    def close(self) -> None:
        if self.handle:
            user32.UnhookWindowsHookEx(self.handle)
            self.handle = None


# ----------------------------------------------------------------------------- precise waiting

CREATE_WAITABLE_TIMER_HIGH_RESOLUTION = 0x2
TIMER_ALL_ACCESS = 0x1F0003
BUSY_TAIL_S = (
    0.0015  # the last stretch of a wait is spun: even a high-resolution timer wakes ~0.5-1 ms late
)
kernel32.CreateWaitableTimerExW.restype = wintypes.HANDLE
kernel32.CreateWaitableTimerExW.argtypes = [
    ctypes.c_void_p,
    wintypes.LPCWSTR,
    wintypes.DWORD,
    wintypes.DWORD,
]
kernel32.SetWaitableTimer.argtypes = [
    wintypes.HANDLE, ctypes.POINTER(ctypes.c_longlong), wintypes.LONG, ctypes.c_void_p, ctypes.c_void_p, wintypes.BOOL
]  # fmt: skip
kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
kernel32.WaitForSingleObject.restype = wintypes.DWORD


class PreciseWait:
    """Sleep until a `time.monotonic()` deadline with about 1 ms accuracy. Plain `time.sleep` and WM_TIMER wake
    on the ~15.6 ms system tick, too coarse for an 8 ms pointer step. A high-resolution waitable timer (Windows
    10 1803 and later) covers all but the last BUSY_TAIL_S, which is spun; without the timer the wait is a
    1 ms sleep loop. One instance per thread."""

    def __init__(self) -> None:
        self.timer = (
            kernel32.CreateWaitableTimerExW(
                None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS
            )
            or None
        )

    def until(self, deadline: float) -> None:
        coarse = deadline - BUSY_TAIL_S
        remaining = coarse - time.monotonic()
        if remaining > 0:
            if self.timer:
                due = ctypes.c_longlong(-int(remaining * 1e7))  # relative, 100 ns units
                if kernel32.SetWaitableTimer(self.timer, ctypes.byref(due), 0, None, None, False):
                    kernel32.WaitForSingleObject(self.timer, int(remaining * 1000) + 20)
            else:
                while time.monotonic() < coarse:
                    time.sleep(0.001)
        while time.monotonic() < deadline:
            time.sleep(0)  # spin, yielding the GIL

    def close(self) -> None:
        if self.timer:
            kernel32.CloseHandle(self.timer)
            self.timer = None


def distance(a: tuple[float, float], b: tuple[float, float]) -> float:
    return math.hypot(a[0] - b[0], a[1] - b[1])
