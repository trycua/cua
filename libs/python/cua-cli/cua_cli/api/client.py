"""HTTP API client for CUA cloud services."""

import hashlib
import os
from pathlib import Path
from typing import Any, Optional
from urllib.parse import quote

import aiohttp
from cua_cli.auth.store import require_api_key

DEFAULT_API_BASE = "https://api.cua.ai"


def get_api_base() -> str:
    """Get the API base URL."""
    return os.environ.get("CUA_API_BASE", DEFAULT_API_BASE).rstrip("/")


class CloudAPIClient:
    """HTTP client for CUA cloud API."""

    def __init__(self, api_key: Optional[str] = None, api_base: Optional[str] = None):
        self.api_key = api_key or require_api_key()
        self.api_base = api_base or get_api_base()

    def _headers(self) -> dict[str, str]:
        return {
            "Authorization": f"Bearer {self.api_key}",
            "Accept": "application/json",
        }

    async def _request(
        self,
        method: str,
        path: str,
        json: Optional[dict] = None,
        timeout: int = 30,
    ) -> tuple[int, Any]:
        """Make an HTTP request to the API.

        Returns:
            Tuple of (status_code, response_data)
        """
        url = f"{self.api_base}{path}"
        headers = self._headers()

        if json is not None:
            headers["Content-Type"] = "application/json"

        async with aiohttp.ClientSession() as session:
            timeout_obj = aiohttp.ClientTimeout(total=timeout)
            async with session.request(
                method, url, headers=headers, json=json, timeout=timeout_obj
            ) as resp:
                try:
                    data = await resp.json(content_type=None)
                except Exception:
                    data = await resp.text()
                return resp.status, data

    # Image API methods

    async def list_images(self) -> list[dict[str, Any]]:
        """List all images in the workspace."""
        status, data = await self._request("GET", "/v1/images")
        if status == 200 and isinstance(data, list):
            return data
        return []

    async def initiate_upload(
        self,
        name: str,
        tag: str,
        image_type: str,
        size_bytes: int,
        checksum_sha256: str,
    ) -> tuple[int, dict[str, Any]]:
        """Initiate a multi-part upload session.

        Returns:
            Tuple of (status_code, session_data)
        """
        path = f"/v1/images/{quote(name, safe='')}/upload"
        body = {
            "tag": tag,
            "image_type": image_type,
            "size_bytes": size_bytes,
            "checksum_sha256": checksum_sha256,
        }
        return await self._request("POST", path, json=body)

    async def get_upload_part_url(
        self, name: str, upload_id: str, part_number: int
    ) -> tuple[int, dict[str, Any]]:
        """Get a signed URL for uploading a part."""
        path = f"/v1/images/{quote(name, safe='')}/upload/{upload_id}/part/{part_number}"
        return await self._request("GET", path)

    async def complete_upload(
        self, name: str, upload_id: str, parts: list[dict[str, Any]]
    ) -> tuple[int, dict[str, Any]]:
        """Complete a multi-part upload."""
        path = f"/v1/images/{quote(name, safe='')}/upload/{upload_id}/complete"
        return await self._request("POST", path, json={"parts": parts})

    async def abort_upload(self, name: str, upload_id: str) -> tuple[int, Any]:
        """Abort an upload session."""
        path = f"/v1/images/{quote(name, safe='')}/upload/{upload_id}"
        return await self._request("DELETE", path)

    async def get_download_url(self, name: str, tag: str) -> tuple[int, dict[str, Any]]:
        """Get a signed URL for downloading an image."""
        path = f"/v1/images/{quote(name, safe='')}/download?tag={quote(tag, safe='')}"
        return await self._request("GET", path)

    async def delete_image(self, name: str, tag: str) -> tuple[int, Any]:
        """Delete an image version."""
        path = f"/v1/images/{quote(name, safe='')}?tag={quote(tag, safe='')}"
        return await self._request("DELETE", path)


def calculate_file_hash(file_path: Path) -> str:
    """Calculate SHA256 hash of a file."""
    sha256 = hashlib.sha256()
    with open(file_path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            sha256.update(chunk)
    return sha256.hexdigest()


def format_bytes(size_bytes: int) -> str:
    """Format bytes in human-readable format."""
    if size_bytes == 0:
        return "0 B"
    units = ["B", "KB", "MB", "GB", "TB"]
    k = 1024
    i = 0
    while size_bytes >= k and i < len(units) - 1:
        size_bytes /= k
        i += 1
    return f"{size_bytes:.2f} {units[i]}"
