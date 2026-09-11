"""Explicit, non-reconnecting MCP carrier for the canonical Driver envelopes."""

from __future__ import annotations

import json
import re
import uuid
from typing import Any

import httpx

_PROTOCOL = "2025-06-18"
_CAPABILITY = "ai.cua.driver.envelopes"
_PREFIX = "cua/driver/v1/"
_REQUEST_LIMIT = 1024 * 1024
_RESPONSE_LIMIT = 16 * 1024 * 1024
_SESSION = re.compile(r"[\x21-\x7e]{1,256}\Z")


class McpCarrierError(RuntimeError):
    """A bounded reason, never a remote body or transport exception."""


def _json(text: str) -> Any:
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate JSON member")
            result[key] = value
        return result

    def constant(_):
        raise ValueError("non-finite JSON value")

    return json.loads(text, object_pairs_hook=pairs, parse_constant=constant)


def _response(response: httpx.Response, request_id: str) -> Any:
    """Decode one bounded JSON or buffered SSE response with exact correlation."""
    try:
        if len(response.content) > _RESPONSE_LIMIT:
            raise ValueError
        text = response.content.decode("utf-8", errors="strict")
        media = response.headers.get("content-type", "").split(";", 1)[0].strip().lower()
        if media == "application/json":
            messages = [_json(text)]
        elif media == "text/event-stream":
            messages = []
            blocks = text.replace("\r\n", "\n").replace("\r", "\n").split("\n\n")
            if blocks[-1].strip() or len(blocks) > 128:
                raise ValueError
            for block in blocks[:-1]:
                data = []
                for line in block.split("\n"):
                    if not line or line.startswith(":"):
                        continue
                    field, _, value = line.partition(":")
                    value = value.removeprefix(" ")
                    if field == "data":
                        data.append(value)
                    elif field == "event" and value not in ("message", ""):
                        raise ValueError
                    elif field not in ("event", "id", "retry"):
                        raise ValueError
                if data:
                    messages.append(_json("\n".join(data)))
        else:
            raise ValueError
        # This carrier has no unsolicited event stream and never follows SSE
        # reconnect instructions. More than one RPC response is ambiguous.
        if len(messages) != 1:
            raise ValueError
        message = messages[0]
        if (
            not isinstance(message, dict)
            or message.get("jsonrpc") != "2.0"
            or type(message.get("id")) is not str
            or message["id"] != request_id
            or ("result" in message) == ("error" in message)
        ):
            raise ValueError
        if "error" in message:
            error = message["error"]
            if not isinstance(error, dict) or type(error.get("code")) is not int:
                raise ValueError
            code = error["code"]
            if code in (-32404, -32409):
                raise McpCarrierError("MCP Driver connection is stale or unavailable")
            if code == -32601:
                raise McpCarrierError("MCP endpoint does not support typed Driver envelopes")
            raise McpCarrierError("MCP Driver request failed; completion is unknown")
        return message["result"]
    except (ValueError, KeyError, TypeError, UnicodeError, RecursionError):
        raise McpCarrierError("MCP Driver response is malformed; completion is unknown") from None


class McpCarrier:
    """One stateful MCP session owned by one typed channel, with no retry."""

    def __init__(self, transport: Any, service: str):
        self.transport = transport
        self.service = service
        self.session: str | None = None
        self.ready = False
        self.broken = False
        self.session_closed = False

    async def _http(self, method: str, body=None, *, timeout=None, initializing=False):
        headers = {"Accept": "application/json, text/event-stream"}
        if self.session is not None:
            headers["Mcp-Session-Id"] = self.session
            headers["MCP-Protocol-Version"] = _PROTOCOL
        try:
            if (
                body is not None
                and len(json.dumps(body, allow_nan=False).encode()) > _REQUEST_LIMIT
            ):
                raise McpCarrierError("MCP Driver request exceeds the size limit")
            response = await self.transport.request_service(
                self.service,
                method=method,
                path="/mcp",
                json_body=body,
                headers=headers,
                **({"timeout": timeout} if timeout is not None else {}),
            )
        except McpCarrierError:
            raise
        except Exception as error:
            self.broken = True
            if getattr(error, "status", None) in (401, 403):
                raise McpCarrierError("Driver service authorization denied") from None
            raise McpCarrierError("MCP Driver transport failed; completion is unknown") from None
        if response.status_code in (401, 403):
            self.broken = True
            raise McpCarrierError("Driver service authorization denied")
        if not 200 <= response.status_code < 300:
            self.broken = True
            if response.status_code in (404, 409):
                raise McpCarrierError("MCP Driver session is stale or unavailable")
            raise McpCarrierError("MCP Driver request failed; completion is unknown")
        sessions = response.headers.get_list("mcp-session-id")
        if initializing:
            if len(sessions) != 1 or not _SESSION.fullmatch(sessions[0]):
                self.broken = True
                raise McpCarrierError("MCP Driver session negotiation is malformed")
            # Retain an allocated session before parsing the body, so malformed
            # negotiation can still perform bounded HTTP-session cleanup.
            self.session = sessions[0]
        elif sessions and sessions != [self.session]:
            self.broken = True
            raise McpCarrierError("MCP Driver session changed unexpectedly")
        if len(response.content) > _RESPONSE_LIMIT:
            self.broken = True
            raise McpCarrierError("MCP Driver response exceeds the size limit")
        return response

    async def _rpc(self, method: str, params: Any, *, timeout=None, initializing=False):
        if self.broken or self.session_closed:
            raise McpCarrierError("MCP Driver carrier is closed")
        request_id = uuid.uuid4().hex
        body = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        response = await self._http("POST", body, timeout=timeout, initializing=initializing)
        try:
            return _response(response, request_id)
        except McpCarrierError:
            self.broken = True
            raise

    async def initialize(self):
        if self.session is not None or self.ready:
            raise McpCarrierError("MCP Driver carrier cannot reconnect")
        result = await self._rpc(
            "initialize",
            {
                "protocolVersion": _PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": "cua-sandbox-driver", "version": "1"},
            },
            initializing=True,
        )
        try:
            extension = result["capabilities"]["experimental"][_CAPABILITY]
            if (
                result["protocolVersion"] != _PROTOCOL
                or type(extension["version"]) is not int
                or extension["version"] != 1
            ):
                raise ValueError
        except (ValueError, KeyError, TypeError):
            self.broken = True
            raise McpCarrierError(
                "MCP endpoint does not support typed Driver envelopes v1"
            ) from None
        response = await self._http(
            "POST", {"jsonrpc": "2.0", "method": "notifications/initialized"}
        )
        if response.status_code not in (202, 204) or response.content:
            self.broken = True
            raise McpCarrierError("MCP Driver initialization acknowledgement is incompatible")
        self.ready = True

    async def request(self, method, suffix, body, connection_id, generation, *, timeout=None):
        if not self.ready:
            raise McpCarrierError("MCP Driver carrier is not initialized")
        if connection_id is None:
            operation, params = "open", {}
        else:
            params = {"connection_id": connection_id, "generation": generation}
            if method == "DELETE":
                operation = "close"
            elif suffix == "/exchange":
                operation = "exchange"
                params["envelope"] = body
            elif suffix == "/cancel":
                operation = "cancel"
                params["request_id"] = body["request_id"]
            else:
                raise McpCarrierError("Invalid MCP Driver operation")
        result = await self._rpc(_PREFIX + operation, params, timeout=timeout)
        if operation in ("close", "cancel") and (
            not isinstance(result, dict) or set(result) != {"ok"} or result["ok"] is not True
        ):
            self.broken = True
            raise McpCarrierError("MCP Driver cleanup acknowledgement is malformed")
        return httpx.Response(200, json=result)

    async def close_session(self):
        if self.session is None or self.session_closed:
            return
        # DELETE is teardown, not action replay. It remains available when a
        # failed exchange has made the carrier unusable for further requests.
        await self._http("DELETE")
        self.session_closed = True
        self.ready = False
