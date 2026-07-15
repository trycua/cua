from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.release_lanes_request import ReleaseLanesRequest
from ...models.release_lanes_response import ReleaseLanesResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    *,
    body: ReleaseLanesRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/batch/{pool}/lanes".format(
            pool=quote(str(pool), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ErrorResponse | ReleaseLanesResponse | None:
    if response.status_code == 200:
        response_200 = ReleaseLanesResponse.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = ErrorResponse.from_dict(response.json())

        return response_401

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ErrorResponse | ReleaseLanesResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    pool: str,
    *,
    client: AuthenticatedClient | Client,
    body: ReleaseLanesRequest,
) -> Response[ErrorResponse | ReleaseLanesResponse]:
    """Release warm VM lanes

     Shuts down the given VMs (lanes from `acquireLanes` or `vm_id`s returned by a `keep_warm` batch).
    Best-effort and idempotent — unknown or already-gone ids are ignored.

    Args:
        pool (str):
        body (ReleaseLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ErrorResponse | ReleaseLanesResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    pool: str,
    *,
    client: AuthenticatedClient | Client,
    body: ReleaseLanesRequest,
) -> ErrorResponse | ReleaseLanesResponse | None:
    """Release warm VM lanes

     Shuts down the given VMs (lanes from `acquireLanes` or `vm_id`s returned by a `keep_warm` batch).
    Best-effort and idempotent — unknown or already-gone ids are ignored.

    Args:
        pool (str):
        body (ReleaseLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ErrorResponse | ReleaseLanesResponse
    """

    return sync_detailed(
        pool=pool,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    pool: str,
    *,
    client: AuthenticatedClient | Client,
    body: ReleaseLanesRequest,
) -> Response[ErrorResponse | ReleaseLanesResponse]:
    """Release warm VM lanes

     Shuts down the given VMs (lanes from `acquireLanes` or `vm_id`s returned by a `keep_warm` batch).
    Best-effort and idempotent — unknown or already-gone ids are ignored.

    Args:
        pool (str):
        body (ReleaseLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ErrorResponse | ReleaseLanesResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pool: str,
    *,
    client: AuthenticatedClient | Client,
    body: ReleaseLanesRequest,
) -> ErrorResponse | ReleaseLanesResponse | None:
    """Release warm VM lanes

     Shuts down the given VMs (lanes from `acquireLanes` or `vm_id`s returned by a `keep_warm` batch).
    Best-effort and idempotent — unknown or already-gone ids are ignored.

    Args:
        pool (str):
        body (ReleaseLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ErrorResponse | ReleaseLanesResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            client=client,
            body=body,
        )
    ).parsed
