from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.lane_create_request import LaneCreateRequest
from ...models.lane_response import LaneResponse
from ...types import Response


def _get_kwargs(
    pool: str,
    *,
    body: LaneCreateRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/gateway/{pool}/lanes".format(
            pool=quote(str(pool), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> LaneResponse | None:
    if response.status_code == 200:
        response_200 = LaneResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[LaneResponse]:
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
    body: LaneCreateRequest,
) -> Response[LaneResponse]:
    """Create Lane

     Create a lane: a fresh OSGymSandboxClaim bound to a Sandbox.

    The lane id is the stable handle the customer holds. The
    underlying claim CR's name rotates on swap; the lane id does
    not. The initial bind is via the normal queue+CAS-PATCH flow,
    same as /reset.

    Args:
        pool (str):
        body (LaneCreateRequest): Body of POST /lanes.

            ``ttl_seconds`` is the absolute time the claim is reserved for;
            after that the reaper deletes it. With ``autoRenew=true`` the lane
            auto-renew timer in pool-operator pushes this forward, so the
            customer's effective TTL is "as long as the lane exists".

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[LaneResponse]
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
    body: LaneCreateRequest,
) -> LaneResponse | None:
    """Create Lane

     Create a lane: a fresh OSGymSandboxClaim bound to a Sandbox.

    The lane id is the stable handle the customer holds. The
    underlying claim CR's name rotates on swap; the lane id does
    not. The initial bind is via the normal queue+CAS-PATCH flow,
    same as /reset.

    Args:
        pool (str):
        body (LaneCreateRequest): Body of POST /lanes.

            ``ttl_seconds`` is the absolute time the claim is reserved for;
            after that the reaper deletes it. With ``autoRenew=true`` the lane
            auto-renew timer in pool-operator pushes this forward, so the
            customer's effective TTL is "as long as the lane exists".

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        LaneResponse
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
    body: LaneCreateRequest,
) -> Response[LaneResponse]:
    """Create Lane

     Create a lane: a fresh OSGymSandboxClaim bound to a Sandbox.

    The lane id is the stable handle the customer holds. The
    underlying claim CR's name rotates on swap; the lane id does
    not. The initial bind is via the normal queue+CAS-PATCH flow,
    same as /reset.

    Args:
        pool (str):
        body (LaneCreateRequest): Body of POST /lanes.

            ``ttl_seconds`` is the absolute time the claim is reserved for;
            after that the reaper deletes it. With ``autoRenew=true`` the lane
            auto-renew timer in pool-operator pushes this forward, so the
            customer's effective TTL is "as long as the lane exists".

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[LaneResponse]
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
    body: LaneCreateRequest,
) -> LaneResponse | None:
    """Create Lane

     Create a lane: a fresh OSGymSandboxClaim bound to a Sandbox.

    The lane id is the stable handle the customer holds. The
    underlying claim CR's name rotates on swap; the lane id does
    not. The initial bind is via the normal queue+CAS-PATCH flow,
    same as /reset.

    Args:
        pool (str):
        body (LaneCreateRequest): Body of POST /lanes.

            ``ttl_seconds`` is the absolute time the claim is reserved for;
            after that the reaper deletes it. With ``autoRenew=true`` the lane
            auto-renew timer in pool-operator pushes this forward, so the
            customer's effective TTL is "as long as the lane exists".

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        LaneResponse
    """

    return (
        await asyncio_detailed(
            pool=pool,
            client=client,
            body=body,
        )
    ).parsed
