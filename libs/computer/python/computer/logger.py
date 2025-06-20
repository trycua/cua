"""Logging utilities for the Computer module."""

import logging
from enum import IntEnum


# Keep LogLevel for backward compatibility, but it will be deprecated
class LogLevel(IntEnum):
    """Log levels for logging. Deprecated - use standard logging levels instead."""

    QUIET = 0  # Only warnings and errors
    NORMAL = 1  # Info level, standard output
    VERBOSE = 2  # More detailed information
    DEBUG = 3  # Full debug information


# Map LogLevel to standard logging levels for backward compatibility
LOGLEVEL_MAP = {
    LogLevel.QUIET: logging.WARNING,
    LogLevel.NORMAL: logging.INFO,
    LogLevel.VERBOSE: logging.DEBUG,
    LogLevel.DEBUG: logging.DEBUG,
}


class Logger:
    """Logger class for Computer."""

    def __init__(self, name: str, verbosity: int):
        """Initialize the logger.

        Args:
            name: The name of the logger.
            verbosity: The log level (use standard logging levels like logging.INFO).
                       For backward compatibility, LogLevel enum values are also accepted.
        """
        self.logger = logging.getLogger(name)

        # Convert LogLevel enum to standard logging level if needed
        if isinstance(verbosity, LogLevel):
            self.verbosity = LOGLEVEL_MAP.get(verbosity, logging.INFO)
        else:
            self.verbosity = verbosity

        self._configure()

    def _configure(self):
        """Configure the logger based on log level."""
        # Set the logging level directly
        self.logger.setLevel(self.verbosity)

        # Log the verbosity level that was set
        if self.verbosity <= logging.DEBUG:
            self.logger.info("Logger set to DEBUG level")
        elif self.verbosity <= logging.INFO:
            self.logger.info("Logger set to INFO level")
        elif self.verbosity <= logging.WARNING:
            self.logger.warning("Logger set to WARNING level")
        elif self.verbosity <= logging.ERROR:
            self.logger.warning("Logger set to ERROR level")
        elif self.verbosity <= logging.CRITICAL:
            self.logger.warning("Logger set to CRITICAL level")

    def debug(self, message: str):
        """Log a debug message if log level is DEBUG or lower."""
        self.logger.debug(message)

    def info(self, message: str):
        """Log an info message if log level is INFO or lower."""
        self.logger.info(message)

    def verbose(self, message: str):
        """Log a verbose message between INFO and DEBUG levels."""
        # Since there's no standard verbose level,
        # use debug level with [VERBOSE] prefix for backward compatibility
        self.logger.debug(f"[VERBOSE] {message}")

    def warning(self, message: str):
        """Log a warning message."""
        self.logger.warning(message)

    def error(self, message: str):
        """Log an error message."""
        self.logger.error(message)
