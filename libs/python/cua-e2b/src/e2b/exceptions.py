"""The exception classes and message helpers of the e2b SDK's public API (2.42, 2.56
and 2.58), with the same names, bases, `status_code` fields and message formats, so
`except` clauses and message checks written for e2b keep working.
(The e2b SDK is MIT-licensed; this module matches its public API.)
"""

from typing import Optional

# Messages of the 502 the proxy in front of envd answers with when the sandbox does
# not exist any more, or runs but envd's port is not open. A cua-e2b sandbox raises
# the first one when its Space is gone.
SANDBOX_NOT_FOUND_MESSAGE = "was not found"
SANDBOX_PORT_NOT_OPEN_MESSAGE = "port is not open"


def is_sandbox_not_found_message(message: str) -> bool:
    return SANDBOX_NOT_FOUND_MESSAGE in message


def is_sandbox_port_not_open_message(message: str) -> bool:
    return SANDBOX_PORT_NOT_OPEN_MESSAGE in message


def format_sandbox_timeout_exception(message: str):
    return TimeoutException(
        f"{message}: This error is likely due to sandbox timeout. You can modify the sandbox timeout by passing 'timeout' when starting the sandbox or calling '.set_timeout' on the sandbox with the desired timeout."
    )


def format_sandbox_unavailable_exception(message: str) -> Exception:
    """Map an UNAVAILABLE answer about a sandbox: "was not found" (killed, paused
    or past its timeout) is a `SandboxNotRunningException`; "port is not open"
    (running, but envd unreachable) and anything else a
    `SandboxUnreachableException`."""
    if is_sandbox_not_found_message(message):
        return SandboxNotRunningException(
            f"{message}: The sandbox was killed or reached its timeout. You can modify the sandbox timeout by passing 'timeout' when starting the sandbox or calling '.set_timeout' on the sandbox with the desired timeout."
        )
    if is_sandbox_port_not_open_message(message):
        return SandboxUnreachableException(
            f"{message}: The sandbox is running but envd inside it could not be reached (e.g. its network is down). Check the sandbox state with 'Sandbox.get_info()'."
        )
    return SandboxUnreachableException(
        f"{message}: The sandbox could not be reached and its state is unknown — it may have been killed or reached its timeout, or the connection to it was lost. Check the sandbox state with 'Sandbox.get_info()'."
    )


def format_request_timeout_error() -> Exception:
    return TimeoutException(
        "Request timed out — the 'request_timeout' option can be used to increase this timeout",
    )


class SandboxException(Exception):
    """Base class for all sandbox errors."""

    status_code: Optional[int] = None

    def __init__(self, *args, status_code: Optional[int] = None):
        super().__init__(*args)
        self.status_code = status_code


class TimeoutException(SandboxException):
    """Raised when a timeout occurs (sandbox lifetime, request or command)."""


class SandboxNotRunningException(TimeoutException):
    """Raised when the sandbox is not running any more (killed, paused or past
    its timeout)."""


class SandboxUnreachableException(TimeoutException):
    """Raised when the sandbox could not be reached and is not confirmed
    stopped."""


class InvalidArgumentException(SandboxException):
    """Raised when an invalid argument is provided."""


class NotEnoughSpaceException(SandboxException):
    """Raised when there is not enough disk space."""


class NotFoundException(SandboxException):
    """Raised when a resource is not found."""


class FileNotFoundException(NotFoundException):
    """Raised when a file or directory is not found inside a sandbox."""


class SandboxNotFoundException(NotFoundException):
    """Raised when a sandbox is not found (it doesn't exist or is no longer running)."""


class AuthenticationException(Exception):
    """Raised when authentication fails."""


class GitAuthException(AuthenticationException):
    """Raised when git authentication fails."""


class GitUpstreamException(SandboxException):
    """Raised when git upstream tracking is missing."""


class TemplateException(SandboxException):
    """Raised when a template can't be used."""


class RateLimitException(SandboxException):
    """Raised when the API rate limit is exceeded."""

    def __init__(self, *args):
        super().__init__(*args, status_code=429)


class ServiceBusyException(Exception):
    """Raised when the service is temporarily busy (HTTP 503); nothing was
    changed, so the call can be retried."""

    status_code = 503


class BuildException(Exception):
    """Raised when the build fails."""


class FileUploadException(BuildException):
    """Raised when the file upload fails."""


class VolumeException(Exception):
    """Base class for all volume errors."""


class VolumeNotFoundException(NotFoundException):
    """Raised when a volume is not found."""


class VolumePathNotFoundException(NotFoundException):
    """Raised when a file or directory is not found inside a volume."""


class SecretException(Exception):
    """Base class for all secret errors."""


class SecretNotFoundException(SecretException):
    """Raised when a secret is not found."""
