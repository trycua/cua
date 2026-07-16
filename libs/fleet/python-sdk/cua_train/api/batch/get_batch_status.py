from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.batch_status_response import BatchStatusResponse
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/batch/{pool}/{id}/status".format(
            pool=quote(str(pool), safe=""),
            id=quote(str(id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> BatchStatusResponse | ErrorResponse | None:
    if response.status_code == 200:
        response_200 = BatchStatusResponse.from_dict(response.json())

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
) -> Response[BatchStatusResponse | ErrorResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    pool: str,
    id: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[BatchStatusResponse | ErrorResponse]:
    """Poll batch progress

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[BatchStatusResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        id=id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    pool: str,
    id: str,
    *,
    client: AuthenticatedClient | Client,
) -> BatchStatusResponse | ErrorResponse | None:
    """Poll batch progress

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        BatchStatusResponse | ErrorResponse
    """

    return sync_detailed(
        pool=pool,
        id=id,
        client=client,
    ).parsed


async def asyncio_detailed(
    pool: str,
    id: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[BatchStatusResponse | ErrorResponse]:
    """Poll batch progress

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[BatchStatusResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        id=id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pool: str,
    id: str,
    *,
    client: AuthenticatedClient | Client,
) -> BatchStatusResponse | ErrorResponse | None:
    """Poll batch progress

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        BatchStatusResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            id=id,
            client=client,
        )
    ).parsed
