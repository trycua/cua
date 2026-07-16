from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.batch_submit_request import BatchSubmitRequest
from ...models.batch_submit_response import BatchSubmitResponse
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    *,
    body: BatchSubmitRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/batch/{pool}/submit".format(
            pool=quote(str(pool), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> BatchSubmitResponse | ErrorResponse | None:
    if response.status_code == 202:
        response_202 = BatchSubmitResponse.from_dict(response.json())

        return response_202

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
) -> Response[BatchSubmitResponse | ErrorResponse]:
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
    body: BatchSubmitRequest,
) -> Response[BatchSubmitResponse | ErrorResponse]:
    """Submit a batch of rollouts

     Asynchronously queues a batch of OSGym rollouts under `{pool}`.
    Returns immediately with a `batch_id`; poll
    `/api/batch/{pool}/{id}/status` for progress and
    `/api/batch/{pool}/{id}/results` for the final payload.

    The pool is bound by the URL path; any `pool_name` in the body must
    match the URL pool or the request is rejected with 400.

    Args:
        pool (str):
        body (BatchSubmitRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[BatchSubmitResponse | ErrorResponse]
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
    body: BatchSubmitRequest,
) -> BatchSubmitResponse | ErrorResponse | None:
    """Submit a batch of rollouts

     Asynchronously queues a batch of OSGym rollouts under `{pool}`.
    Returns immediately with a `batch_id`; poll
    `/api/batch/{pool}/{id}/status` for progress and
    `/api/batch/{pool}/{id}/results` for the final payload.

    The pool is bound by the URL path; any `pool_name` in the body must
    match the URL pool or the request is rejected with 400.

    Args:
        pool (str):
        body (BatchSubmitRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        BatchSubmitResponse | ErrorResponse
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
    body: BatchSubmitRequest,
) -> Response[BatchSubmitResponse | ErrorResponse]:
    """Submit a batch of rollouts

     Asynchronously queues a batch of OSGym rollouts under `{pool}`.
    Returns immediately with a `batch_id`; poll
    `/api/batch/{pool}/{id}/status` for progress and
    `/api/batch/{pool}/{id}/results` for the final payload.

    The pool is bound by the URL path; any `pool_name` in the body must
    match the URL pool or the request is rejected with 400.

    Args:
        pool (str):
        body (BatchSubmitRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[BatchSubmitResponse | ErrorResponse]
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
    body: BatchSubmitRequest,
) -> BatchSubmitResponse | ErrorResponse | None:
    """Submit a batch of rollouts

     Asynchronously queues a batch of OSGym rollouts under `{pool}`.
    Returns immediately with a `batch_id`; poll
    `/api/batch/{pool}/{id}/status` for progress and
    `/api/batch/{pool}/{id}/results` for the final payload.

    The pool is bound by the URL path; any `pool_name` in the body must
    match the URL pool or the request is rejected with 400.

    Args:
        pool (str):
        body (BatchSubmitRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        BatchSubmitResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            client=client,
            body=body,
        )
    ).parsed
