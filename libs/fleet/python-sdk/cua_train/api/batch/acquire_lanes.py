from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.acquire_lanes_request import AcquireLanesRequest
from ...models.acquire_lanes_response import AcquireLanesResponse
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    *,
    body: AcquireLanesRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
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
) -> AcquireLanesResponse | ErrorResponse | None:
    if response.status_code == 200:
        response_200 = AcquireLanesResponse.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = ErrorResponse.from_dict(response.json())

        return response_401

    if response.status_code == 503:
        response_503 = ErrorResponse.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[AcquireLanesResponse | ErrorResponse]:
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
    body: AcquireLanesRequest,
) -> Response[AcquireLanesResponse | ErrorResponse]:
    r"""Acquire warm VM lanes

     Explicitly provisions N warm VMs (\"lanes\") and returns their vm_ids. Pass each as
    `RunConfig.vm_id` to run actions on it (state persists across batches), and `DELETE` this route to
    release them. All-or-nothing: a partial acquisition is rolled back, and 503 is returned if the pool
    can't supply every lane in time.

    Args:
        pool (str):
        body (AcquireLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AcquireLanesResponse | ErrorResponse]
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
    body: AcquireLanesRequest,
) -> AcquireLanesResponse | ErrorResponse | None:
    r"""Acquire warm VM lanes

     Explicitly provisions N warm VMs (\"lanes\") and returns their vm_ids. Pass each as
    `RunConfig.vm_id` to run actions on it (state persists across batches), and `DELETE` this route to
    release them. All-or-nothing: a partial acquisition is rolled back, and 503 is returned if the pool
    can't supply every lane in time.

    Args:
        pool (str):
        body (AcquireLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AcquireLanesResponse | ErrorResponse
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
    body: AcquireLanesRequest,
) -> Response[AcquireLanesResponse | ErrorResponse]:
    r"""Acquire warm VM lanes

     Explicitly provisions N warm VMs (\"lanes\") and returns their vm_ids. Pass each as
    `RunConfig.vm_id` to run actions on it (state persists across batches), and `DELETE` this route to
    release them. All-or-nothing: a partial acquisition is rolled back, and 503 is returned if the pool
    can't supply every lane in time.

    Args:
        pool (str):
        body (AcquireLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AcquireLanesResponse | ErrorResponse]
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
    body: AcquireLanesRequest,
) -> AcquireLanesResponse | ErrorResponse | None:
    r"""Acquire warm VM lanes

     Explicitly provisions N warm VMs (\"lanes\") and returns their vm_ids. Pass each as
    `RunConfig.vm_id` to run actions on it (state persists across batches), and `DELETE` this route to
    release them. All-or-nothing: a partial acquisition is rolled back, and 503 is returned if the pool
    can't supply every lane in time.

    Args:
        pool (str):
        body (AcquireLanesRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AcquireLanesResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            client=client,
            body=body,
        )
    ).parsed
