"""Async file getters - download files from VM, cloud, check existence."""

import logging
import os
from datetime import datetime
from typing import Any, Dict, List, Optional, Set, Union

import aiohttp
import pandas as pd

from .base import get_cache_path

logger = logging.getLogger("winarena.getters_async.file")


async def get_cloud_file(session, config: Dict[str, Any]) -> Union[str, List[str]]:
    """Download file(s) from a URL.

    Config:
        path (str|List[str]): URL(s) to download from
        dest (str|List[str]): Destination filename(s)
        multi (bool): Whether path/dest are lists (default: False)
        gives (List[int]): Which files to return (default: [0])

    Returns:
        Path(s) to downloaded file(s)
    """
    if not config.get("multi", False):
        paths: List[str] = [config["path"]]
        dests: List[str] = [config["dest"]]
    else:
        paths = config["path"]
        dests = config["dest"]

    cache_paths: List[str] = []
    gives: Set[int] = set(config.get("gives", [0]))

    async with aiohttp.ClientSession() as http:
        for i, (url, dest) in enumerate(zip(paths, dests)):
            local_path = get_cache_path(dest)

            if i in gives:
                cache_paths.append(str(local_path))

            # Skip if already cached
            if local_path.exists():
                continue

            try:
                async with http.get(url) as response:
                    response.raise_for_status()
                    content = await response.read()
                    local_path.write_bytes(content)
            except Exception as e:
                logger.error(f"Failed to download {url}: {e}")
                if i in gives and str(local_path) in cache_paths:
                    cache_paths.remove(str(local_path))
                    cache_paths.append(None)

    return cache_paths[0] if len(cache_paths) == 1 else cache_paths


async def get_vm_file(session, config: Dict[str, Any]) -> Union[Optional[str], List[Optional[str]]]:
    """Download file(s) from the VM.

    Config:
        path (str|List[str]): Path(s) on VM
        dest (str|List[str]): Local destination filename(s)
        multi (bool): Whether path/dest are lists (default: False)
        gives (List[int]): Which files to return (default: [0])
        time_suffix (bool): Append timestamp to filename (default: False)
        time_format (str): Timestamp format (default: "%Y_%m_%d")

    Returns:
        Local path(s) to downloaded file(s)
    """
    time_format = config.get("time_format", "%Y_%m_%d")

    if not config.get("multi", False):
        paths: List[str] = [config["path"]]
        dests: List[str] = [config["dest"]]

        if config.get("time_suffix", False):
            timestamp = datetime.now().strftime(time_format)
            paths = [
                (
                    p.rsplit(".", 1)[0] + timestamp + "." + p.rsplit(".", 1)[1]
                    if "." in p
                    else p + timestamp
                )
                for p in paths
            ]
            dests = [
                (
                    d.rsplit(".", 1)[0] + timestamp + "." + d.rsplit(".", 1)[1]
                    if "." in d
                    else d + timestamp
                )
                for d in dests
            ]
    else:
        paths = config["path"]
        dests = config["dest"]

    cache_paths: List[Optional[str]] = []
    gives: Set[int] = set(config.get("gives", [0]))

    for i, (vm_path, dest) in enumerate(zip(paths, dests)):
        local_path = get_cache_path(dest)

        try:
            content = await session.read_bytes(vm_path)
            if content is None:
                if i in gives:
                    cache_paths.append(None)
                continue

            local_path.write_bytes(content)
            if i in gives:
                cache_paths.append(str(local_path))

        except Exception as e:
            logger.error(f"Failed to get VM file {vm_path}: {e}")
            if i in gives:
                cache_paths.append(None)

    return cache_paths[0] if len(cache_paths) == 1 else cache_paths


async def get_cache_file(session, config: Dict[str, Any]) -> str:
    """Get path to a cached file.

    Config:
        path (str): Relative path in cache directory

    Returns:
        Absolute path to cached file

    Raises:
        AssertionError if file doesn't exist
    """
    local_path = get_cache_path(config["path"])
    assert local_path.exists(), f"Cache file not found: {local_path}"
    return str(local_path)


async def get_content_from_vm_file(session, config: Dict[str, Any]) -> Any:
    """Get content from a VM file with specific parsing.

    Config:
        path (str): Path on VM
        file_type (str): Type of file (e.g., 'xlsx')
        file_content (str): What to extract (e.g., 'last_row')

    Returns:
        Extracted content based on file_type and file_content
    """
    path = config["path"]
    file_type = config["file_type"]
    file_content = config["file_content"]

    # Download file first
    local_path = await get_vm_file(session, {"path": path, "dest": os.path.basename(path)})

    if local_path is None:
        return None

    if file_type == "xlsx":
        if file_content == "last_row":
            df = pd.read_excel(local_path)
            last_row = df.iloc[-1]
            return last_row.astype(str).tolist()
    else:
        raise NotImplementedError(f"File type {file_type} not supported")

    return None


async def get_vm_file_exists_in_vm_folder(session, config: Dict[str, Any]) -> float:
    """Check if a file exists in a folder on the VM.

    Config:
        folder_name (str): Folder path on VM
        file_name (str): File name to check

    Returns:
        1.0 if file exists, 0.0 otherwise
    """
    folder_name = config.get("folder_name")
    file_name = config.get("file_name")

    if not folder_name or not file_name:
        return 0.0

    # Construct full path
    if "\\" in folder_name:
        file_path = f"{folder_name}\\{file_name}"
    else:
        file_path = f"{folder_name}/{file_name}"

    try:
        exists = await session.file_exists(file_path)
        return 1.0 if exists else 0.0
    except Exception as e:
        logger.error(f"Failed to check file existence: {e}")
        return 0.0
