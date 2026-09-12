"""Stable command exit codes."""

OK = 0
USAGE = 1
VALIDATION = 2
HARNESS = 3
TIMEOUT = 4
INTERRUPTED = 5
CLEANUP = 6
BUDGET = 7
HARD_ABORT = 130


def dominant(original: int, cleanup_failed: bool) -> int:
    """Return the final status while preserving cleanup as the strongest failure."""

    return CLEANUP if cleanup_failed else original
