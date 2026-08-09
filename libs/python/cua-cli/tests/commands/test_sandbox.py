"""Tests for sandbox command module."""

import argparse
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, call, patch

from cua_cli.commands import sandbox


class TestCloudAuth:
    """Tests for cloud authentication mode selection."""

    async def test_cloud_auth_kwargs_omits_legacy_key_for_wif(self, monkeypatch):
        monkeypatch.setenv("FLEETS_TOKEN", "github-token")

        with patch.object(sandbox, "get_access_token", new_callable=AsyncMock) as mock_token:
            assert await sandbox._cloud_auth_kwargs() == {}

        mock_token.assert_not_awaited()

    async def test_cloud_auth_kwargs_uses_interactive_token_without_wif(self, monkeypatch):
        monkeypatch.delenv("FLEETS_TOKEN", raising=False)

        with patch.object(
            sandbox, "get_access_token", new_callable=AsyncMock, return_value="interactive"
        ):
            assert await sandbox._cloud_auth_kwargs() == {"api_key": "interactive"}


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

    def test_launch_command_has_required_args(self):
        """Test that launch accepts its image and optional settings."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(
            [
                "sandbox",
                "launch",
                "ubuntu:24.04",
                "--name",
                "test-sandbox",
                "--region",
                "north-america",
            ]
        )
        assert args.image == "ubuntu:24.04"
        assert args.name == "test-sandbox"
        assert args.region == "north-america"


class TestExecute:
    """Tests for execute function."""

    def test_dispatch_to_list(self, args_namespace):
        """Test dispatch to list command."""
        args = args_namespace(
            command="sandbox", sandbox_command="list", json=False, show_passwords=False
        )

        with patch.object(sandbox, "cmd_ls", return_value=0) as mock_cmd:
            result = sandbox.execute(args)

        mock_cmd.assert_called_once_with(args)
        assert result == 0

    def test_dispatch_to_list_alias(self, args_namespace):
        """Test dispatch to list command via ls alias."""
        args = args_namespace(command="sb", sandbox_command="ls", json=False, show_passwords=False)

        with patch.object(sandbox, "cmd_ls", return_value=0) as mock_cmd:
            sandbox.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_unknown_command_returns_error(self, args_namespace):
        """Test that unknown command returns error."""
        args = args_namespace(command="sandbox", sandbox_command=None)

        with patch.object(sandbox, "print_error") as mock_error:
            result = sandbox.execute(args)

        assert result == 1
        mock_error.assert_called()


class TestCmdLs:
    """Tests for cmd_ls function."""

    def test_ls_cloud_sandboxes(self, args_namespace, mock_api_key):
        """Test the default list path uses the cloud Fleet SDK."""
        args = args_namespace(json=False, local=False, all=False)
        cloud_sandboxes = [SimpleNamespace(name="cloudbox", status="running")]
        mock_list = AsyncMock(return_value=cloud_sandboxes)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.list = mock_list

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_table") as mock_table:
                result = sandbox.cmd_ls(args)

        assert result == 0
        mock_list.assert_awaited_once_with(local=False, api_key="test-access-token")
        mock_table.assert_called_once_with(
            [{"name": "cloudbox", "status": "running", "source": "cloud"}],
            [("name", "NAME"), ("status", "STATUS"), ("source", "SOURCE")],
        )

    def test_ls_local_sandboxes(self, args_namespace):
        """Test --local lists only local sandboxes without cloud auth."""
        args = args_namespace(json=False, local=True, all=False)
        local_sandboxes = [SimpleNamespace(name="localbox", status="running", source="local")]
        mock_list = AsyncMock(return_value=local_sandboxes)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.list = mock_list

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "get_access_token", new_callable=AsyncMock) as mock_token:
                with patch.object(sandbox, "print_table"):
                    result = sandbox.cmd_ls(args)

        assert result == 0
        mock_list.assert_awaited_once_with(local=True)
        mock_token.assert_not_awaited()

    def test_ls_all_json(self, args_namespace, mock_api_key):
        """Test --all combines local and cloud results in JSON output."""
        args = args_namespace(json=True, local=False, all=True)
        local_sandboxes = [SimpleNamespace(name="localbox", status="running", source="local")]
        cloud_sandboxes = [SimpleNamespace(name="cloudbox", status="pending")]
        mock_list = AsyncMock(side_effect=[local_sandboxes, cloud_sandboxes])
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.list = mock_list

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_json") as mock_json:
                result = sandbox.cmd_ls(args)

        assert result == 0
        assert mock_list.await_args_list == [
            call(local=True),
            call(local=False, api_key="test-access-token"),
        ]
        mock_json.assert_called_once_with(
            [
                {"name": "localbox", "status": "running", "source": "local"},
                {"name": "cloudbox", "status": "pending", "source": "cloud"},
            ]
        )


class TestCmdLaunch:
    """Tests for cmd_launch function."""

    def test_launch_cloud_with_options(self, args_namespace, mock_api_key):
        """Test cloud launch forwards resource and image options to Fleet."""
        args = args_namespace(
            image="ubuntu:24.04",
            local=False,
            name="new-sandbox",
            vm=True,
            cpu=4,
            memory="8GB",
            disk="50GB",
            region="us-east",
            json=True,
        )
        image = object()
        created = MagicMock()
        created.name = "new-sandbox"
        created.disconnect = AsyncMock()
        mock_create = AsyncMock(return_value=created)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.create = mock_create

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "_parse_image", return_value=image) as mock_parse:
                with patch.object(sandbox, "print_json") as mock_json:
                    result = sandbox.cmd_launch(args)

        assert result == 0
        mock_parse.assert_called_once_with("ubuntu:24.04", vm=True)
        mock_create.assert_awaited_once_with(
            image,
            name="new-sandbox",
            region="us-east",
            cpu=4,
            memory_mb=8192,
            disk_gb=50,
            api_key="test-access-token",
        )
        created.disconnect.assert_awaited_once_with()
        mock_json.assert_called_once_with({"name": "new-sandbox", "status": "ready"})

    def test_launch_local(self, args_namespace):
        """Test --local launches through Fleet without cloud authentication."""
        args = args_namespace(
            image="macos:sequoia",
            local=True,
            name="local-sandbox",
            vm=False,
            cpu=None,
            memory=None,
            disk=None,
            region=None,
            json=False,
        )
        image = object()
        created = MagicMock()
        created.name = "local-sandbox"
        created.disconnect = AsyncMock()
        mock_create = AsyncMock(return_value=created)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.create = mock_create

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "_parse_image", return_value=image):
                with patch.object(
                    sandbox, "get_access_token", new_callable=AsyncMock
                ) as mock_token:
                    with patch.object(sandbox, "print_success") as mock_success:
                        result = sandbox.cmd_launch(args)

        assert result == 0
        mock_create.assert_awaited_once_with(image, local=True, name="local-sandbox")
        mock_token.assert_not_awaited()
        created.disconnect.assert_awaited_once_with()
        mock_success.assert_called_once_with("Sandbox 'local-sandbox' is ready")


    def test_launch_with_wif_omits_legacy_key_and_interactive_token(self, args_namespace, monkeypatch):
        """Test workload identity selects the Fleet SDK transport."""
        monkeypatch.setenv("FLEETS_TOKEN", "github-token")
        args = args_namespace(
            image="ubuntu:24.04", local=False, name="fleet-sandbox", vm=False, cpu=None,
            memory=None, disk=None, region=None, json=False,
        )
        image = object()
        created = MagicMock(name="created")
        created.name = "fleet-sandbox"
        created.disconnect = AsyncMock()
        mock_create = AsyncMock(return_value=created)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.create = mock_create

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "_parse_image", return_value=image):
                with patch.object(
                    sandbox, "get_access_token", new_callable=AsyncMock
                ) as mock_token:
                    with patch.object(sandbox, "print_success"):
                        result = sandbox.cmd_launch(args)

        assert result == 0
        mock_token.assert_not_awaited()
        mock_create.assert_awaited_once_with(image, name="fleet-sandbox")
        created.disconnect.assert_awaited_once_with()

class TestCmdInfo:
    """Tests for cmd_info function."""

    def test_info_cloud_json(self, args_namespace, mock_api_key):
        """Test cloud info uses Fleet and renders all available JSON fields."""
        args = args_namespace(name="cloudbox", local=False, json=True)
        info = SimpleNamespace(
            name="cloudbox",
            status="running",
            os_type="linux",
            host="cloudbox.example.com",
            region="us-east",
            created_at="2026-08-06T00:00:00Z",
            cpu=4,
            memory_mb=8192,
            disk_gb=50,
        )
        mock_get_info = AsyncMock(return_value=info)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.get_info = mock_get_info

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_json") as mock_json:
                result = sandbox.cmd_info(args)

        assert result == 0
        mock_get_info.assert_awaited_once_with("cloudbox", local=False, api_key="test-access-token")
        mock_json.assert_called_once_with(
            {
                "name": "cloudbox",
                "status": "running",
                "os_type": "linux",
                "host": "cloudbox.example.com",
                "region": "us-east",
                "created_at": "2026-08-06T00:00:00Z",
                "cpu": 4,
                "memory_mb": 8192,
                "disk_gb": 50,
            }
        )

    def test_info_local(self, args_namespace):
        """Test --local gets sandbox info without cloud authentication."""
        args = args_namespace(name="localbox", local=True, json=False)
        info = SimpleNamespace(
            name="localbox",
            status="running",
            os_type="macos",
            host=None,
            region=None,
            created_at=None,
        )
        mock_get_info = AsyncMock(return_value=info)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.get_info = mock_get_info

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "get_access_token", new_callable=AsyncMock) as mock_token:
                with patch.object(sandbox, "print_info"):
                    result = sandbox.cmd_info(args)

        assert result == 0
        mock_get_info.assert_awaited_once_with("localbox", local=True)
        mock_token.assert_not_awaited()


class TestCmdRestart:
    """Tests for cmd_restart function."""

    def test_restart_sandbox_success(self, args_namespace, mock_api_key):
        """Test restarting a sandbox."""
        args = args_namespace(name="test-sandbox")

        mock_restart = AsyncMock()
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.restart = mock_restart

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_restart(args)

        assert result == 0
        mock_restart.assert_awaited_once_with(
            "test-sandbox", local=False, api_key="test-access-token"
        )


class TestCmdSuspend:
    """Tests for cmd_suspend function."""

    def test_suspend_sandbox_success(self, args_namespace, mock_api_key):
        """Test suspending a sandbox."""
        args = args_namespace(name="test-sandbox")

        mock_suspend = AsyncMock()
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.suspend = mock_suspend

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_success"):
                result = sandbox.cmd_suspend(args)

        assert result == 0
        mock_suspend.assert_awaited_once_with(
            "test-sandbox", local=False, api_key="test-access-token"
        )

    def test_suspend_unsupported(self, args_namespace, mock_api_key):
        """Test suspend on unsupported sandbox."""
        args = args_namespace(name="test-sandbox")

        mock_suspend = AsyncMock(side_effect=RuntimeError("Suspend not supported for Windows"))
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.suspend = mock_suspend

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_error"):
                result = sandbox.cmd_suspend(args)

        assert result == 1
        mock_suspend.assert_awaited_once_with(
            "test-sandbox", local=False, api_key="test-access-token"
        )


class TestCmdDelete:
    """Tests for cmd_delete function."""

    def test_delete_force_cloud(self, args_namespace, mock_api_key):
        """Test --force deletes a cloud sandbox without prompting."""
        args = args_namespace(name="cloudbox", local=False, force=True)
        mock_delete = AsyncMock()
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.delete = mock_delete

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch("builtins.input") as mock_input:
                with patch.object(sandbox, "print_success") as mock_success:
                    result = sandbox.cmd_delete(args)

        assert result == 0
        mock_input.assert_not_called()
        mock_delete.assert_awaited_once_with("cloudbox", local=False, api_key="test-access-token")
        mock_success.assert_called_once_with("Sandbox 'cloudbox' is being deleted.")

    def test_delete_confirmation_aborts(self, args_namespace):
        """Test declining interactive confirmation does not call Fleet."""
        args = args_namespace(name="cloudbox", local=False, force=False)
        mock_delete = AsyncMock()
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.delete = mock_delete

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch("sys.stdin") as mock_stdin:
                mock_stdin.isatty.return_value = True
                with patch("builtins.input", return_value="n"):
                    with patch.object(sandbox, "print_info") as mock_info:
                        result = sandbox.cmd_delete(args)

        assert result == 0
        mock_delete.assert_not_awaited()
        mock_info.assert_called_once_with("Aborted.")

    def test_delete_confirmed_local(self, args_namespace):
        """Test confirming a local delete avoids cloud authentication."""
        args = args_namespace(name="localbox", local=True, force=False)
        mock_delete = AsyncMock()
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.delete = mock_delete

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch("sys.stdin") as mock_stdin:
                mock_stdin.isatty.return_value = True
                with patch("builtins.input", return_value="yes"):
                    with patch.object(
                        sandbox, "get_access_token", new_callable=AsyncMock
                    ) as mock_token:
                        with patch.object(sandbox, "print_success"):
                            result = sandbox.cmd_delete(args)

        assert result == 0
        mock_delete.assert_awaited_once_with("localbox", local=True)
        mock_token.assert_not_awaited()


    def test_delete_with_wif_omits_legacy_key_and_interactive_token(
        self, args_namespace, monkeypatch
    ):
        """Test workload identity releases the Fleet claim without an API key."""
        monkeypatch.setenv("FLEETS_TOKEN", "github-token")
        args = args_namespace(name="fleet-sandbox", local=False, force=True)
        mock_delete = AsyncMock()
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.delete = mock_delete

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(
                sandbox, "get_access_token", new_callable=AsyncMock
            ) as mock_token:
                with patch.object(sandbox, "print_success"):
                    result = sandbox.cmd_delete(args)

        assert result == 0
        mock_token.assert_not_awaited()
        mock_delete.assert_awaited_once_with("fleet-sandbox", local=False)

class TestCmdVnc:
    """Tests for cmd_vnc function."""

    def test_vnc_opens_browser(self, args_namespace, mock_api_key, mock_webbrowser):
        """Test VNC opens browser with correct URL."""
        args = args_namespace(name="test-sandbox")

        mock_sandbox = MagicMock()
        mock_sandbox.get_display_url = AsyncMock(
            return_value="https://vnc.example.com/test"
        )
        mock_sandbox.disconnect = AsyncMock()
        mock_connect = AsyncMock(return_value=mock_sandbox)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.connect = mock_connect

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_info"):
                result = sandbox.cmd_vnc(args)

        assert result == 0
        mock_connect.assert_awaited_once_with(
            "test-sandbox", local=False, api_key="test-access-token"
        )
        mock_sandbox.get_display_url.assert_awaited_once_with(share=True)
        mock_sandbox.disconnect.assert_awaited_once_with()
        mock_webbrowser.assert_called_once_with("https://vnc.example.com/test")

    def test_vnc_constructs_url_from_host(self, args_namespace, mock_api_key, mock_webbrowser):
        """Test VNC opens the display URL returned by the sandbox."""
        args = args_namespace(name="test-sandbox")
        mock_sandbox = MagicMock()
        mock_sandbox.get_display_url = AsyncMock(
            return_value="https://sandbox.example.com/vnc?password=secret123"
        )
        mock_sandbox.disconnect = AsyncMock()
        mock_connect = AsyncMock(return_value=mock_sandbox)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.connect = mock_connect

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_info"):
                result = sandbox.cmd_vnc(args)

        assert result == 0
        mock_connect.assert_awaited_once_with(
            "test-sandbox", local=False, api_key="test-access-token"
        )
        mock_sandbox.get_display_url.assert_awaited_once_with(share=True)
        mock_sandbox.disconnect.assert_awaited_once_with()
        mock_webbrowser.assert_called_once_with(
            "https://sandbox.example.com/vnc?password=secret123"
        )

    def test_vnc_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test VNC with nonexistent sandbox."""
        args = args_namespace(name="nonexistent")

        mock_connect = AsyncMock(side_effect=ValueError("Sandbox 'nonexistent' not found"))
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.connect = mock_connect

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "print_error"):
                result = sandbox.cmd_vnc(args)

        assert result == 1
        mock_connect.assert_awaited_once_with(
            "nonexistent", local=False, api_key="test-access-token"
        )


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

        with patch.object(
            sandbox,
            "_get_sandbox_api_url",
            new_callable=AsyncMock,
            side_effect=ValueError("Sandbox 'nonexistent' not found"),
        ) as mock_api_url:
            with patch.object(sandbox, "print_error") as mock_error:
                with patch("sys.stdin") as mock_stdin:
                    mock_stdin.isatty.return_value = False
                    result = sandbox.cmd_shell(args)

        assert result == 1
        mock_api_url.assert_awaited_once_with("nonexistent", False)
        mock_error.assert_called()

    def test_shell_no_api_url(self, args_namespace, mock_api_key):
        """Test shell when sandbox has no API URL."""
        args = args_namespace(
            name="test-sandbox",
            shell_command=["echo", "hello"],
            cols=None,
            rows=None,
        )

        with patch.object(
            sandbox,
            "_get_sandbox_api_url",
            new_callable=AsyncMock,
            side_effect=ValueError(
                "Sandbox 'test-sandbox' has no API URL (is it running?)"
            ),
        ) as mock_api_url:
            with patch.object(sandbox, "print_error") as mock_error:
                with patch("sys.stdin") as mock_stdin:
                    mock_stdin.isatty.return_value = False
                    result = sandbox.cmd_shell(args)

        assert result == 1
        mock_api_url.assert_awaited_once_with("test-sandbox", False)
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
        """Test exec command with --json before the remainder arguments."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        sandbox.register_parser(subparsers)

        args = parser.parse_args(["sb", "exec", "--json", "my-sandbox", "echo", "hello"])
        assert args.json is True
        assert args.name == "my-sandbox"
        assert args.exec_command == ["echo", "hello"]

    def test_exec_no_command_error(self, args_namespace):
        """Test exec with no command returns an error."""
        args = args_namespace(name="test-sandbox", exec_command=[], json=False, local=False)

        with patch.object(sandbox, "print_error") as mock_error:
            result = sandbox.cmd_exec(args)

        assert result == 1
        mock_error.assert_called_once_with("No command provided")

    def test_exec_success_uses_sandbox_shell(self, args_namespace, mock_api_key, capsys):
        """Test successful execution uses the Fleet-backed shell interface."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["echo", "hello"],
            json=False,
            local=False,
        )
        command_result = SimpleNamespace(stdout="hello\n", stderr="", returncode=0)
        mock_sandbox = MagicMock()
        mock_sandbox.shell.run = AsyncMock(return_value=command_result)
        mock_sandbox.disconnect = AsyncMock()
        mock_connect = AsyncMock(return_value=mock_sandbox)
        mock_sandbox_sdk = MagicMock()
        mock_sandbox_sdk.Sandbox.connect = mock_connect

        with patch.dict("sys.modules", {"cua_sandbox": mock_sandbox_sdk}):
            with patch.object(
                sandbox,
                "_get_sandbox_api_url",
                side_effect=AssertionError("legacy resolver must not be used"),
            ) as mock_legacy_resolver:
                result = sandbox.cmd_exec(args)

        assert result == 0
        mock_connect.assert_awaited_once_with(
            "test-sandbox", local=False, api_key="test-access-token"
        )
        mock_sandbox.shell.run.assert_awaited_once_with("echo hello", timeout=120)
        mock_sandbox.disconnect.assert_awaited_once_with()
        mock_legacy_resolver.assert_not_called()
        assert capsys.readouterr().out == "hello\n"

    def test_exec_json_output(self, args_namespace, mock_api_key):
        """Test JSON output uses CommandResult fields."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["echo", "hello"],
            json=True,
            local=False,
        )
        command_result = SimpleNamespace(stdout="hello\n", stderr="", returncode=0)
        mock_sandbox = MagicMock()
        mock_sandbox.shell.run = AsyncMock(return_value=command_result)
        mock_sandbox.disconnect = AsyncMock()
        mock_sandbox_sdk = MagicMock()
        mock_sandbox_sdk.Sandbox.connect = AsyncMock(return_value=mock_sandbox)

        with patch.dict("sys.modules", {"cua_sandbox": mock_sandbox_sdk}):
            with patch.object(sandbox, "print_json") as mock_json:
                result = sandbox.cmd_exec(args)

        assert result == 0
        mock_json.assert_called_once_with({"stdout": "hello\n", "stderr": "", "returncode": 0})
        mock_sandbox.disconnect.assert_awaited_once_with()

    def test_exec_command_failure(self, args_namespace, mock_api_key, capsys):
        """Test a non-zero command result returns a clear failure."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["false"],
            json=False,
            local=False,
        )
        command_result = SimpleNamespace(stdout="", stderr="command failed\n", returncode=7)
        mock_sandbox = MagicMock()
        mock_sandbox.shell.run = AsyncMock(return_value=command_result)
        mock_sandbox.disconnect = AsyncMock()
        mock_sandbox_sdk = MagicMock()
        mock_sandbox_sdk.Sandbox.connect = AsyncMock(return_value=mock_sandbox)

        with patch.dict("sys.modules", {"cua_sandbox": mock_sandbox_sdk}):
            with patch.object(sandbox, "print_error") as mock_error:
                result = sandbox.cmd_exec(args)

        assert result == 1
        mock_error.assert_called_once_with("Command failed with exit code 7")
        assert capsys.readouterr().err == "command failed\n"
        mock_sandbox.disconnect.assert_awaited_once_with()

    def test_exec_sandbox_not_found(self, args_namespace, mock_api_key):
        """Test a nonexistent sandbox returns a connection error."""
        args = args_namespace(
            name="missing-sandbox",
            exec_command=["echo", "hello"],
            json=False,
            local=False,
        )
        mock_sandbox_sdk = MagicMock()
        mock_sandbox_sdk.Sandbox.connect = AsyncMock(
            side_effect=ValueError("Sandbox 'missing-sandbox' not found")
        )

        with patch.dict("sys.modules", {"cua_sandbox": mock_sandbox_sdk}):
            with patch.object(sandbox, "print_error") as mock_error:
                result = sandbox.cmd_exec(args)

        assert result == 1
        mock_error.assert_called_once_with(
            "Failed to connect to sandbox: Sandbox 'missing-sandbox' not found"
        )

    def test_exec_connection_failure(self, args_namespace, mock_api_key):
        """Test a transport connection failure returns a clear error."""
        args = args_namespace(
            name="test-sandbox",
            exec_command=["echo", "hello"],
            json=False,
            local=False,
        )
        mock_sandbox_sdk = MagicMock()
        mock_sandbox_sdk.Sandbox.connect = AsyncMock(
            side_effect=RuntimeError("Fleet service unavailable")
        )

        with patch.dict("sys.modules", {"cua_sandbox": mock_sandbox_sdk}):
            with patch.object(sandbox, "print_error") as mock_error:
                result = sandbox.cmd_exec(args)

        assert result == 1
        mock_error.assert_called_once_with(
            "Failed to connect to sandbox: Fleet service unavailable"
        )

    def test_exec_local_mode(self, args_namespace, capsys):
        """Test local mode connects without requesting cloud authentication."""
        args = args_namespace(
            name="local-sandbox",
            exec_command=["pwd"],
            json=False,
            local=True,
        )
        command_result = SimpleNamespace(stdout="/workspace\n", stderr="", returncode=0)
        mock_sandbox = MagicMock()
        mock_sandbox.shell.run = AsyncMock(return_value=command_result)
        mock_sandbox.disconnect = AsyncMock()
        mock_connect = AsyncMock(return_value=mock_sandbox)
        mock_sandbox_sdk = MagicMock()
        mock_sandbox_sdk.Sandbox.connect = mock_connect

        with patch.dict("sys.modules", {"cua_sandbox": mock_sandbox_sdk}):
            with patch.object(sandbox, "get_access_token", new_callable=AsyncMock) as mock_token:
                result = sandbox.cmd_exec(args)

        assert result == 0
        mock_token.assert_not_awaited()
        mock_connect.assert_awaited_once_with("local-sandbox", local=True)
        mock_sandbox.shell.run.assert_awaited_once_with("pwd", timeout=120)
        mock_sandbox.disconnect.assert_awaited_once_with()
        assert capsys.readouterr().out == "/workspace\n"

    def test_exec_with_wif_uses_sdk_without_legacy_auth_or_url(
        self, args_namespace, monkeypatch, capsys
    ):
        """Test workload identity executes through the Fleet SDK service proxy."""
        monkeypatch.setenv("FLEETS_TOKEN", "github-token")
        args = args_namespace(
            name="fleet-sandbox",
            exec_command=["echo", "hello"],
            json=False,
            local=False,
        )
        command_result = SimpleNamespace(stdout="hello\n", stderr="", returncode=0)
        mock_sandbox = MagicMock()
        mock_sandbox.shell.run = AsyncMock(return_value=command_result)
        mock_sandbox.disconnect = AsyncMock()
        mock_connect = AsyncMock(return_value=mock_sandbox)
        mock_sdk = MagicMock()
        mock_sdk.Sandbox.connect = mock_connect

        with patch.dict("sys.modules", {"cua_sandbox": mock_sdk}):
            with patch.object(sandbox, "get_access_token", new_callable=AsyncMock) as mock_token:
                with patch.object(sandbox, "_get_sandbox_api_url") as mock_api_url:
                    result = sandbox.cmd_exec(args)

        assert result == 0
        mock_token.assert_not_awaited()
        mock_api_url.assert_not_called()
        mock_connect.assert_awaited_once_with("fleet-sandbox", local=False)
        mock_sandbox.shell.run.assert_awaited_once_with("echo hello", timeout=120)
        mock_sandbox.disconnect.assert_awaited_once_with()
        assert capsys.readouterr().out == "hello\n"
