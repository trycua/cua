"""Secrets: E2B's account-level secret store has no Cua counterpart, so the store
operations validate like e2b's and then raise `SecretException` with the reason;
the placeholder helpers work. No network."""

from __future__ import annotations

import e2b
import pytest
from e2b import (
    AsyncSecret,
    AsyncSecretPaginator,
    InvalidArgumentException,
    Secret,
    SecretException,
    SecretInfo,
    SecretNotFoundException,
    SecretPaginator,
)
from e2b.secret.base import NO_SECRET_STORE

SYNC_STORE_CALLS = [
    ("create", lambda S, **o: S.create("openai-key", "sk-test", {"team": "x"}, **o)),
    ("update", lambda S, **o: S.update("openai-key", "sk-new", **o)),
    ("get_info", lambda S, **o: S.get_info("openai-key", **o)),
    ("exists", lambda S, **o: S.exists("openai-key", **o)),
    ("destroy", lambda S, **o: S.destroy("openai-key", **o)),
]


@pytest.mark.parametrize("operation,call", SYNC_STORE_CALLS)
def test_sync_store_operations_raise_with_the_reason(operation, call):
    with pytest.raises(SecretException) as info:
        call(Secret)
    text = str(info.value)
    assert text == f"Secret.{operation}: {NO_SECRET_STORE}"
    assert "envs=" in text and "Keyvault" in text
    assert not isinstance(info.value, SecretNotFoundException)
    # Not a SandboxException, like e2b's SecretException.
    assert not isinstance(info.value, e2b.SandboxException)


@pytest.mark.parametrize("operation,call", SYNC_STORE_CALLS)
async def test_async_store_operations_raise_with_the_reason(operation, call):
    with pytest.raises(SecretException) as info:
        await call(AsyncSecret)
    assert str(info.value) == f"Secret.{operation}: {NO_SECRET_STORE}"


@pytest.mark.parametrize("S", [Secret, AsyncSecret])
def test_listing_returns_a_paginator_whose_pages_raise(S):
    pager = S.list(limit=10, next_token="t")
    assert isinstance(pager, (SecretPaginator, AsyncSecretPaginator))
    assert (pager.limit, pager.next_token, pager.has_next) == (10, "t", True)
    assert pager.has_next  # nothing was fetched, nothing pretends to have been


def test_sync_paginator_next_items_raises():
    pager = Secret.list()
    with pytest.raises(
        SecretException, match="Secret.list: cua-e2b has no account-level secret store"
    ):
        pager.next_items()
    # The failed call fetched nothing, so the paginator is not exhausted.
    assert pager.has_next


async def test_async_paginator_next_items_raises():
    pager = AsyncSecret.list()
    with pytest.raises(SecretException, match="Secret.list:"):
        await pager.next_items()


async def test_paginator_exhausted_state_raises_like_e2b():
    pager = AsyncSecret.list()
    pager._has_next = False
    with pytest.raises(Exception, match="No more items to fetch"):
        await pager.next_items()


@pytest.mark.parametrize(
    "call,text",
    [
        (
            lambda: Secret.create("", "v"),
            "secret name '' is not usable: a secret name must be a non-empty string",
        ),
        (lambda: Secret.create("a{b", "v"), "secret name 'a{b' is not usable"),
        (lambda: Secret.create("n", 5), "secret value must be a string, got int."),
        (
            lambda: Secret.create("n", "v", {"k": 1}),
            "secret metadata must be a dict of string keys and values.",
        ),
        (
            lambda: Secret.update("", "v"),
            "secret '' is not usable: expected the ID or name of a secret.",
        ),
        (lambda: Secret.update("n", None), "secret value must be a string, got NoneType."),
        (lambda: Secret.get_info(None), "secret None is not usable"),
        (lambda: Secret.exists(""), "secret '' is not usable"),
        (lambda: Secret.destroy(3), "secret 3 is not usable"),
    ],
)
def test_arguments_are_validated_before_the_native_error(call, text):
    with pytest.raises(InvalidArgumentException) as info:
        call()
    assert text in str(info.value)


def test_invalid_connection_options_fail_first_like_e2b():
    with pytest.raises(InvalidArgumentException, match="Invalid retries=-1"):
        Secret.create("n", "v", retries=-1)
    with pytest.raises(InvalidArgumentException, match="http_version must be"):
        Secret.get_info("n", http_version="3")
    pager = Secret.list()
    with pytest.raises(InvalidArgumentException, match="Invalid retries=-1"):
        pager.next_items(retries=-1)
    with pytest.raises(InvalidArgumentException, match="Invalid retries=-3"):
        Secret.list(retries=-3).next_items()


def test_bound_client_params_apply_to_secrets():
    client = e2b.E2B(retries=-5)
    with pytest.raises(InvalidArgumentException, match="Invalid retries=-5"):
        client.Secret.create("n", "v")
    # A per-call value wins over the bound one, reaching the native error.
    with pytest.raises(SecretException):
        client.Secret.create("n", "v", retries=1)


@pytest.mark.parametrize("S", [Secret, AsyncSecret])
def test_fill_builds_the_placeholder(S):
    assert S.fill("openai-api-key") == "${e2b.secrets.openai-api-key}"
    for bad in ("", "a}b", "a{b", "a\nb", None):
        with pytest.raises(InvalidArgumentException) as info:
            S.fill(bad)
        assert str(info.value).startswith(
            f"secret name {bad!r} is not usable: a secret name must be a non-empty string"
        )


@pytest.mark.parametrize("S", [Secret, AsyncSecret])
def test_iam_token_builds_the_definition(S):
    assert S.iam_token(audience="sts.amazonaws.com", token_type="JWT-SVID") == {
        "audience": "sts.amazonaws.com",
        "token_type": "JWT-SVID",
    }
    with pytest.raises(TypeError):
        S.iam_token("sts.amazonaws.com", "JWT-SVID")  # keyword-only, as in e2b


def test_secret_info_fields():
    from datetime import datetime, timezone

    now = datetime.now(timezone.utc)
    info = SecretInfo("id", "name", 2, {"a": "b"}, now, now)
    assert (info.secret_id, info.name, info.version, info.metadata) == ("id", "name", 2, {"a": "b"})
    assert not hasattr(info, "value")  # values are write-only


def test_exception_hierarchy():
    assert issubclass(SecretNotFoundException, SecretException)
    assert SecretException.__bases__ == (Exception,)
