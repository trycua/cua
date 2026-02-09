"""Tests for output utilities."""

from unittest.mock import MagicMock, patch

import pytest
from cua_cli.utils.output import (
    print_error,
    print_info,
    print_json,
    print_success,
    print_table,
    print_warning,
)


class TestPrintTable:
    """Tests for print_table function."""

    def test_prints_table_with_data(self):
        """Test printing a table with data."""
        data = [
            {"name": "test1", "status": "running"},
            {"name": "test2", "status": "stopped"},
        ]
        columns = [("name", "NAME"), ("status", "STATUS")]

        with patch("cua_cli.utils.output.console") as mock_console:
            print_table(data, columns)

        mock_console.print.assert_called_once()

    def test_handles_empty_data(self):
        """Test handling empty data list."""
        data = []

        with patch("cua_cli.utils.output.console") as mock_console:
            print_table(data)

        mock_console.print.assert_called_once()
        # Should print "No data" message
        call_args = mock_console.print.call_args[0][0]
        assert "No data" in call_args

    def test_auto_generates_columns(self):
        """Test that columns are auto-generated from data keys."""
        data = [{"foo": "bar", "baz": "qux"}]

        with patch("cua_cli.utils.output.console") as mock_console:
            print_table(data)

        mock_console.print.assert_called_once()

    def test_handles_missing_keys(self):
        """Test handling data items with missing keys."""
        data = [
            {"name": "test1", "status": "running"},
            {"name": "test2"},  # Missing status
        ]
        columns = [("name", "NAME"), ("status", "STATUS")]

        with patch("cua_cli.utils.output.console") as mock_console:
            print_table(data, columns)

        mock_console.print.assert_called_once()

    def test_with_title(self):
        """Test table with title."""
        data = [{"name": "test"}]

        with patch("cua_cli.utils.output.console") as mock_console:
            print_table(data, title="Test Table")

        mock_console.print.assert_called_once()


class TestPrintJson:
    """Tests for print_json function."""

    def test_prints_dict(self):
        """Test printing a dictionary as JSON."""
        data = {"key": "value", "number": 42}

        with patch("cua_cli.utils.output.console") as mock_console:
            print_json(data)

        mock_console.print_json.assert_called_once()

    def test_prints_list(self):
        """Test printing a list as JSON."""
        data = [{"name": "test1"}, {"name": "test2"}]

        with patch("cua_cli.utils.output.console") as mock_console:
            print_json(data)

        mock_console.print_json.assert_called_once()

    def test_handles_nested_data(self):
        """Test printing nested data structures."""
        data = {
            "level1": {
                "level2": {
                    "value": 123,
                }
            }
        }

        with patch("cua_cli.utils.output.console") as mock_console:
            print_json(data)

        mock_console.print_json.assert_called_once()


class TestPrintError:
    """Tests for print_error function."""

    def test_prints_to_stderr(self):
        """Test that error is printed to stderr."""
        with patch("cua_cli.utils.output.error_console") as mock_console:
            print_error("Test error message")

        mock_console.print.assert_called_once()
        call_args = mock_console.print.call_args[0][0]
        assert "Error:" in call_args
        assert "Test error message" in call_args


class TestPrintSuccess:
    """Tests for print_success function."""

    def test_prints_success_message(self):
        """Test printing success message."""
        with patch("cua_cli.utils.output.console") as mock_console:
            print_success("Operation successful")

        mock_console.print.assert_called_once()
        call_args = mock_console.print.call_args[0][0]
        assert "Operation successful" in call_args


class TestPrintWarning:
    """Tests for print_warning function."""

    def test_prints_warning_message(self):
        """Test printing warning message."""
        with patch("cua_cli.utils.output.console") as mock_console:
            print_warning("This is a warning")

        mock_console.print.assert_called_once()
        call_args = mock_console.print.call_args[0][0]
        assert "Warning:" in call_args
        assert "This is a warning" in call_args


class TestPrintInfo:
    """Tests for print_info function."""

    def test_prints_info_message(self):
        """Test printing info message."""
        with patch("cua_cli.utils.output.console") as mock_console:
            print_info("Information message")

        mock_console.print.assert_called_once()
        call_args = mock_console.print.call_args[0][0]
        assert "Information message" in call_args
