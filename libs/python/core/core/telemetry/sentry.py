"""Sentry error tracking for CUA SDK.

Provides error tracking, crash reporting, and performance monitoring.
"""

from __future__ import annotations

import logging
import os
from threading import Lock
from typing import Any, Callable, Dict, Optional, TypeVar

logger = logging.getLogger("core.telemetry.sentry")

# Type vars for decorator
F = TypeVar("F", bound=Callable[..., Any])

# Public Sentry DSN for CUA SDK
# This is intentionally public - it only allows sending events to our project
PUBLIC_SENTRY_DSN = (
    "https://d3fed5d05939cb8343dfcab552948d17@o4510539933483008.ingest.us.sentry.io/4510551288381440"
)

# Lazy initialization state
_initialized = False
_init_lock = Lock()
_sentry_sdk: Optional[Any] = None


def is_sentry_enabled() -> bool:
    """Check if Sentry is enabled.

    Returns True unless CUA_TELEMETRY_DISABLED is set to a truthy value.
    """
    disabled = os.environ.get("CUA_TELEMETRY_DISABLED", "").lower()
    return disabled not in {"1", "true", "yes", "on"}


def _get_sentry_dsn() -> str:
    """Get the Sentry DSN."""
    return os.environ.get("CUA_SENTRY_DSN", PUBLIC_SENTRY_DSN)


def _get_sdk_version() -> str:
    """Get the CUA SDK version."""
    try:
        from core import __version__
        return __version__
    except ImportError:
        return "unknown"


def _get_environment() -> str:
    """Get the environment name."""
    return os.environ.get("CUA_ENVIRONMENT", "production")


def _initialize_sentry() -> bool:
    """Initialize Sentry SDK.

    Returns True if initialization succeeded, False otherwise.
    Thread-safe via lock.
    """
    global _initialized, _sentry_sdk

    if _initialized:
        return True

    with _init_lock:
        # Double-check after acquiring lock
        if _initialized:
            return True

        if not is_sentry_enabled():
            logger.debug("Sentry disabled via CUA_TELEMETRY_DISABLED")
            return False

        try:
            import sentry_sdk
            from sentry_sdk.integrations.logging import LoggingIntegration

            _sentry_sdk = sentry_sdk

            # Configure logging integration to capture warnings and above
            logging_integration = LoggingIntegration(
                level=logging.WARNING,
                event_level=logging.ERROR,
            )

            sentry_sdk.init(
                dsn=_get_sentry_dsn(),
                environment=_get_environment(),
                release=f"cua-sdk@{_get_sdk_version()}",
                # Performance monitoring
                traces_sample_rate=0.1,  # 10% of transactions
                # Only send errors, not all transactions
                profiles_sample_rate=0.0,
                # Integrations
                integrations=[logging_integration],
                # Don't send PII
                send_default_pii=False,
                # Attach stacktrace to messages
                attach_stacktrace=True,
                # Set max breadcrumbs
                max_breadcrumbs=50,
                # Before send hook to filter sensitive data
                before_send=_before_send,
            )

            # Set default tags
            sentry_sdk.set_tag("sdk.name", "cua-sdk")
            sentry_sdk.set_tag("sdk.version", _get_sdk_version())

            _initialized = True
            logger.info("Sentry initialized")
            return True

        except ImportError as e:
            logger.warning(
                f"Sentry SDK not installed: {e}. "
                "Install with: pip install sentry-sdk"
            )
            return False
        except Exception as e:
            logger.warning(f"Failed to initialize Sentry: {e}")
            return False


def _before_send(event: Dict[str, Any], hint: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    """Filter sensitive data before sending to Sentry."""
    # Remove any potential PII from breadcrumbs
    if "breadcrumbs" in event:
        for breadcrumb in event.get("breadcrumbs", {}).get("values", []):
            # Filter out any data that might contain prompts or user content
            if "data" in breadcrumb:
                data = breadcrumb["data"]
                for key in list(data.keys()):
                    if any(
                        sensitive in key.lower()
                        for sensitive in ["prompt", "content", "message", "text", "input", "output"]
                    ):
                        data[key] = "[FILTERED]"

    # Remove sensitive context
    if "contexts" in event:
        contexts = event["contexts"]
        for key in list(contexts.keys()):
            if any(
                sensitive in key.lower()
                for sensitive in ["prompt", "content", "message"]
            ):
                del contexts[key]

    return event


# --- Public API ---


def capture_exception(
    error: Optional[BaseException] = None,
    context: Optional[Dict[str, Any]] = None,
) -> Optional[str]:
    """Capture an exception and send to Sentry.

    Args:
        error: The exception to capture. If None, captures current exception.
        context: Additional context to attach to the event.

    Returns:
        Event ID if captured, None otherwise.
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        return None

    try:
        with _sentry_sdk.push_scope() as scope:
            if context:
                for key, value in context.items():
                    # Don't include potentially sensitive data
                    if not any(
                        sensitive in key.lower()
                        for sensitive in ["prompt", "content", "message", "text"]
                    ):
                        scope.set_extra(key, value)

            return _sentry_sdk.capture_exception(error)
    except Exception as e:
        logger.debug(f"Failed to capture exception: {e}")
        return None


def capture_message(
    message: str,
    level: str = "info",
    context: Optional[Dict[str, Any]] = None,
) -> Optional[str]:
    """Capture a message and send to Sentry.

    Args:
        message: The message to capture.
        level: Log level (debug, info, warning, error, fatal).
        context: Additional context to attach to the event.

    Returns:
        Event ID if captured, None otherwise.
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        return None

    try:
        with _sentry_sdk.push_scope() as scope:
            if context:
                for key, value in context.items():
                    if not any(
                        sensitive in key.lower()
                        for sensitive in ["prompt", "content", "message", "text"]
                    ):
                        scope.set_extra(key, value)

            return _sentry_sdk.capture_message(message, level=level)
    except Exception as e:
        logger.debug(f"Failed to capture message: {e}")
        return None


def add_breadcrumb(
    category: str,
    message: str,
    level: str = "info",
    data: Optional[Dict[str, Any]] = None,
) -> None:
    """Add a breadcrumb for debugging context.

    Args:
        category: Breadcrumb category (e.g., "agent", "computer")
        message: Breadcrumb message
        level: Log level
        data: Additional data (will be filtered for sensitive content)
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        return

    try:
        # Filter sensitive data
        filtered_data = {}
        if data:
            for key, value in data.items():
                if not any(
                    sensitive in key.lower()
                    for sensitive in ["prompt", "content", "message", "text", "input", "output"]
                ):
                    filtered_data[key] = value

        _sentry_sdk.add_breadcrumb(
            category=category,
            message=message,
            level=level,
            data=filtered_data if filtered_data else None,
        )
    except Exception as e:
        logger.debug(f"Failed to add breadcrumb: {e}")


def set_user(user_id: str) -> None:
    """Set the user context (installation ID, not PII).

    Args:
        user_id: Anonymous user/installation ID
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        return

    try:
        _sentry_sdk.set_user({"id": user_id})
    except Exception as e:
        logger.debug(f"Failed to set user: {e}")


def set_tag(key: str, value: str) -> None:
    """Set a tag on all future events.

    Args:
        key: Tag key
        value: Tag value
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        return

    try:
        _sentry_sdk.set_tag(key, value)
    except Exception as e:
        logger.debug(f"Failed to set tag: {e}")


def set_context(name: str, data: Dict[str, Any]) -> None:
    """Set a context on all future events.

    Args:
        name: Context name
        data: Context data (will be filtered for sensitive content)
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        return

    try:
        # Filter sensitive data
        filtered_data = {}
        for key, value in data.items():
            if not any(
                sensitive in key.lower()
                for sensitive in ["prompt", "content", "message", "text", "input", "output"]
            ):
                filtered_data[key] = value

        _sentry_sdk.set_context(name, filtered_data)
    except Exception as e:
        logger.debug(f"Failed to set context: {e}")


def start_transaction(
    name: str,
    op: str,
    description: Optional[str] = None,
) -> Any:
    """Start a performance transaction.

    Args:
        name: Transaction name
        op: Operation type (e.g., "agent.run", "computer.action")
        description: Optional description

    Returns:
        Transaction object (use as context manager)

    Example:
        with start_transaction("agent.run", "agent") as transaction:
            # do work
            transaction.set_tag("model", "claude-3")
    """
    if not _initialize_sentry() or _sentry_sdk is None:
        # Return a no-op context manager
        from contextlib import nullcontext
        return nullcontext()

    try:
        return _sentry_sdk.start_transaction(
            name=name,
            op=op,
            description=description,
        )
    except Exception as e:
        logger.debug(f"Failed to start transaction: {e}")
        from contextlib import nullcontext
        return nullcontext()


def flush(timeout: float = 2.0) -> None:
    """Flush pending events to Sentry.

    Args:
        timeout: Timeout in seconds
    """
    if not _initialized or _sentry_sdk is None:
        return

    try:
        _sentry_sdk.flush(timeout=timeout)
    except Exception as e:
        logger.debug(f"Failed to flush Sentry: {e}")
