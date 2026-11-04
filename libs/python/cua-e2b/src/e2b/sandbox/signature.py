"""Signed file URLs: the v1 signature of a path, operation, user and access token."""

from __future__ import annotations

import base64
import hashlib
import time
from typing import Literal, Optional, TypedDict

Operation = Literal["read", "write"]


class Signature(TypedDict):
    signature: str
    expiration: Optional[int]  # Unix timestamp or None


def get_signature(
    path: str,
    operation: Operation,
    user: Optional[str],
    envd_access_token: Optional[str],
    expiration_in_seconds: Optional[int] = None,
) -> Signature:
    """`v1_` + the unpadded base64 SHA-256 of `path:operation:user:token[:expiration]`
    (a missing user is the empty string), with the Unix expiration time when
    `expiration_in_seconds` is given."""
    if not envd_access_token:
        raise ValueError("Access token is not set and signature cannot be generated!")

    expiration = (
        int(time.time()) + expiration_in_seconds if expiration_in_seconds is not None else None
    )

    if user is None:
        user = ""

    raw = (
        f"{path}:{operation}:{user}:{envd_access_token}"
        if expiration is None
        else f"{path}:{operation}:{user}:{envd_access_token}:{expiration}"
    )

    digest = hashlib.sha256(raw.encode("utf-8")).digest()
    encoded = base64.b64encode(digest).rstrip(b"=").decode("ascii")

    return {"signature": f"v1_{encoded}", "expiration": expiration}
