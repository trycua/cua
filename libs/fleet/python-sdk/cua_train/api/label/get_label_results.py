from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.label_in_progress_response import LabelInProgressResponse
from ...models.label_results_response import LabelResultsResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    pool: str,
    label: str,
    *,
    completed_only: bool | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["completed_only"] = completed_only

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/label/{pool}/{label}/results".format(
            pool=quote(str(pool), safe=""),
            label=quote(str(label), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ErrorResponse | LabelInProgressResponse | LabelResultsResponse | None:
    if response.status_code == 200:
        response_200 = LabelResultsResponse.from_dict(response.json())

        return response_200

    if response.status_code == 202:
        response_202 = LabelInProgressResponse.from_dict(response.json())

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
) -> Response[ErrorResponse | LabelInProgressResponse | LabelResultsResponse]:
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
    completed_only: bool | Unset = UNSET,
) -> Response[ErrorResponse | LabelInProgressResponse | LabelResultsResponse]:
    """Aggregate results for all batches under a pool+label

     Default mode returns 200 only once every matched batch is completed.
    Pass `?completed_only=true` to drain partial results (200 with the
    filtered subset; non-completed batches are visible via status).
    Aggregates only batches submitted under `{pool}`.

    Args:
        pool (str):
        label (str):
        completed_only (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ErrorResponse | LabelInProgressResponse | LabelResultsResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        label=label,
        completed_only=completed_only,
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
    completed_only: bool | Unset = UNSET,
) -> ErrorResponse | LabelInProgressResponse | LabelResultsResponse | None:
    """Aggregate results for all batches under a pool+label

     Default mode returns 200 only once every matched batch is completed.
    Pass `?completed_only=true` to drain partial results (200 with the
    filtered subset; non-completed batches are visible via status).
    Aggregates only batches submitted under `{pool}`.

    Args:
        pool (str):
        label (str):
        completed_only (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ErrorResponse | LabelInProgressResponse | LabelResultsResponse
    """

    return sync_detailed(
        pool=pool,
        label=label,
        client=client,
        completed_only=completed_only,
    ).parsed


async def asyncio_detailed(
    pool: str,
    label: str,
    *,
    client: AuthenticatedClient | Client,
    completed_only: bool | Unset = UNSET,
) -> Response[ErrorResponse | LabelInProgressResponse | LabelResultsResponse]:
    """Aggregate results for all batches under a pool+label

     Default mode returns 200 only once every matched batch is completed.
    Pass `?completed_only=true` to drain partial results (200 with the
    filtered subset; non-completed batches are visible via status).
    Aggregates only batches submitted under `{pool}`.

    Args:
        pool (str):
        label (str):
        completed_only (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ErrorResponse | LabelInProgressResponse | LabelResultsResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        label=label,
        completed_only=completed_only,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pool: str,
    label: str,
    *,
    client: AuthenticatedClient | Client,
    completed_only: bool | Unset = UNSET,
) -> ErrorResponse | LabelInProgressResponse | LabelResultsResponse | None:
    """Aggregate results for all batches under a pool+label

     Default mode returns 200 only once every matched batch is completed.
    Pass `?completed_only=true` to drain partial results (200 with the
    filtered subset; non-completed batches are visible via status).
    Aggregates only batches submitted under `{pool}`.

    Args:
        pool (str):
        label (str):
        completed_only (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ErrorResponse | LabelInProgressResponse | LabelResultsResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            label=label,
            client=client,
            completed_only=completed_only,
        )
    ).parsed
