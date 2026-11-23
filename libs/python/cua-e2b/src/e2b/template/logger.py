"""Build log entries and the default build logger.

The default logger prints `0.4s | 12:00:01 INFO  message` lines and, on a
terminal, an animated "Building" timer, as e2b's does (plain ANSI instead of
a terminal-UI library, so the package has no dependency for it).

The e2b SDK is MIT-licensed; this module matches its public API.
"""

import sys
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime
from typing import Callable, Dict, Literal, Optional, TypedDict

from e2b.template.utils import strip_ansi_escape_codes

"""Log entry severity levels."""
LogEntryLevel = Literal["debug", "info", "warn", "error"]


@dataclass
class LogEntry:
    """A single log entry of a template build."""

    timestamp: datetime
    level: LogEntryLevel
    message: str

    def __post_init__(self):
        self.message = strip_ansi_escape_codes(self.message)

    def __str__(self) -> str:
        return f"[{self.timestamp.isoformat()}] [{self.level}] {self.message}"


@dataclass
class LogEntryStart(LogEntry):
    """Marks the start of a build."""

    level: LogEntryLevel = field(default="debug", init=False)


@dataclass
class LogEntryEnd(LogEntry):
    """Marks the end of a build."""

    level: LogEntryLevel = field(default="debug", init=False)


TIMER_UPDATE_INTERVAL_MS = 150

DEFAULT_LEVEL: LogEntryLevel = "info"

# Label and ANSI colour of each level.
levels: Dict[str, tuple] = {
    "error": ("ERROR", "\x1b[31m"),
    "warn": ("WARN ", "\x1b[38;5;202m"),
    "info": ("INFO ", "\x1b[38;5;208m"),
    "debug": ("DEBUG", "\x1b[90m"),
}

level_order = {"debug": 0, "info": 1, "warn": 2, "error": 3}

_RESET = "\x1b[0m"
_DIM = "\x1b[2m"


def set_interval(func, interval):
    """Calls `func` every `interval` seconds on a daemon thread; returns the
    function that stops it (JavaScript's `setInterval`)."""
    stopped = threading.Event()

    def loop():
        while not stopped.is_set():
            if stopped.wait(interval):
                break
            if not stopped.is_set():
                func()

    threading.Thread(target=loop, daemon=True).start()
    return stopped.set


class DefaultBuildLoggerInitialState(TypedDict):
    start_time: float
    animation_frame: int
    timer: Optional[Callable[[], None]]


class DefaultBuildLogger:
    """Prints build logs; see `default_build_logger`."""

    def __init__(self, min_level: Optional[LogEntryLevel] = None):
        self.__min_level = min_level if min_level is not None else DEFAULT_LEVEL
        self.__reset_initial_state()

    def logger(self, log):
        if isinstance(log, LogEntryStart):
            self.__start_timer()
            return

        if isinstance(log, LogEntryEnd):
            if self.__state["timer"] is not None:
                self.__state["timer"]()
            return

        if level_order[log.level] < level_order[self.__min_level]:
            return

        print(self.__format_log_line(log), flush=True)
        self.__update_timer()

    def __reset_initial_state(self, timer: Optional[Callable[[], None]] = None):
        self.__state: DefaultBuildLoggerInitialState = {
            "start_time": time.time(),
            "animation_frame": 0,
            "timer": timer,
        }

    def __format_timer_line(self) -> str:
        return f"{time.time() - self.__state['start_time']:.1f}s"

    def __animate_status(self) -> str:
        frames = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"]
        return frames[self.__state["animation_frame"] % len(frames)]

    def __format_log_line(self, line: LogEntry) -> str:
        label, colour = levels.get(line.level, levels[DEFAULT_LEVEL])
        stamp = line.timestamp.strftime("%H:%M:%S")
        tty = sys.stdout.isatty()
        if tty:
            stamp, label = f"{_DIM}{stamp}{_RESET}", f"{colour}{label}{_RESET}"
        return f"{self.__format_timer_line().ljust(5)} | {stamp} {label} {line.message}"

    def __start_timer(self):
        if not sys.stdout.isatty():
            return
        stop_timer = set_interval(self.__update_timer, TIMER_UPDATE_INTERVAL_MS / 1000.0)
        self.__reset_initial_state(stop_timer)
        self.__update_timer()

    def __update_timer(self):
        if not sys.stdout.isatty():
            return
        self.__state["animation_frame"] += 1
        print(
            f"{self.__animate_status()} Building {self.__format_timer_line()}",
            end="\r",
            flush=True,
        )


def default_build_logger(
    min_level: Optional[LogEntryLevel] = None,
) -> Callable[[LogEntry], None]:
    """A logger for `on_build_logs` that prints the build log with a timer.

    :param min_level: minimum level shown (default `"info"`)
    """
    return DefaultBuildLogger(min_level).logger
