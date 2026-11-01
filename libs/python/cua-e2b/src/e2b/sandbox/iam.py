"""Workload identity (iam) helpers: the `${e2b.identity.tokens.<name>}` placeholders
that network transforms reference."""

from __future__ import annotations

import re
from typing import Iterable, Iterator, Mapping

from e2b.exceptions import InvalidArgumentException

# Characters a workload token name cannot carry: a brace would end or open a
# placeholder early (resolving a different token than the one named), and control
# characters cannot appear in an HTTP header value.
INVALID_IAM_TOKEN_NAME_CHARS = re.compile(r"[{}\x00-\x1f\x7f-\x9f]")


def validate_iam_token_name(name: str) -> None:
    if not isinstance(name, str) or not name or INVALID_IAM_TOKEN_NAME_CHARS.search(name):
        raise InvalidArgumentException(
            f"iam token name {name!r} is not usable: a token name must be a "
            "non-empty string and cannot contain '{', '}' or control "
            "characters, because it is interpolated into the "
            "'${e2b.identity.tokens.<name>}' placeholder the egress proxy "
            "resolves."
        )


def iam_token_placeholder(name: str) -> str:
    validate_iam_token_name(name)

    return f"${{e2b.identity.tokens.{name}}}"


class IamTokenPlaceholders(Mapping[str, str]):
    """Workload token placeholders keyed by token name, as a network `transform`
    callable sees them. Every lookup resolves through `__getitem__`, so an
    unregistered name raises instead of yielding `None`; iteration and `in` see only
    the registered names. `validate=False` (the update-network call, whose payload
    carries no `iam` config) resolves any name."""

    def __init__(self, names: Iterable[str], *, validate: bool) -> None:
        self._names = tuple(dict.fromkeys(names))
        self._validate = validate

    def __getitem__(self, name: str) -> str:
        if not self._validate or name in self._names:
            return iam_token_placeholder(name)

        hint = (
            f"Registered tokens: {', '.join(repr(known) for known in self._names)}."
            if self._names
            else (
                f"Pass it to Sandbox.create as iam={{'tokens': "
                f"{{{name!r}: Secret.iam_token(audience=..., token_type=...)}}}}."
            )
        )
        raise InvalidArgumentException(
            f"Network transform references iam token {name!r}, which is not registered. {hint}"
        )

    def __contains__(self, name: object) -> bool:
        return name in self._names

    def __iter__(self) -> Iterator[str]:
        return iter(self._names)

    def __len__(self) -> int:
        return len(self._names)
