"""Chunked reading and gzip helpers for uploads, with e2b's names and behaviour."""

from __future__ import annotations

import asyncio
import zlib
from typing import IO, AsyncIterable, AsyncIterator, Iterable, Iterator

IO_CHUNK_SIZE = 65_536


def iter_io_chunks(data: IO) -> Iterator[bytes]:
    """Read a file-like object in chunks, encoding text chunks to UTF-8."""
    while True:
        chunk = data.read(IO_CHUNK_SIZE)
        if not chunk:
            break
        yield chunk if isinstance(chunk, bytes) else chunk.encode("utf-8")


async def aiter_io_chunks(data: IO) -> AsyncIterator[bytes]:
    """As `iter_io_chunks`; each blocking `read` runs in a worker thread."""
    while True:
        chunk = await asyncio.to_thread(data.read, IO_CHUNK_SIZE)
        if not chunk:
            break
        yield chunk if isinstance(chunk, bytes) else chunk.encode("utf-8")


def _gzip_compressor():
    # wbits > 16 makes zlib produce a gzip-formatted stream.
    return zlib.compressobj(wbits=zlib.MAX_WBITS | 16)


def gzip_iter(chunks: Iterable[bytes]) -> Iterator[bytes]:
    """Gzip-compress a byte stream chunk by chunk."""
    compressor = _gzip_compressor()
    for chunk in chunks:
        compressed = compressor.compress(chunk)
        if compressed:
            yield compressed
    yield compressor.flush()


async def agzip_iter(chunks: AsyncIterable[bytes]) -> AsyncIterator[bytes]:
    """As `gzip_iter` for an async stream; compression runs in a worker thread."""
    compressor = _gzip_compressor()
    async for chunk in chunks:
        compressed = await asyncio.to_thread(compressor.compress, chunk)
        if compressed:
            yield compressed
    yield await asyncio.to_thread(compressor.flush)
