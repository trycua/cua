"""Shared pieces of `Secret` / `AsyncSecret`.

E2B Secrets are an account-level, write-only store behind E2B's API: a value is
created once, referenced from a sandbox as `${e2b.secrets.<name>}`, and substituted
by E2B's egress proxy. A Cua Space has no such store or proxy, so every operation
that would talk to it (`create`, `update`, `get_info`, `list`, `exists`, `destroy`)
validates its arguments and the connection options exactly as e2b does and then
raises `SecretException` with the reason and the alternatives. Nothing is stored
and nothing pretends to be: `Secret.fill` and `Secret.iam_token`, which only build
values, work for real.
"""

from __future__ import annotations

import re
from typing import Any, Dict, Optional

from e2b.connection_config import ApiParams, ClientFactory, ConnectionConfig
from e2b.exceptions import InvalidArgumentException, SecretException
from e2b.paginator import PaginatorBase
from e2b.sandbox.sandbox_api import SandboxIamToken, SandboxIamTokenType
from e2b.secret.types import SecretInfo

NO_SECRET_STORE = (
    "cua-e2b has no account-level secret store: E2B Secrets are held and substituted by "
    "E2B's own service, and a Cua Space has no equivalent that can resolve "
    "'${e2b.secrets.<name>}' placeholders. Pass the value with envs= (Sandbox.create, "
    "commands.run) or keep it in Keyvault and let the job read it there."
)


class SecretPaginatorBase(PaginatorBase[SecretInfo, ApiParams]):
    pass


INVALID_SECRET_NAME_CHARS = re.compile(r"[{}\x00-\x1f\x7f-\x9f]")


def _validate_secret_name(name: str) -> None:
    if not isinstance(name, str) or not name or INVALID_SECRET_NAME_CHARS.search(name):
        raise InvalidArgumentException(
            f"secret name {name!r} is not usable: a secret name must be a "
            "non-empty string and cannot contain '{', '}' or control "
            "characters, because it is interpolated into the "
            "'${e2b.secrets.<name>}' placeholder the runtime resolves."
        )


def _validate_secret_ref(secret: str) -> None:
    if not isinstance(secret, str) or not secret:
        raise InvalidArgumentException(
            f"secret {secret!r} is not usable: expected the ID or name of a secret."
        )


def _validate_secret_value(value: str) -> None:
    if not isinstance(value, str):
        raise InvalidArgumentException(
            f"secret value must be a string, got {type(value).__name__}."
        )


def _validate_metadata(metadata: Optional[Dict[str, str]]) -> None:
    if metadata is None:
        return
    if not isinstance(metadata, dict) or not all(
        isinstance(k, str) and isinstance(v, str) for k, v in metadata.items()
    ):
        raise InvalidArgumentException("secret metadata must be a dict of string keys and values.")


def _check_connection(opts: Any) -> None:
    """Build the connection config e2b builds first on every call, so invalid
    connection options (`retries`, `http_version`, ...) fail the same way."""
    ConnectionConfig(**opts)


def _no_secret_store(operation: str) -> SecretException:
    return SecretException(f"Secret.{operation}: {NO_SECRET_STORE}")


class SecretBase(ClientFactory):
    """Module for managing secrets and workload identity helpers.

    Secret values are write-only: they would be accepted by `create` and `update` but
    never returned by any read surface. Here every store operation raises
    `SecretException` (see the module docstring); `fill` and `iam_token` work.
    """

    @staticmethod
    def fill(secret: str) -> str:
        """The placeholder a runtime resolves to the secret's current value,
        `'${e2b.secrets.<name>}'`. A local formatting helper: it does not check that
        the secret exists."""
        _validate_secret_name(secret)
        return f"${{e2b.secrets.{secret}}}"

    @staticmethod
    def iam_token(*, audience: str, token_type: SandboxIamTokenType) -> SandboxIamToken:
        """Define a workload identity token to pass to `iam.tokens` when creating a
        sandbox.

        :param audience: Audience of the workload token, stored exactly as provided.
        :param token_type: Workload token type.
        """
        return SandboxIamToken(audience=audience, token_type=token_type)
