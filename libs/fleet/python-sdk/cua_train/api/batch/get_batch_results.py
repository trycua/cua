from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.batch_in_progress_response import BatchInProgressResponse
from ...models.batch_results_response import BatchResultsResponse
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/batch/{pool}/{id}/results".format(
            pool=quote(str(pool), safe=""),
            id=quote(str(id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> BatchInProgressResponse | BatchResultsResponse | ErrorResponse | None:
    if response.status_code == 200:
        response_200 = BatchResultsResponse.from_dict(response.json())

        return response_200

    if response.status_code == 202:
        response_202 = BatchInProgressResponse.from_dict(response.json())

        return response_202

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
) -> Response[BatchInProgressResponse | BatchResultsResponse | ErrorResponse]:
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
) -> Response[BatchInProgressResponse | BatchResultsResponse | ErrorResponse]:
    """Final batch results

     Returns 200 with the full results once the batch is in a terminal
    status (completed/failed). While still running, returns 202 with a
    `message` field.

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[BatchInProgressResponse | BatchResultsResponse | ErrorResponse]
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
) -> BatchInProgressResponse | BatchResultsResponse | ErrorResponse | None:
    """Final batch results

     Returns 200 with the full results once the batch is in a terminal
    status (completed/failed). While still running, returns 202 with a
    `message` field.

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        BatchInProgressResponse | BatchResultsResponse | ErrorResponse
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
) -> Response[BatchInProgressResponse | BatchResultsResponse | ErrorResponse]:
    """Final batch results

     Returns 200 with the full results once the batch is in a terminal
    status (completed/failed). While still running, returns 202 with a
    `message` field.

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[BatchInProgressResponse | BatchResultsResponse | ErrorResponse]
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
) -> BatchInProgressResponse | BatchResultsResponse | ErrorResponse | None:
    """Final batch results

     Returns 200 with the full results once the batch is in a terminal
    status (completed/failed). While still running, returns 202 with a
    `message` field.

    Args:
        pool (str):
        id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        BatchInProgressResponse | BatchResultsResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            id=id,
            client=client,
        )
    ).parsed
