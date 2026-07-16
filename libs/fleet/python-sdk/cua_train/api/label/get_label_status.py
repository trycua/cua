from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.label_status_response import LabelStatusResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    label: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/label/{pool}/{label}/status".format(
            pool=quote(str(pool), safe=""),
            label=quote(str(label), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ErrorResponse | LabelStatusResponse | None:
    if response.status_code == 200:
        response_200 = LabelStatusResponse.from_dict(response.json())

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
) -> Response[ErrorResponse | LabelStatusResponse]:
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
) -> Response[ErrorResponse | LabelStatusResponse]:
    """Aggregate status for all batches under a pool+label

     Aggregates only batches submitted under `{pool}`; batches with the same label in other pools are
    invisible.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ErrorResponse | LabelStatusResponse]
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
) -> ErrorResponse | LabelStatusResponse | None:
    """Aggregate status for all batches under a pool+label

     Aggregates only batches submitted under `{pool}`; batches with the same label in other pools are
    invisible.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ErrorResponse | LabelStatusResponse
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
) -> Response[ErrorResponse | LabelStatusResponse]:
    """Aggregate status for all batches under a pool+label

     Aggregates only batches submitted under `{pool}`; batches with the same label in other pools are
    invisible.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ErrorResponse | LabelStatusResponse]
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
) -> ErrorResponse | LabelStatusResponse | None:
    """Aggregate status for all batches under a pool+label

     Aggregates only batches submitted under `{pool}`; batches with the same label in other pools are
    invisible.

    Args:
        pool (str):
        label (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ErrorResponse | LabelStatusResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            label=label,
            client=client,
        )
    ).parsed
