"""Unit tests for CloudV2Provider.create_vm, instance_type routing, and docker image support.

Tests the new user-defined docker registry feature for VMI instances and gVisor containers.
"""

import json
from unittest.mock import AsyncMock, MagicMock, patch

import pytest


def _make_capture_session(mock_resp, captured_payload):
    """Helper: creates a mock aiohttp session that captures POST payloads."""
    mock_session = MagicMock()
    mock_session.__aenter__ = AsyncMock(return_value=mock_session)
    mock_session.__aexit__ = AsyncMock(return_value=None)

    def capturing_post(url, headers=None, json=None, **kwargs):
        if json:
            captured_payload.update(json)
        return mock_resp

    mock_session.post = capturing_post
    return mock_session


def _make_mock_resp(status, body):
    mock_resp = MagicMock()
    mock_resp.status = status
    mock_resp.json = AsyncMock(return_value=body)
    mock_resp.text = AsyncMock(return_value=str(body))
    mock_resp.__aenter__ = AsyncMock(return_value=mock_resp)
    mock_resp.__aexit__ = AsyncMock(return_value=None)
    return mock_resp


class TestCloudV2ProviderCreateVM:
    """Tests for CloudV2Provider.create_vm (SRP: Only tests create_vm)."""

    def _make_provider(self):
        from computer.providers.cloud.providerv2 import CloudV2Provider

        return CloudV2Provider(api_key="test-key", api_base="https://api.test.cua.ai")

    # -----------------------------------------------------------------
    # instanceType="vm" (default) — KubeVirt VMI path
    # -----------------------------------------------------------------

    @pytest.mark.asyncio
    async def test_create_vm_default_instance_type_is_vm(self):
        """create_vm with docker_image and no instance_type should send instanceType='vm'."""
        provider = self._make_provider()
        captured = {}
        resp = _make_mock_resp(202, {"status": "provisioning", "name": "witty-falcon", "source": "kubevirt-ubuntu"})
        session = _make_capture_session(resp, captured)

        with patch("aiohttp.ClientSession", return_value=session):
            result = await provider.create_vm(
                os="linux",
                region="us-east-1",
                docker_image="trycua/cua-ubuntu:custom",
            )

        assert result["name"] == "witty-falcon"
        assert result["source"] == "kubevirt-ubuntu"
        assert captured["dockerImage"] == "trycua/cua-ubuntu:custom"
        assert captured["instanceType"] == "vm"

    @pytest.mark.asyncio
    async def test_create_vm_explicit_instance_type_vm(self):
        """create_vm with instance_type='vm' should send instanceType='vm'."""
        provider = self._make_provider()
        captured = {}
        resp = _make_mock_resp(202, {"status": "provisioning", "name": "brave-eagle", "source": "kubevirt-ubuntu"})
        session = _make_capture_session(resp, captured)

        with patch("aiohttp.ClientSession", return_value=session):
            result = await provider.create_vm(
                os="linux",
                region="us-east-1",
                docker_image="myorg/myimage:v1",
                instance_type="vm",
            )

        assert captured["instanceType"] == "vm"
        assert result["source"] == "kubevirt-ubuntu"

    # -----------------------------------------------------------------
    # instanceType="container" — gVisor/Incus container path
    # -----------------------------------------------------------------

    @pytest.mark.asyncio
    async def test_create_vm_instance_type_container(self):
        """create_vm with instance_type='container' should send instanceType='container'."""
        provider = self._make_provider()
        captured = {}
        resp = _make_mock_resp(202, {"status": "provisioning", "name": "calm-fox", "source": "incus-container"})
        session = _make_capture_session(resp, captured)

        with patch("aiohttp.ClientSession", return_value=session):
            result = await provider.create_vm(
                os="linux",
                region="us-east-1",
                docker_image="myorg/myapp:latest",
                instance_type="container",
            )

        assert captured["dockerImage"] == "myorg/myapp:latest"
        assert captured["instanceType"] == "container"
        assert result["source"] == "incus-container"

    # -----------------------------------------------------------------
    # No docker image → instanceType not sent
    # -----------------------------------------------------------------

    @pytest.mark.asyncio
    async def test_create_vm_without_docker_image_no_instance_type_in_payload(self):
        """create_vm should NOT include dockerImage or instanceType when image is None."""
        provider = self._make_provider()
        captured = {}
        resp = _make_mock_resp(202, {"status": "provisioning", "name": "calm-panda"})
        session = _make_capture_session(resp, captured)

        with patch("aiohttp.ClientSession", return_value=session):
            await provider.create_vm(os="linux", region="us-east-1")

        assert "dockerImage" not in captured
        assert "instanceType" not in captured

    # -----------------------------------------------------------------
    # Error cases
    # -----------------------------------------------------------------

    @pytest.mark.asyncio
    async def test_create_vm_raises_on_400(self):
        """create_vm should raise RuntimeError on 400 Bad Request."""
        provider = self._make_provider()
        resp = _make_mock_resp(400, '{"error":"invalid dockerImage"}')
        session = _make_capture_session(resp, {})

        with patch("aiohttp.ClientSession", return_value=session):
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
        resp = _make_mock_resp(401, "Unauthorized")
        session = _make_capture_session(resp, {})

        with patch("aiohttp.ClientSession", return_value=session):
            with pytest.raises(RuntimeError, match="Unauthorized"):
                await provider.create_vm(os="linux", region="us-east-1")

    @pytest.mark.asyncio
    async def test_create_vm_raises_on_402(self):
        """create_vm should raise RuntimeError on 402 Payment Required."""
        provider = self._make_provider()
        resp = _make_mock_resp(402, '{"error":"Free credits exhausted"}')
        session = _make_capture_session(resp, {})

        with patch("aiohttp.ClientSession", return_value=session):
            with pytest.raises(RuntimeError, match="Payment required"):
                await provider.create_vm(os="linux", region="us-east-1")
