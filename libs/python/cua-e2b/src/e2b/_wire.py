"""The checks E2B's REST API makes on a request, with its texts.

E2B validates every request against its OpenAPI schema on the server, so a
typed argument of the wrong kind fails with `SandboxException("400: validation
error: ...")` rather than a `TypeError`, and some values (an unknown sandbox
id, a metrics window that ends before it starts) have their own refusals. Code
written for E2B branches on those classes and texts; cua-e2b has no REST
server, so it makes the same checks on the arguments, before it touches a
Space, and raises the same exception with the same text.
"""

from __future__ import annotations

import ipaddress
import re
from datetime import datetime, timezone
from typing import Any, Mapping, Optional

from e2b.exceptions import SandboxException

_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]*$")
_TEMPLATE_BAD = re.compile(r"[\s!\"#$%&'()*,;<>?\[\]\\^`{|}]")


def bad_request(text: str) -> SandboxException:
    """E2B's `400: <text>`."""
    return SandboxException(f"400: {text}")


def invalid(schema: str, pointer: str, what: str) -> SandboxException:
    """A body that does not match its schema (`kin-openapi`'s wording)."""
    return bad_request(
        "validation error: error in openapi3filter.RequestError: request body has an error: "
        f'doesn\'t match schema #/components/schemas/{schema}: Error at "{pointer}": {what}'
    )


def invalid_param(name: str, what: str) -> SandboxException:
    """A query parameter that does not match its schema."""
    return bad_request(
        "validation error: error in openapi3filter.RequestError: "
        f'parameter "{name}" in query has an error: {what}'
    )


def _is_int(v: Any) -> bool:
    if isinstance(v, bool):
        return False
    return isinstance(v, int) or (isinstance(v, float) and v.is_integer())


def check_integer(schema: str, pointer: str, v: Any, minimum: Optional[int] = None) -> None:
    """`v` is a JSON integer (a bool, string or fraction is not; `None` is not
    nullable) of at least `minimum`."""
    if v is None:
        raise invalid(schema, pointer, "Value is not nullable")
    if not _is_int(v):
        raise invalid(schema, pointer, "value must be an integer")
    if minimum is not None and v < minimum:
        raise invalid(schema, pointer, f"number must be at least {minimum}")


def check_string(schema: str, pointer: str, v: Any) -> None:
    if v is None:
        raise invalid(schema, pointer, "Value is not nullable")
    if not isinstance(v, str):
        raise invalid(schema, pointer, "value must be a string")


def check_bool(schema: str, pointer: str, v: Any) -> None:
    if v is None:
        raise invalid(schema, pointer, "Value is not nullable")
    if not isinstance(v, bool):
        raise invalid(schema, pointer, "value must be a boolean")


def check_string_map(schema: str, pointer: str, v: Any) -> None:
    """An object whose values are strings (metadata, environment variables)."""
    if v is None:
        return
    if not isinstance(v, Mapping):
        raise invalid(schema, pointer, "value must be an object")
    for k, x in v.items():
        check_string(schema, f"{pointer}/{k}", x)


def check_string_list(schema: str, pointer: str, v: Any) -> None:
    if v is None:
        return
    if not isinstance(v, (list, tuple)):
        raise invalid(schema, pointer, "value must be an array")
    for i, x in enumerate(v):
        check_string(schema, f"{pointer}/{i}", x)


def sandbox_id(sandbox_id: Any) -> None:
    """A sandbox id in a path: E2B refuses a malformed one before looking it up."""
    if isinstance(sandbox_id, str) and sandbox_id and not _ID.match(sandbox_id):
        raise bad_request("Invalid sandbox ID")


def template_ref(template: Any) -> None:
    if isinstance(template, str) and (not template or _TEMPLATE_BAD.search(template)):
        raise bad_request(f"Invalid template reference: invalid template ID: {template}")


def snapshot_ref(name: Any) -> None:
    if isinstance(name, str) and (not name or _TEMPLATE_BAD.search(name)):
        raise bad_request(f"Invalid template ID: invalid template ID: {name}")


def is_ip_or_cidr(entry: Any) -> bool:
    if not isinstance(entry, str):
        return False
    try:
        ipaddress.ip_network(entry, strict=False)
        return "/" not in entry or entry.rpartition("/")[2].isdigit()
    except ValueError:
        return False


def denied_cidr(entry: str) -> None:
    """E2B parses every `deny_out` entry as an address or CIDR block."""
    if not is_ip_or_cidr(entry):
        raise bad_request(f"invalid denied CIDR {entry}")


def metrics_window(start: Optional[datetime], end: Optional[datetime]) -> None:
    """A metrics window that ends before it starts, or starts in the future."""
    now = datetime.now(timezone.utc)

    def aware(t: datetime) -> datetime:
        return t if t.tzinfo else t.astimezone()

    if (start is not None and aware(start) > now) or (
        start is not None and end is not None and aware(end) < aware(start)
    ):
        raise bad_request("Invalid time range for metrics")
