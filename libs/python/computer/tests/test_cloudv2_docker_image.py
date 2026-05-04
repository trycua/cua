"""Unit tests for CloudV2Provider.create_vm and docker image support.

Tests the new user-defined docker registry feature for VMI instances.
"""

import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest


class TestCloudV2ProviderCreateVM:
    """Tests for CloudV2Provider.create_vm (SRP: Only tests create_vm)."""

    def _make_provider(self):
        from computer.providers.cloud.providerv2 import CloudV2Provider

        return CloudV2Provider(api_key="test-key", api_base="https://api.test.cua.ai")

    @pytest.mark.asyncio
    async def test_create_vm_with_public_docker_image(self):
        """create_vm should POST dockerImage to /v1/vms and return the name."""
        provider = self._make_provider()

        response_body = {
            "status": "provisioning",
            "name": "witty-falcon",
            "source": "kubevirt-ubuntu",
        }

        mock_resp = MagicMock()
        mock_resp.status = 202
        mock_resp.json = AsyncMock(return_value=response_body)
        mock_resp.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_resp.__aexit__ = AsyncMock(return_value=None)

        mock_session = MagicMock()
        mock_session.post = MagicMock(return_value=mock_resp)
        mock_session.__aenter__ = AsyncMock(return_value=mock_session)
        mock_session.__aexit__ = AsyncMock(return_value=None)

        captured_payload = {}

        original_post = mock_session.post

        def capturing_post(url, headers=None, json=None, **kwargs):
            captured_payload.update(json or {})
            return original_post(url, headers=headers, json=json, **kwargs)

        mock_session.post = capturing_post

        with patch("aiohttp.ClientSession", return_value=mock_session):
            result = await provider.create_vm(
                os="linux",
                region="us-east-1",
                docker_image="trycua/cua-ubuntu:custom",
            )

        assert result["name"] == "witty-falcon"
        assert result["status"] == "provisioning"
        assert captured_payload.get("dockerImage") == "trycua/cua-ubuntu:custom"
        assert captured_payload.get("os") == "linux"
        assert captured_payload.get("region") == "us-east-1"

    @pytest.mark.asyncio
    async def test_create_vm_without_docker_image(self):
        """create_vm should NOT include dockerImage when image is None."""
        provider = self._make_provider()

        response_body = {"status": "provisioning", "name": "calm-panda"}
        mock_resp = MagicMock()
        mock_resp.status = 202
        mock_resp.json = AsyncMock(return_value=response_body)
        mock_resp.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_resp.__aexit__ = AsyncMock(return_value=None)

        mock_session = MagicMock()
        mock_session.__aenter__ = AsyncMock(return_value=mock_session)
        mock_session.__aexit__ = AsyncMock(return_value=None)

        captured_payload = {}

        def capturing_post(url, headers=None, json=None, **kwargs):
            captured_payload.update(json or {})
            return mock_resp

        mock_session.post = capturing_post

        with patch("aiohttp.ClientSession", return_value=mock_session):
            result = await provider.create_vm(os="linux", region="us-east-1")

        assert "dockerImage" not in captured_payload

    @pytest.mark.asyncio
    async def test_create_vm_raises_on_400(self):
        """create_vm should raise RuntimeError on 400 Bad Request."""
        provider = self._make_provider()

        mock_resp = MagicMock()
        mock_resp.status = 400
        mock_resp.text = AsyncMock(return_value='{"error":"invalid dockerImage"}')
        mock_resp.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_resp.__aexit__ = AsyncMock(return_value=None)

        mock_session = MagicMock()
        mock_session.__aenter__ = AsyncMock(return_value=mock_session)
        mock_session.__aexit__ = AsyncMock(return_value=None)
        mock_session.post = MagicMock(return_value=mock_resp)

        with patch("aiohttp.ClientSession", return_value=mock_session):
            with pytest.raises(RuntimeError, match="Bad request"):
                await provider.create_vm(
                    os="linux",
                    region="us-east-1",
                    docker_image="invalid image",
                )

    @pytest.mark.asyncio
    async def test_create_vm_raises_on_401(self):
        """create_vm should raise RuntimeError on 401 Unauthorized."""
        provider = self._make_provider()

        mock_resp = MagicMock()
        mock_resp.status = 401
        mock_resp.text = AsyncMock(return_value="Unauthorized")
        mock_resp.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_resp.__aexit__ = AsyncMock(return_value=None)

        mock_session = MagicMock()
        mock_session.__aenter__ = AsyncMock(return_value=mock_session)
        mock_session.__aexit__ = AsyncMock(return_value=None)
        mock_session.post = MagicMock(return_value=mock_resp)

        with patch("aiohttp.ClientSession", return_value=mock_session):
            with pytest.raises(RuntimeError, match="Unauthorized"):
                await provider.create_vm(os="linux", region="us-east-1")

    @pytest.mark.asyncio
    async def test_create_vm_raises_on_402(self):
        """create_vm should raise RuntimeError on 402 Payment Required."""
        provider = self._make_provider()

        mock_resp = MagicMock()
        mock_resp.status = 402
        mock_resp.text = AsyncMock(return_value='{"error":"Free credits exhausted"}')
        mock_resp.__aenter__ = AsyncMock(return_value=mock_resp)
        mock_resp.__aexit__ = AsyncMock(return_value=None)

        mock_session = MagicMock()
        mock_session.__aenter__ = AsyncMock(return_value=mock_session)
        mock_session.__aexit__ = AsyncMock(return_value=None)
        mock_session.post = MagicMock(return_value=mock_resp)

        with patch("aiohttp.ClientSession", return_value=mock_session):
            with pytest.raises(RuntimeError, match="Payment required"):
                await provider.create_vm(os="linux", region="us-east-1")
