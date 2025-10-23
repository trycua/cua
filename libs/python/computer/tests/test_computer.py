"""Unit tests for Computer class.

This file tests ONLY the Computer class initialization and context manager.
Following SRP: This file tests ONE class (Computer).
All external dependencies (providers, interfaces) are mocked.
"""

import pytest
from unittest.mock import Mock, AsyncMock, patch, MagicMock


class TestComputerImport:
    """Test Computer module imports (SRP: Only tests imports)."""

    def test_computer_class_exists(self):
        """Test that Computer class can be imported."""
        from computer import Computer
        
        assert Computer is not None

    def test_vm_provider_type_exists(self):
        """Test that VMProviderType enum can be imported."""
        from computer import VMProviderType
        
        assert VMProviderType is not None


class TestComputerInitialization:
    """Test Computer initialization (SRP: Only tests initialization)."""

    @patch("computer.computer.LocalProvider")
    @patch("computer.computer.Interface")
    def test_computer_initialization_with_defaults(self, mock_interface, mock_provider, disable_telemetry):
        """Test that Computer can be initialized with default parameters."""
        from computer import Computer
        
        computer = Computer()
        
        assert computer is not None

    @patch("computer.computer.CloudProvider")
    @patch("computer.computer.Interface")
    def test_computer_initialization_with_cloud_provider(self, mock_interface, mock_provider, disable_telemetry):
        """Test that Computer can be initialized with cloud provider."""
        from computer import Computer
        
        computer = Computer(
            provider_type="cloud",
            api_key="test-api-key"
        )
        
        assert computer is not None

    @patch("computer.computer.LocalProvider")
    @patch("computer.computer.Interface")
    def test_computer_initialization_with_os_type(self, mock_interface, mock_provider, disable_telemetry):
        """Test that Computer can be initialized with specific OS type."""
        from computer import Computer
        
        computer = Computer(os_type="linux")
        
        assert computer is not None


class TestComputerContextManager:
    """Test Computer context manager protocol (SRP: Only tests context manager)."""

    @pytest.mark.asyncio
    @patch("computer.computer.LocalProvider")
    @patch("computer.computer.Interface")
    async def test_computer_async_context_manager(self, mock_interface, mock_provider, disable_telemetry):
        """Test that Computer works as async context manager."""
        from computer import Computer
        
        # Mock provider
        mock_provider_instance = AsyncMock()
        mock_provider_instance.start = AsyncMock()
        mock_provider_instance.stop = AsyncMock()
        mock_provider.return_value = mock_provider_instance
        
        # Mock interface
        mock_interface_instance = AsyncMock()
        mock_interface.return_value = mock_interface_instance
        
        async with Computer() as computer:
            assert computer is not None
            assert hasattr(computer, "interface")

    @pytest.mark.asyncio
    @patch("computer.computer.LocalProvider")
    @patch("computer.computer.Interface")
    async def test_computer_cleanup_on_exit(self, mock_interface, mock_provider, disable_telemetry):
        """Test that Computer cleans up resources on exit."""
        from computer import Computer
        
        # Mock provider
        mock_provider_instance = AsyncMock()
        mock_provider_instance.start = AsyncMock()
        mock_provider_instance.stop = AsyncMock()
        mock_provider.return_value = mock_provider_instance
        
        # Mock interface
        mock_interface_instance = AsyncMock()
        mock_interface.return_value = mock_interface_instance
        
        async with Computer() as computer:
            pass
        
        # Provider stop should be called on exit
        mock_provider_instance.stop.assert_called_once()


class TestComputerInterface:
    """Test Computer.interface property (SRP: Only tests interface access)."""

    @pytest.mark.asyncio
    @patch("computer.computer.LocalProvider")
    @patch("computer.computer.Interface")
    async def test_computer_has_interface(self, mock_interface, mock_provider, disable_telemetry):
        """Test that Computer exposes an interface property."""
        from computer import Computer
        
        # Mock provider
        mock_provider_instance = AsyncMock()
        mock_provider_instance.start = AsyncMock()
        mock_provider_instance.stop = AsyncMock()
        mock_provider.return_value = mock_provider_instance
        
        # Mock interface
        mock_interface_instance = AsyncMock()
        mock_interface.return_value = mock_interface_instance
        
        async with Computer() as computer:
            assert hasattr(computer, "interface")
            assert computer.interface is not None
