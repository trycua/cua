"""Tests for sandbox command module."""

import argparse
from unittest.mock import AsyncMock, MagicMock, patch

from cua_cli.commands import sandbox


class TestRegisterParser:
    """Tests for register_parser function."""

    def test_registers_sandbox_command(self):
        """Test that sandbox command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()

        sandbox.register_parser(subparsers)

        # Parse a sandbox command to verify it's registered
        args = parser.parse_args(["sandbox", "list"])
        assert args.sandbox_command == "list"

    def test_registers_sb_alias(self):
        """Test that sb alias is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()

        sandbox.register_parser(subparsers)

        # Parse using alias
        args = parser.parse_args(["sb", "list"])
        assert args.sandbox_command == "list"

    def test_list_command_has_json_flag(self):
        """Test that list command has --json flag."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(["sandbox", "list", "--json"])
        assert args.json is True

    def test_create_command_has_required_args(self):
        """Test that create command has required arguments."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(
            [
                "sandbox",
                "create",
                "--os",
                "linux",
                "--size",
                "medium",
                "--region",
                "north-america",
            ]
        )
        assert args.os == "linux"
        assert args.size == "medium"
        assert args.region == "north-america"


class TestExecute:
    """Tests for execute function."""

    def test_dispatch_to_list(self, args_namespace):
        """Test dispatch to list command."""
        args = args_namespace(
            command="sandbox", sandbox_command="list", json=False, show_passwords=False
        )

        with patch.object(sandbox, "cmd_list", return_value=0) as mock_cmd:
            result = sandbox.execute(args)

        mock_cmd.assert_called_once_with(args)
        assert result == 0

    def test_dispatch_to_list_alias(self, args_namespace):
        """Test dispatch to list command via ls alias."""
        args = args_namespace(command="sb", sandbox_command="ls", json=False, show_passwords=False)

        with patch.object(sandbox, "cmd_list", return_value=0) as mock_cmd:
            sandbox.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_unknown_command_returns_error(self, args_namespace):
        """Test that unknown command returns error."""
        args = args_namespace(command="sandbox", sandbox_command=None)

        with patch.object(sandbox, "print_error") as mock_error:
            result = sandbox.execute(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdList:
    """Tests for cmd_list function."""

    def test_list_sandboxes_success(self, args_namespace, sample_vm_list, mock_api_key):
        """Test listing sandboxes successfully."""
        args = args_namespace(json=False, show_passwords=False)

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=sample_vm_list)

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_table") as mock_print:
                result = sandbox.cmd_list(args)

        assert result == 0
        mock_print.assert_called_once()

    def test_list_sandboxes_empty(self, args_namespace, mock_api_key):
        """Test listing when no sandboxes exist."""
        args = args_namespace(json=False, show_passwords=False)

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=[])

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_info") as mock_print:
                result = sandbox.cmd_list(args)

        assert result == 0
        mock_print.assert_called_with("No sandboxes found.")

    def test_list_sandboxes_json_output(self, args_namespace, sample_vm_list, mock_api_key):
        """Test JSON output format."""
        args = args_namespace(json=True, show_passwords=False)

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=sample_vm_list)

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_json") as mock_print:
                result = sandbox.cmd_list(args)

        assert result == 0
        mock_print.assert_called_once_with(sample_vm_list)


class TestCmdCreate:
    """Tests for cmd_create function."""

    def test_create_sandbox_success(self, args_namespace, mock_api_key):
        """Test creating a sandbox successfully."""
        args = args_namespace(
            os="linux",
            size="medium",
            region="north-america",
            json=False,
        )

        # Mock the API response (status 202 = provisioning)
        async def mock_api_request(*args, **kwargs):
            return (
                202,
                {
                    "status": "provisioning",
                    "name": "new-sandbox",
                    "host": "sandbox.example.com",
                },
            )

        with patch.object(sandbox, "_api_request", side_effect=mock_api_request):
            with patch.object(sandbox, "print_info"):
                result = sandbox.cmd_create(args)

        assert result == 0

    def test_create_sandbox_error(self, args_namespace, mock_api_key):
        """Test handling create error."""
        args = args_namespace(
            os="linux",
            size="medium",
            region="north-america",
            json=False,
        )

        # Mock the API response (status 500 = error)
        async def mock_api_request(*args, **kwargs):
            return (500, "Quota exceeded")

        with patch.object(sandbox, "_api_request", side_effect=mock_api_request):
            with patch.object(sandbox, "print_error") as mock_error:
                result = sandbox.cmd_create(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdGet:
    """Tests for cmd_get function."""

    def test_get_sandbox_success(self, args_namespace, sample_vm, mock_api_key):
        """Test getting sandbox details."""
        args = args_namespace(
            name="test-sandbox-1",
            json=False,
            show_passwords=False,
            show_vnc_url=False,
        )

        vm_dict = {
            "name": sample_vm.name,
            "status": sample_vm.status,
            "os_type": sample_vm.os_type,
            "host": "sandbox.example.com",
        }

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=[vm_dict])
        mock_provider.get_vm = AsyncMock(return_value={"status": "running"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_info"):
                result = sandbox.cmd_get(args)

        assert result == 0

    def test_get_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test getting nonexistent sandbox."""
        args = args_namespace(
            name="nonexistent",
            json=False,
            show_passwords=False,
            show_vnc_url=False,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=[])
        mock_provider.get_vm = AsyncMock(return_value={"status": "not_found"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_error") as mock_error:
                result = sandbox.cmd_get(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdStart:
    """Tests for cmd_start function."""

    def test_start_sandbox_success(self, args_namespace, mock_api_key):
        """Test starting a sandbox."""
        args = args_namespace(name="test-sandbox")

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.run_vm = AsyncMock(return_value={"status": "starting"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_start(args)

        assert result == 0

    def test_start_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test starting nonexistent sandbox."""
        args = args_namespace(name="nonexistent")

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.run_vm = AsyncMock(return_value={"status": "not_found"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_error"):
                result = sandbox.cmd_start(args)

        assert result == 1


class TestCmdStop:
    """Tests for cmd_stop function."""

    def test_stop_sandbox_success(self, args_namespace, mock_api_key):
        """Test stopping a sandbox."""
        args = args_namespace(name="test-sandbox")

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.stop_vm = AsyncMock(return_value={"status": "stopping"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_stop(args)

        assert result == 0


class TestCmdRestart:
    """Tests for cmd_restart function."""

    def test_restart_sandbox_success(self, args_namespace, mock_api_key):
        """Test restarting a sandbox."""
        args = args_namespace(name="test-sandbox")

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.restart_vm = AsyncMock(return_value={"status": "restarting"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_restart(args)

        assert result == 0


class TestCmdSuspend:
    """Tests for cmd_suspend function."""

    def test_suspend_sandbox_success(self, args_namespace, mock_api_key):
        """Test suspending a sandbox."""
        args = args_namespace(name="test-sandbox")

        # Mock the API response (status 202 = suspending)
        async def mock_api_request(*args, **kwargs):
            return (202, {"status": "suspending"})

        with patch.object(sandbox, "_api_request", side_effect=mock_api_request):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_suspend(args)

        assert result == 0

    def test_suspend_unsupported(self, args_namespace, mock_api_key):
        """Test suspend on unsupported sandbox."""
        args = args_namespace(name="test-sandbox")

        # Mock the API response (status 400 = unsupported)
        async def mock_api_request(*args, **kwargs):
            return (400, "Suspend not supported for Windows")

        with patch.object(sandbox, "_api_request", side_effect=mock_api_request):
            with patch.object(sandbox, "print_error"):
                result = sandbox.cmd_suspend(args)

        assert result == 1


class TestCmdDelete:
    """Tests for cmd_delete function."""

    def test_delete_sandbox_success(self, args_namespace, mock_api_key):
        """Test deleting a sandbox."""
        args = args_namespace(name="test-sandbox")

        # Mock the API response (status 202 = deleting)
        async def mock_api_request(*args, **kwargs):
            return (202, {"status": "deleting"})

        with patch.object(sandbox, "_api_request", side_effect=mock_api_request):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_delete(args)

        assert result == 0


class TestCmdVnc:
    """Tests for cmd_vnc function."""

    def test_vnc_opens_browser(self, args_namespace, mock_api_key, mock_webbrowser):
        """Test VNC opens browser with correct URL."""
        args = args_namespace(name="test-sandbox")

        vm_info = {
            "name": "test-sandbox",
            "vnc_url": "https://vnc.example.com/test",
        }

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=[vm_info])

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_info"):
                result = sandbox.cmd_vnc(args)

        assert result == 0
        mock_webbrowser.assert_called_once_with("https://vnc.example.com/test")

    def test_vnc_constructs_url_from_host(self, args_namespace, mock_api_key, mock_webbrowser):
        """Test VNC constructs URL when vnc_url not provided."""
        args = args_namespace(name="test-sandbox")

        vm_info = {
            "name": "test-sandbox",
            "host": "sandbox.example.com",
            "password": "secret123",
        }

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=[vm_info])

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_info"):
                result = sandbox.cmd_vnc(args)

        assert result == 0
        mock_webbrowser.assert_called_once()
        # Check URL contains host and encoded password
        call_url = mock_webbrowser.call_args[0][0]
        assert "sandbox.example.com" in call_url
        assert "secret123" in call_url

    def test_vnc_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test VNC with nonexistent sandbox."""
        args = args_namespace(name="nonexistent")

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.list_vms = AsyncMock(return_value=[])

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_error"):
                result = sandbox.cmd_vnc(args)

        assert result == 1


class TestCmdShell:
    """Tests for cmd_shell function."""

    def test_shell_parser_registration(self):
        """Test that shell command is registered with correct arguments."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        # Test basic shell command
        args = parser.parse_args(["sb", "shell", "my-sandbox"])
        assert args.name == "my-sandbox"
        assert args.sandbox_command == "shell"

    def test_shell_with_command(self):
        """Test shell command with a command to execute."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(["sb", "shell", "my-sandbox", "ls", "-la"])
        assert args.name == "my-sandbox"
        assert args.shell_command == ["ls", "-la"]

    def test_shell_with_cols_rows(self):
        """Test shell command with terminal size options.

        Options must come before name due to argparse REMAINDER behavior.
        Usage: cua sb shell --cols 120 --rows 40 mybox [command...]
        """
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(
            ["sb", "shell", "--cols", "120", "--rows", "40", "my-sandbox", "ls"]
        )
        assert args.cols == 120
        assert args.rows == 40
        assert args.name == "my-sandbox"
        assert args.shell_command == ["ls"]

    def test_shell_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test shell with nonexistent sandbox."""
        args = args_namespace(
            name="nonexistent",
            shell_command=[],
            cols=None,
            rows=None,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.get_vm = AsyncMock(return_value={"status": "not_found"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_error") as mock_error:
                with patch("sys.stdin") as mock_stdin:
                    mock_stdin.isatty.return_value = False
                    result = sandbox.cmd_shell(args)

        assert result == 1
        mock_error.assert_called()

    def test_shell_no_api_url(self, args_namespace, mock_api_key):
        """Test shell when sandbox has no API URL."""
        args = args_namespace(
            name="test-sandbox",
            shell_command=["echo", "hello"],
            cols=None,
            rows=None,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.get_vm = AsyncMock(return_value={"status": "stopped", "api_url": None})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_error") as mock_error:
                with patch("sys.stdin") as mock_stdin:
                    mock_stdin.isatty.return_value = False
                    result = sandbox.cmd_shell(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdExec:
    """Tests for cmd_exec function."""

    def test_exec_parser_registration(self):
        """Test that exec command is registered with correct arguments."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(["sb", "exec", "my-sandbox", "echo", "hello"])
        assert args.name == "my-sandbox"
        assert args.sandbox_command == "exec"
        assert args.exec_command == ["echo", "hello"]

    def test_exec_with_json_flag(self):
        """Test exec command with --json flag.

        --json must come before name due to argparse REMAINDER behavior.
        Usage: cua sb exec --json mybox <command...>
        """
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(["sb", "exec", "--json", "my-sandbox", "echo", "hello"])
        assert args.json is True
        assert args.name == "my-sandbox"
        assert args.exec_command == ["echo", "hello"]

    def test_exec_no_command_error(self, args_namespace, mock_api_key):
        """Test exec with no command returns error."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=[],
            json=False,
        )

        with patch.object(sandbox, "print_error") as mock_error:
            result = sandbox.cmd_exec(args)

        assert result == 1
        mock_error.assert_called_with("No command provided")

    def test_exec_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test exec with nonexistent sandbox."""
        args = args_namespace(
            name="nonexistent",
            exec_command=["echo", "hello"],
            json=False,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.get_vm = AsyncMock(return_value={"status": "not_found"})

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "print_error") as mock_error:
                result = sandbox.cmd_exec(args)

        assert result == 1
        mock_error.assert_called()

    def test_exec_success(self, args_namespace, mock_api_key, capsys):
        """Test successful command execution."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["echo", "hello"],
            json=False,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.get_vm = AsyncMock(
            return_value={"status": "running", "api_url": "https://sandbox.example.com:8443"}
        )

        async def mock_exec(*a, **kw):
            return {"success": True, "stdout": "hello\n", "stderr": "", "returncode": 0}

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "_exec_noninteractive", side_effect=mock_exec):
                result = sandbox.cmd_exec(args)

        assert result == 0
        captured = capsys.readouterr()
        assert "hello" in captured.out

    def test_exec_json_output(self, args_namespace, mock_api_key):
        """Test exec with JSON output."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["echo", "hello"],
            json=True,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.get_vm = AsyncMock(
            return_value={"status": "running", "api_url": "https://sandbox.example.com:8443"}
        )

        async def mock_exec(*a, **kw):
            return {"success": True, "stdout": "hello\n", "stderr": "", "returncode": 0}

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "_exec_noninteractive", side_effect=mock_exec):
                with patch.object(sandbox, "print_json") as mock_json:
                    result = sandbox.cmd_exec(args)

        assert result == 0
        mock_json.assert_called_once()

    def test_exec_command_failure(self, args_namespace, mock_api_key, capsys):
        """Test exec when command returns non-zero exit code."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["false"],
            json=False,
        )

        mock_provider = MagicMock()
        mock_provider.__aenter__ = AsyncMock(return_value=mock_provider)
        mock_provider.__aexit__ = AsyncMock(return_value=None)
        mock_provider.get_vm = AsyncMock(
            return_value={"status": "running", "api_url": "https://sandbox.example.com:8443"}
        )

        async def mock_exec(*a, **kw):
            return {"success": True, "stdout": "", "stderr": "error", "returncode": 1}

        with patch.object(sandbox, "_get_provider", return_value=mock_provider):
            with patch.object(sandbox, "_exec_noninteractive", side_effect=mock_exec):
                result = sandbox.cmd_exec(args)

        assert result == 1
