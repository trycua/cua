from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.cancel_label_response import CancelLabelResponse
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    label: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/label/{pool}/{label}".format(
            pool=quote(str(pool), safe=""),
            label=quote(str(label), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> CancelLabelResponse | ErrorResponse | None:
    if response.status_code == 200:
        response_200 = CancelLabelResponse.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = ErrorResponse.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[CancelLabelResponse | ErrorResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    pool: str,
    label: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[CancelLabelResponse | ErrorResponse]:
    """Cancel every non-terminal batch under a label (scoped to pool)

     Cancels only batches submitted under `{pool}`; same label in other pools is untouched.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[CancelLabelResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        label=label,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    pool: str,
    label: str,
    *,
    client: AuthenticatedClient | Client,
) -> CancelLabelResponse | ErrorResponse | None:
    """Cancel every non-terminal batch under a label (scoped to pool)

     Cancels only batches submitted under `{pool}`; same label in other pools is untouched.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        CancelLabelResponse | ErrorResponse
    """

    return sync_detailed(
        pool=pool,
        label=label,
        client=client,
    ).parsed


async def asyncio_detailed(
    pool: str,
    label: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[CancelLabelResponse | ErrorResponse]:
    """Cancel every non-terminal batch under a label (scoped to pool)

     Cancels only batches submitted under `{pool}`; same label in other pools is untouched.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[CancelLabelResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        label=label,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pool: str,
    label: str,
    *,
    client: AuthenticatedClient | Client,
) -> CancelLabelResponse | ErrorResponse | None:
    """Cancel every non-terminal batch under a label (scoped to pool)

     Cancels only batches submitted under `{pool}`; same label in other pools is untouched.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        CancelLabelResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            label=label,
            client=client,
        )
    ).parsed
