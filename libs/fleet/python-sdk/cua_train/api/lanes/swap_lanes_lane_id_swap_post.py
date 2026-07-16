from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.lane_swap_response import LaneSwapResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    lane_id: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/gateway/{pool}/lanes/{lane_id}/swap".format(
            pool=quote(str(pool), safe=""),
            lane_id=quote(str(lane_id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> LaneSwapResponse | None:
    if response.status_code == 200:
        response_200 = LaneSwapResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[LaneSwapResponse]:
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
) -> Response[LaneSwapResponse]:
    """Swap

     New-first claim rotation for ``lane_id``.

    Calls into the pure protocol body in osgym/lane_swap.py with a
    K8s-backed SwapDeps. The lane keeps its bound VM if the swap
    bind times out (SwapAtomicity); the new vm_id is returned on
    success.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[LaneSwapResponse]
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
) -> LaneSwapResponse | None:
    """Swap

     New-first claim rotation for ``lane_id``.

    Calls into the pure protocol body in osgym/lane_swap.py with a
    K8s-backed SwapDeps. The lane keeps its bound VM if the swap
    bind times out (SwapAtomicity); the new vm_id is returned on
    success.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        LaneSwapResponse
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
) -> Response[LaneSwapResponse]:
    """Swap

     New-first claim rotation for ``lane_id``.

    Calls into the pure protocol body in osgym/lane_swap.py with a
    K8s-backed SwapDeps. The lane keeps its bound VM if the swap
    bind times out (SwapAtomicity); the new vm_id is returned on
    success.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[LaneSwapResponse]
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
) -> LaneSwapResponse | None:
    """Swap

     New-first claim rotation for ``lane_id``.

    Calls into the pure protocol body in osgym/lane_swap.py with a
    K8s-backed SwapDeps. The lane keeps its bound VM if the swap
    bind times out (SwapAtomicity); the new vm_id is returned on
    success.

    Args:
        pool (str):
        lane_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        LaneSwapResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            lane_id=lane_id,
            client=client,
        )
    ).parsed
