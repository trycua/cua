"""Unit tests for version compatibility checking between Computer SDK and Computer Server.

This test verifies that the Computer SDK properly checks protocol version compatibility
when connecting to Computer Server, addressing Issue #544.
"""

import pytest
from unittest.mock import MagicMock, patch


class TestVersionCompatibility:
    """Test version compatibility checking between SDK and Server."""

    @pytest.mark.anyio
    async def test_compatible_versions(self):
        """Test that compatible versions connect successfully."""
        from computer.interface.generic import GenericComputerInterface

        interface = GenericComputerInterface(ip_address="192.168.1.100")

        # Mock the REST API response with compatible version
        mock_response = {
            "success": True,
            "protocol": 1,
            "package": "0.1.29"
        }

        # Create async mock functions
        async def mock_send_command_rest(*args, **kwargs):
            return mock_response

        async def mock_send_command(*args, **kwargs):
            return {"success": True, "width": 1920, "height": 1080}

        with patch.object(interface, '_send_command_rest', new=mock_send_command_rest):
            with patch.object(interface, '_send_command', new=mock_send_command):
                # This should succeed without raising an exception
                await interface.wait_for_ready(timeout=5)

    @pytest.mark.anyio
    async def test_incompatible_versions(self):
        """Test that incompatible versions raise an error."""
        from computer.interface.generic import GenericComputerInterface

        interface = GenericComputerInterface(ip_address="192.168.1.100")

        # Mock the REST API response with incompatible version
        mock_response = {
            "success": True,
            "protocol": 2,  # Different protocol version
            "package": "0.2.0"
        }

        async def mock_send_command_rest(*args, **kwargs):
            return mock_response

        with patch.object(interface, '_send_command_rest', new=mock_send_command_rest):
            # This should raise a RuntimeError due to version mismatch
            with pytest.raises(RuntimeError) as exc_info:
                await interface.wait_for_ready(timeout=5)

            # Verify the error message contains helpful information
            error_msg = str(exc_info.value)
            assert "Version mismatch" in error_msg
            assert "protocol version 1" in error_msg
            assert "protocol version 2" in error_msg
            assert "pip install --upgrade cua-computer" in error_msg

    @pytest.mark.anyio
    async def test_missing_version_info(self):
        """Test that missing version info generates a warning but continues."""
        from computer.interface.generic import GenericComputerInterface

        interface = GenericComputerInterface(ip_address="192.168.1.100")

        # Mock the REST API response without version info
        mock_response = {
            "success": True
            # No protocol or package version
        }

        async def mock_send_command_rest(*args, **kwargs):
            return mock_response

        async def mock_send_command(*args, **kwargs):
            return {"success": True, "width": 1920, "height": 1080}

        with patch.object(interface, '_send_command_rest', new=mock_send_command_rest):
            with patch.object(interface, '_send_command', new=mock_send_command):
                # This should succeed but log a warning
                # We can't easily test the warning, but we verify it doesn't raise
                await interface.wait_for_ready(timeout=5)

    @pytest.mark.anyio
    async def test_websocket_version_check(self):
        """Test that WebSocket fallback also checks version compatibility."""
        from computer.interface.generic import GenericComputerInterface
        import websockets.protocol

        interface = GenericComputerInterface(ip_address="192.168.1.100")

        # Mock WebSocket connection
        mock_ws = MagicMock()
        mock_ws.state = websockets.protocol.State.OPEN
        interface._ws = mock_ws

        # Mock version command response (incompatible)
        version_response = {
            "success": True,
            "protocol": 99,
            "package": "99.0.0"
        }

        async def mock_send_command_ws(*args, **kwargs):
            # First call returns version, second would be screen size
            return version_response

        with patch.object(interface, '_send_command_ws', new=mock_send_command_ws):
            # This should raise due to version mismatch
            with pytest.raises(RuntimeError) as exc_info:
                await interface._wait_for_ready_ws(timeout=5)

            error_msg = str(exc_info.value)
            assert "Version mismatch" in error_msg
            assert "protocol version 99" in error_msg


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
