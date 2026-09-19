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


@pytest.mark.parametrize("key", ["files", "entries"])
async def test_list_preserves_supplied_metadata(key):
    transport = AsyncMock(spec=Transport)
    transport.send.return_value = {
        "success": True,
        key: [
            {"name": "subdir", "path": "/work/subdir", "is_dir": True},
            {"name": "note.txt", "is_dir": False, "size": 12},
        ],
    }

    assert await Files(transport).list("/work/") == [
        FileEntry("subdir", "/work/subdir", True),
        FileEntry("note.txt", "/work/note.txt", False, size=12),
    ]
    transport.send.assert_awaited_once_with("list_dir", path="/work/")


async def test_list_empty_directory():
    transport = AsyncMock(spec=Transport)
    transport.send.return_value = {"success": True, "files": []}

    assert await Files(transport).list("/work") == []
    transport.send.assert_awaited_once_with("list_dir", path="/work")


async def test_list_propagates_directory_lookup_failure():
    transport = AsyncMock(spec=Transport)
    transport.send.side_effect = [
        {"success": True, "files": ["subdir"]},
        RuntimeError("connection closed"),
    ]

    with pytest.raises(RuntimeError, match="connection closed"):
        await Files(transport).list("/work")
