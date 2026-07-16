from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.lane_delete_response import LaneDeleteResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    lane_id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/gateway/{pool}/lanes/{lane_id}".format(
            pool=quote(str(pool), safe=""),
            lane_id=quote(str(lane_id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> LaneDeleteResponse | None:
    if response.status_code == 200:
        response_200 = LaneDeleteResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[LaneDeleteResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    pool: str,
    lane_id: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[LaneDeleteResponse]:
    """Delete Lane

     Release the lane's bound claim. Idempotent.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[LaneDeleteResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        lane_id=lane_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    pool: str,
    lane_id: str,
    *,
    client: AuthenticatedClient | Client,
) -> LaneDeleteResponse | None:
    """Delete Lane

     Release the lane's bound claim. Idempotent.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        LaneDeleteResponse
    """

    return sync_detailed(
        pool=pool,
        lane_id=lane_id,
        client=client,
    ).parsed


async def asyncio_detailed(
    pool: str,
    lane_id: str,
    *,
    client: AuthenticatedClient | Client,
) -> Response[LaneDeleteResponse]:
    """Delete Lane

     Release the lane's bound claim. Idempotent.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[LaneDeleteResponse]
    """

    kwargs = _get_kwargs(
        pool=pool,
        lane_id=lane_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pool: str,
    lane_id: str,
    *,
    client: AuthenticatedClient | Client,
) -> LaneDeleteResponse | None:
    """Delete Lane

     Release the lane's bound claim. Idempotent.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        LaneDeleteResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            lane_id=lane_id,
            client=client,
        )
    ).parsed
