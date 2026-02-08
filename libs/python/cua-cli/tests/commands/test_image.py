"""Tests for image command module."""

import argparse
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from cua_cli.commands import image


class TestRegisterParser:
    """Tests for register_parser function."""

    def test_registers_image_command(self):
        """Test that image command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()

        image.register_parser(subparsers)

        args = parser.parse_args(["image", "list"])
        assert args.image_command == "list"

    def test_registers_img_alias(self):
        """Test that img alias is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        image.register_parser(subparsers)

        args = parser.parse_args(["img", "list"])
        assert args.image_command == "list"

    def test_list_has_json_flag(self):
        """Test that list command has --json flag."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        image.register_parser(subparsers)

        args = parser.parse_args(["image", "list", "--json"])
        assert args.json is True

    def test_list_has_local_flag(self):
        """Test that list command has --local flag."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        image.register_parser(subparsers)

        args = parser.parse_args(["image", "list", "--local"])
        assert args.local is True

    def test_push_command(self):
        """Test that push command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        image.register_parser(subparsers)

        args = parser.parse_args(["image", "push", "my-image"])
        assert args.image_command == "push"
        assert args.name == "my-image"

    def test_pull_command(self):
        """Test that pull command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        image.register_parser(subparsers)

        args = parser.parse_args(["image", "pull", "my-image"])
        assert args.image_command == "pull"
        assert args.name == "my-image"

    def test_delete_command(self):
        """Test that delete command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        image.register_parser(subparsers)

        args = parser.parse_args(["image", "delete", "my-image", "--force"])
        assert args.image_command == "delete"
        assert args.name == "my-image"
        assert args.force is True


class TestExecute:
    """Tests for execute function."""

    def test_dispatch_to_list(self, args_namespace):
        """Test dispatch to list command."""
        args = args_namespace(
            command="image",
            image_command="list",
            json=False,
            local=False,
            cloud=False,
        )

        with patch.object(image, "cmd_list", return_value=0) as mock_cmd:
            result = image.execute(args)

        mock_cmd.assert_called_once_with(args)
        assert result == 0

    def test_dispatch_to_push(self, args_namespace):
        """Test dispatch to push command."""
        args = args_namespace(command="image", image_command="push", name="my-image")

        with patch.object(image, "cmd_push", return_value=0) as mock_cmd:
            result = image.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_dispatch_to_pull(self, args_namespace):
        """Test dispatch to pull command."""
        args = args_namespace(command="image", image_command="pull", name="my-image")

        with patch.object(image, "cmd_pull", return_value=0) as mock_cmd:
            result = image.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_dispatch_to_delete(self, args_namespace):
        """Test dispatch to delete command."""
        args = args_namespace(command="image", image_command="delete", name="my-image")

        with patch.object(image, "cmd_delete", return_value=0) as mock_cmd:
            result = image.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_unknown_command_returns_error(self, args_namespace):
        """Test that unknown command returns error."""
        args = args_namespace(command="image", image_command=None)

        with patch.object(image, "print_error"):
            result = image.execute(args)

        assert result == 1


class TestCmdList:
    """Tests for cmd_list function."""

    def test_list_cloud_images(self, args_namespace, sample_cloud_images, mock_api_key):
        """Test listing cloud images."""
        args = args_namespace(json=False, local=False, cloud=True)

        with patch.object(image, "run_async") as mock_run:
            mock_run.return_value = [
                {
                    "name": "ubuntu-22.04",
                    "type": "qcow2",
                    "tag": "latest",
                    "size": "5.0 GB",
                    "status": "ready",
                    "created": "2024-01-10",
                    "source": "cloud",
                }
            ]
            with patch.object(image, "print_table") as mock_print:
                result = image.cmd_list(args)

        assert result == 0
        mock_print.assert_called_once()

    def test_list_local_images(self, args_namespace, tmp_path, monkeypatch):
        """Test listing local images delegates to local_image."""
        args = args_namespace(json=False, local=True, cloud=False)

        with patch("cua_cli.commands.local_image.cmd_local_list", return_value=0) as mock_local:
            result = image.cmd_list(args)

        assert result == 0
        mock_local.assert_called_once_with(args)

    def test_list_empty(self, args_namespace, tmp_path, monkeypatch):
        """Test listing when no local images exist."""
        args = args_namespace(json=False, local=True, cloud=False)

        with patch("cua_cli.commands.local_image.cmd_local_list", return_value=0) as mock_local:
            result = image.cmd_list(args)

        assert result == 0
        mock_local.assert_called_once()

    def test_list_json_output(self, args_namespace, mock_api_key):
        """Test JSON output format."""
        args = args_namespace(json=True, local=False, cloud=True)

        with patch.object(image, "run_async") as mock_run:
            mock_run.return_value = [{"name": "test", "source": "cloud"}]
            with patch.object(image, "print_json") as mock_print:
                result = image.cmd_list(args)

        assert result == 0
        mock_print.assert_called_once()


class TestCmdPush:
    """Tests for cmd_push function."""

    def test_push_success(self, args_namespace, temp_image_file, mock_api_key):
        """Test successful image push."""
        args = args_namespace(
            name="test-image",
            tag="latest",
            type="qcow2",
            file=str(temp_image_file),
        )

        with patch.object(image, "run_async") as mock_run:
            mock_run.return_value = 0
            result = image.cmd_push(args)

        assert result == 0

    def test_push_file_not_found(self, args_namespace, mock_api_key):
        """Test push with nonexistent file."""
        args = args_namespace(
            name="test-image",
            tag="latest",
            type="qcow2",
            file="/nonexistent/path",
        )

        with patch.object(image, "print_error") as mock_error:
            result = image.cmd_push(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdPull:
    """Tests for cmd_pull function."""

    def test_pull_success(self, args_namespace, tmp_path, mock_api_key):
        """Test successful image pull."""
        args = args_namespace(
            name="test-image",
            tag="latest",
            output=str(tmp_path / "output.qcow2"),
        )

        with patch.object(image, "run_async") as mock_run:
            mock_run.return_value = 0
            result = image.cmd_pull(args)

        assert result == 0


class TestCmdDelete:
    """Tests for cmd_delete function."""

    def test_delete_with_force(self, args_namespace, mock_api_key):
        """Test delete with --force flag."""
        args = args_namespace(name="test-image", tag="latest", force=True, local=False)

        with patch.object(image, "run_async") as mock_run:
            mock_run.return_value = 0
            result = image.cmd_delete(args)

        assert result == 0

    def test_delete_without_force_prompts(self, args_namespace, mock_api_key):
        """Test delete without --force shows prompt."""
        args = args_namespace(name="test-image", tag="latest", force=False, local=False)

        with patch.object(image, "print_info") as mock_print:
            result = image.cmd_delete(args)

        assert result == 1
        mock_print.assert_called()

    def test_delete_local_delegates(self, args_namespace):
        """Test delete with --local delegates to local_image."""
        args = args_namespace(name="test-image", tag="latest", force=True, local=True)

        with patch("cua_cli.commands.local_image.cmd_local_delete", return_value=0) as mock_local:
            result = image.cmd_delete(args)

        assert result == 0
        mock_local.assert_called_once_with(args)


class TestHelperFunctions:
    """Tests for helper functions."""

    def test_format_date_valid(self):
        """Test formatting a valid ISO date."""
        result = image._format_date("2024-01-15T10:30:00Z")
        assert result == "2024-01-15"

    def test_format_date_empty(self):
        """Test formatting empty string."""
        result = image._format_date("")
        assert result == "-"

    def test_format_date_invalid(self):
        """Test formatting invalid date."""
        result = image._format_date("not-a-date")
        assert result == "not-a-date"  # Falls back to first 10 chars
