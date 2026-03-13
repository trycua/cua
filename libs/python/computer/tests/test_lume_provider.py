"""Unit tests for local Lume provider startup behavior."""

import subprocess
from unittest.mock import MagicMock, patch

from computer.providers.factory import VMProviderFactory
from computer.providers.lume.provider import LumeProvider


class TestVMProviderFactory:
    """Test provider factory compatibility behavior."""

    def test_factory_accepts_port_alias_for_lume(self):
        """Older callers passing `port=` should still override provider_port."""
        provider = VMProviderFactory.create_provider("lume", port=3000)
        assert isinstance(provider, LumeProvider)
        assert provider.port == 3000


class TestLumeProviderLifecycle:
    """Test local Lume server management behavior."""

    @patch("computer.providers.lume.provider.subprocess.Popen")
    def test_provider_starts_local_server_when_needed(self, mock_popen):
        """Provider should launch `lume serve` if no server is reachable."""
        process = MagicMock()
        process.poll.return_value = None
        mock_popen.return_value = process

        provider = LumeProvider(provider_port=7777)

        with patch.object(
            provider, "_is_server_ready", side_effect=[False, False, False, True]
        ) as mock_ready:
            provider._ensure_server()

        assert provider._manages_server is True
        assert mock_ready.call_count >= 2
        mock_popen.assert_called_once_with(
            ["lume", "serve", "--port", "7777"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )

    def test_provider_reuses_existing_server_on_legacy_port(self):
        """Provider should attach to an already-running daemon on port 3000."""
        provider = LumeProvider(provider_port=7777)

        with patch.object(provider, "_is_server_ready", side_effect=[False, True]):
            attached = provider._attach_to_existing_server()

        assert attached is True
        assert provider.port == 3000
        assert provider.api_base_url == "http://localhost:3000"
