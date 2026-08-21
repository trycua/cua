"""Files interface — file operations inside the sandbox, backed by a Transport.

Wraps the computer-server file_handler commands (read_text, write_text,
read_bytes, write_bytes, file_exists, directory_exists, list_dir, create_dir,
delete_file, delete_dir, get_file_size).

Adds two helpers on top of the wire commands:
- ``upload(local_path, remote_path)`` — push a host file into the sandbox.
- ``download(remote_path, local_path)`` — pull a sandbox file to the host.
"""

from __future__ import annotations

import base64
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional, Union

from cua_sandbox.transport.base import Transport


# Fleet service requests pass through a gateway with a 1 MiB body limit. Binary
# payloads expand by one third when base64 encoded, so leave room for the JSON
# command envelope instead of relying on the gateway's exact implementation.
_TRANSFER_CHUNK_BYTES = 512 * 1024


@dataclass
class FileEntry:
    name: str
    path: str
    is_dir: bool
    size: Optional[int] = None


def _unwrap(result: Any, key: str, default: Any = None) -> Any:
    """computer-server returns dicts like {'success': True, 'content': ...}."""
    if isinstance(result, dict):
        return result.get(key, default)
    return default


class Files:
    """File and directory operations inside the sandbox.

    All paths are inside the sandbox unless otherwise noted.
    """

    def __init__(self, transport: Transport):
        self._t = transport

    # ── existence / metadata ─────────────────────────────────────────────
    async def exists(self, path: str) -> bool:
        result = await self._t.send("file_exists", path=path)
        return bool(_unwrap(result, "exists", False))

    async def is_dir(self, path: str) -> bool:
        result = await self._t.send("directory_exists", path=path)
        return bool(_unwrap(result, "exists", False))

    async def size(self, path: str) -> int:
        result = await self._t.send("get_file_size", path=path)
        return int(_unwrap(result, "size", 0) or 0)

    # ── directories ──────────────────────────────────────────────────────
    async def list(self, path: str) -> list[FileEntry]:
        result = await self._t.send("list_dir", path=path)
        raw = _unwrap(result, "files", []) or _unwrap(result, "entries", []) or []
        out: list[FileEntry] = []
        for item in raw:
            if isinstance(item, str):
                out.append(FileEntry(name=item, path=f"{path.rstrip('/')}/{item}", is_dir=False))
            elif isinstance(item, dict):
                out.append(
                    FileEntry(
                        name=item.get("name", ""),
                        path=item.get("path", f"{path.rstrip('/')}/{item.get('name', '')}"),
                        is_dir=bool(item.get("is_dir", False)),
                        size=item.get("size"),
                    )
                )
        return out

    async def make_dir(self, path: str) -> None:
        await self._t.send("create_dir", path=path)

    async def remove_dir(self, path: str) -> None:
        await self._t.send("delete_dir", path=path)

    async def remove(self, path: str) -> None:
        await self._t.send("delete_file", path=path)

    # ── text I/O ─────────────────────────────────────────────────────────
    async def read_text(self, path: str) -> str:
        result = await self._t.send("read_text", path=path)
        return _unwrap(result, "content", "") or ""

    async def write_text(self, path: str, content: str) -> None:
        await self._t.send("write_text", path=path, content=content)

    # ── binary I/O ───────────────────────────────────────────────────────
    async def read_bytes(self, path: str, offset: int = 0, length: Optional[int] = None) -> bytes:
        requested_length = length
        if requested_length is None:
            requested_length = max(0, await self.size(path) - max(0, offset))

        if requested_length <= _TRANSFER_CHUNK_BYTES:
            result = await self._t.send(
                "read_bytes", path=path, offset=offset, length=requested_length
            )
            b64 = _unwrap(result, "content_b64", "") or _unwrap(result, "content", "") or ""
            return base64.b64decode(b64) if b64 else b""

        chunks: list[bytes] = []
        current_offset = max(0, offset)
        remaining = requested_length
        while remaining > 0:
            chunk_length = min(remaining, _TRANSFER_CHUNK_BYTES)
            result = await self._t.send(
                "read_bytes", path=path, offset=current_offset, length=chunk_length
            )
            b64 = _unwrap(result, "content_b64", "") or _unwrap(result, "content", "") or ""
            chunk = base64.b64decode(b64) if b64 else b""
            if not chunk:
                break
            chunks.append(chunk)
            current_offset += len(chunk)
            remaining -= len(chunk)
        return b"".join(chunks)

    async def write_bytes(self, path: str, content: bytes) -> None:
        if len(content) <= _TRANSFER_CHUNK_BYTES:
            b64 = base64.b64encode(content).decode("ascii")
            await self._t.send("write_bytes", path=path, content_b64=b64)
            return

        for offset in range(0, len(content), _TRANSFER_CHUNK_BYTES):
            chunk = content[offset : offset + _TRANSFER_CHUNK_BYTES]
            await self._t.send(
                "write_bytes",
                path=path,
                content_b64=base64.b64encode(chunk).decode("ascii"),
                append=offset > 0,
            )

    # ── host ↔ sandbox transfer ──────────────────────────────────────────
    async def upload(self, local_path: Union[str, Path], remote_path: str) -> None:
        """Push a file from the host into the sandbox."""
        data = Path(local_path).read_bytes()
        await self.write_bytes(remote_path, data)

    async def download(self, remote_path: str, local_path: Union[str, Path]) -> None:
        """Pull a file from the sandbox down to the host."""
        data = await self.read_bytes(remote_path)
        Path(local_path).write_bytes(data)
