from __future__ import annotations

import base64
from typing import Any

import pytest

from cua_sandbox.interfaces.files import Files, _TRANSFER_CHUNK_BYTES


class RecordingTransport:
    def __init__(self, content: bytes = b"") -> None:
        self.content = content
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def send(self, action: str, **params: Any) -> dict[str, Any]:
        self.calls.append((action, params))
        if action == "get_file_size":
            return {"success": True, "size": len(self.content)}
        if action == "write_bytes":
            chunk = base64.b64decode(params["content_b64"])
            self.content = self.content + chunk if params.get("append") else chunk
            return {"success": True}
        if action == "read_bytes":
            offset = params["offset"]
            length = params["length"]
            chunk = self.content[offset : offset + length]
            return {"success": True, "content_b64": base64.b64encode(chunk).decode("ascii")}
        raise AssertionError(f"unexpected action: {action}")


@pytest.mark.asyncio
async def test_write_bytes_chunks_large_payload_below_gateway_limit() -> None:
    transport = RecordingTransport(b"old content")
    files = Files(transport)  # type: ignore[arg-type]
    payload = b"a" * (_TRANSFER_CHUNK_BYTES * 2 + 17)

    await files.write_bytes("/tmp/payload", payload)

    assert transport.content == payload
    write_calls = [params for action, params in transport.calls if action == "write_bytes"]
    assert [params.get("append") for params in write_calls] == [False, True, True]
    assert [len(base64.b64decode(params["content_b64"])) for params in write_calls] == [
        _TRANSFER_CHUNK_BYTES,
        _TRANSFER_CHUNK_BYTES,
        17,
    ]


@pytest.mark.asyncio
async def test_write_bytes_keeps_small_and_empty_payloads_single_request() -> None:
    for payload in (b"", b"small"):
        transport = RecordingTransport(b"old content")
        files = Files(transport)  # type: ignore[arg-type]

        await files.write_bytes("/tmp/payload", payload)

        assert transport.content == payload
        assert len(transport.calls) == 1
        assert "append" not in transport.calls[0][1]


@pytest.mark.asyncio
async def test_read_bytes_chunks_large_file_using_size_and_offsets() -> None:
    payload = bytes(range(251)) * ((_TRANSFER_CHUNK_BYTES * 2) // 251 + 2)
    transport = RecordingTransport(payload)
    files = Files(transport)  # type: ignore[arg-type]

    result = await files.read_bytes("/tmp/payload")

    assert result == payload
    assert transport.calls[0] == ("get_file_size", {"path": "/tmp/payload"})
    read_calls = [params for action, params in transport.calls if action == "read_bytes"]
    assert [params["offset"] for params in read_calls] == [
        0,
        _TRANSFER_CHUNK_BYTES,
        _TRANSFER_CHUNK_BYTES * 2,
    ]
    assert all(params["length"] <= _TRANSFER_CHUNK_BYTES for params in read_calls)


@pytest.mark.asyncio
async def test_read_bytes_chunks_explicit_offset_and_length() -> None:
    payload = bytes(range(256)) * 5000
    transport = RecordingTransport(payload)
    files = Files(transport)  # type: ignore[arg-type]
    offset = 123
    length = _TRANSFER_CHUNK_BYTES + 19

    result = await files.read_bytes("/tmp/payload", offset=offset, length=length)

    assert result == payload[offset : offset + length]
    assert all(action == "read_bytes" for action, _ in transport.calls)
    assert [params["offset"] for _, params in transport.calls] == [
        offset,
        offset + _TRANSFER_CHUNK_BYTES,
    ]
