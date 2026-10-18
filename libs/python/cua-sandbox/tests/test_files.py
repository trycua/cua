"""File listing metadata from computer-server responses."""

from unittest.mock import AsyncMock, call

import pytest
from cua_sandbox.interfaces.files import FileEntry, Files
from cua_sandbox.transport.base import Transport

pytestmark = pytest.mark.asyncio


@pytest.mark.parametrize("path", ["/work", "/work/", "/", "work"])
async def test_list_classifies_name_only_entries(path):
    transport = AsyncMock(spec=Transport)
    transport.send.side_effect = [
        {"success": True, "files": ["subdir", "note.txt"]},
        {"success": True, "exists": True},
        {"success": True, "exists": False},
    ]
    prefix = path.rstrip("/")

    entries = await Files(transport).list(path)

    assert entries == [
        FileEntry("subdir", f"{prefix}/subdir", True),
        FileEntry("note.txt", f"{prefix}/note.txt", False),
    ]
    assert transport.send.await_args_list == [
        call("list_dir", path=path),
        call("directory_exists", path=f"{prefix}/subdir"),
        call("directory_exists", path=f"{prefix}/note.txt"),
    ]


async def test_list_propagates_directory_lookup_failure():
    transport = AsyncMock(spec=Transport)
    transport.send.side_effect = [
        {"success": True, "files": ["subdir"]},
        RuntimeError("connection closed"),
    ]

    with pytest.raises(RuntimeError, match="connection closed"):
        await Files(transport).list("/work")
