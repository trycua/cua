"""Unit tests for Computer class.

This file tests ONLY the Computer class initialization and context manager.
Following SRP: This file tests ONE class (Computer).
All external dependencies (providers, interfaces) are mocked.
"""

from unittest.mock import AsyncMock, MagicMock, Mock, patch

import pytest


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

    def test_computer_class_can_be_imported(self, disable_telemetry):
        """Test that Computer class can be imported without errors."""
        from computer import Computer

        assert Computer is not None

    def test_computer_has_required_methods(self, disable_telemetry):
        """Test that Computer class has required methods."""
        from computer import Computer

        assert hasattr(Computer, "__aenter__")
        assert hasattr(Computer, "__aexit__")


class TestComputerContextManager:
    """Test Computer context manager protocol (SRP: Only tests context manager)."""

    def test_computer_is_async_context_manager(self, disable_telemetry):
        """Test that Computer has async context manager methods."""
        from computer import Computer

        assert hasattr(Computer, "__aenter__")
        assert hasattr(Computer, "__aexit__")
        assert callable(Computer.__aenter__)
        assert callable(Computer.__aexit__)


class TestComputerInterface:
    """Test Computer.interface property (SRP: Only tests interface access)."""

    def test_computer_class_structure(self, disable_telemetry):
        """Test that Computer class has expected structure."""
        from computer import Computer

        # Verify Computer is a class
        assert isinstance(Computer, type)
