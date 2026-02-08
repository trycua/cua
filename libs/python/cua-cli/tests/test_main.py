"""Tests for the main CLI entry point."""

import argparse
import sys
from io import StringIO
from unittest.mock import MagicMock, patch

import pytest
from cua_cli.main import create_parser, main


class TestCreateParser:
    """Tests for create_parser function."""

    def test_creates_parser(self):
        """Test that parser is created."""
        parser = create_parser()

        assert isinstance(parser, argparse.ArgumentParser)
        assert parser.prog == "cua"

    def test_has_version_flag(self):
        """Test that --version flag is present."""
        parser = create_parser()

        with pytest.raises(SystemExit) as exc_info:
            parser.parse_args(["--version"])

        assert exc_info.value.code == 0

    def test_has_auth_command(self):
        """Test that auth command is registered."""
        parser = create_parser()

        args = parser.parse_args(["auth", "login"])
        assert args.command == "auth"
        assert args.auth_command == "login"

    def test_has_sandbox_command(self):
        """Test that sandbox command is registered."""
        parser = create_parser()

        args = parser.parse_args(["sandbox", "list"])
        assert args.command == "sandbox"
        assert args.sandbox_command == "list"

    def test_has_sb_alias(self):
        """Test that sb alias works."""
        parser = create_parser()

        args = parser.parse_args(["sb", "list"])
        assert args.command == "sb"
        assert args.sandbox_command == "list"

    def test_has_image_command(self):
        """Test that image command is registered."""
        parser = create_parser()

        args = parser.parse_args(["image", "list"])
        assert args.command == "image"
        assert args.image_command == "list"

    def test_has_skills_command(self):
        """Test that skills command is registered."""
        parser = create_parser()

        args = parser.parse_args(["skills", "list"])
        assert args.command == "skills"
        assert args.skills_command == "list"

    def test_has_serve_mcp_command(self):
        """Test that serve-mcp command is registered."""
        parser = create_parser()

        args = parser.parse_args(["serve-mcp"])
        assert args.command == "serve-mcp"


class TestMain:
    """Tests for main function."""

    def test_no_args_shows_help(self, capsys):
        """Test that no arguments shows help."""
        with patch.object(sys, "argv", ["cua"]):
            result = main()

        assert result == 0
        captured = capsys.readouterr()
        assert "usage:" in captured.out.lower() or "cua" in captured.out

    def test_dispatch_to_auth(self):
        """Test dispatch to auth command."""
        with patch.object(sys, "argv", ["cua", "auth", "logout"]):
            with patch("cua_cli.commands.auth.execute", return_value=0) as mock_execute:
                result = main()

        mock_execute.assert_called_once()
        assert result == 0

    def test_dispatch_to_sandbox(self):
        """Test dispatch to sandbox command."""
        with patch.object(sys, "argv", ["cua", "sandbox", "list"]):
            with patch("cua_cli.commands.sandbox.execute", return_value=0) as mock_execute:
                result = main()

        mock_execute.assert_called_once()
        assert result == 0

    def test_dispatch_to_sb_alias(self):
        """Test dispatch to sb alias."""
        with patch.object(sys, "argv", ["cua", "sb", "list"]):
            with patch("cua_cli.commands.sandbox.execute", return_value=0) as mock_execute:
                result = main()

        mock_execute.assert_called_once()
        assert result == 0

    def test_dispatch_to_image(self):
        """Test dispatch to image command."""
        with patch.object(sys, "argv", ["cua", "image", "list"]):
            with patch("cua_cli.commands.image.execute", return_value=0) as mock_execute:
                result = main()

        mock_execute.assert_called_once()
        assert result == 0

    def test_dispatch_to_skills(self):
        """Test dispatch to skills command."""
        with patch.object(sys, "argv", ["cua", "skills", "list"]):
            with patch("cua_cli.commands.skills.execute", return_value=0) as mock_execute:
                result = main()

        mock_execute.assert_called_once()
        assert result == 0

    def test_dispatch_to_mcp(self):
        """Test dispatch to serve-mcp command."""
        with patch.object(sys, "argv", ["cua", "serve-mcp"]):
            with patch("cua_cli.commands.mcp.execute", return_value=0) as mock_execute:
                result = main()

        mock_execute.assert_called_once()
        assert result == 0

    def test_keyboard_interrupt_returns_130(self):
        """Test that KeyboardInterrupt returns exit code 130."""
        with patch.object(sys, "argv", ["cua", "sandbox", "list"]):
            with patch("cua_cli.commands.sandbox.execute", side_effect=KeyboardInterrupt):
                result = main()

        assert result == 130

    def test_exception_returns_1(self):
        """Test that exceptions return exit code 1."""
        with patch.object(sys, "argv", ["cua", "sandbox", "list"]):
            with patch("cua_cli.commands.sandbox.execute", side_effect=Exception("Test error")):
                result = main()

        assert result == 1

    def test_unknown_command_returns_1(self):
        """Test that unknown commands are handled."""
        # Mock parse_args to return an unknown command
        with patch.object(sys, "argv", ["cua", "unknown"]):
            # This should trigger SystemExit from argparse
            with pytest.raises(SystemExit):
                main()
