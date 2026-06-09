"""Tests for auth command module."""

import argparse
from types import SimpleNamespace
from unittest.mock import patch

from cua_cli.commands import auth


class TestRegisterParser:
    """Tests for register_parser function."""

    def test_registers_auth_command(self):
        """Test that auth command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()

        auth.register_parser(subparsers)

        args = parser.parse_args(["auth", "login"])
        assert args.auth_command == "login"

    def test_login_has_api_key_flag(self):
        """Test that login has --api-key flag."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        auth.register_parser(subparsers)

        args = parser.parse_args(["auth", "login", "--api-key", "my-key"])
        assert args.api_key == "my-key"

    def test_logout_command(self):
        """Test that logout command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        auth.register_parser(subparsers)

        args = parser.parse_args(["auth", "logout"])
        assert args.auth_command == "logout"

    def test_env_command(self):
        """Test that env command is registered."""
        parser = argparse.ArgumentParser()
        subparsers = parser.add_subparsers()
        auth.register_parser(subparsers)

        args = parser.parse_args(["auth", "env"])
        assert args.auth_command == "env"


class TestExecute:
    """Tests for execute function."""

    def test_dispatch_to_login(self, args_namespace):
        """Test dispatch to login command."""
        args = args_namespace(command="auth", auth_command="login", api_key=False)

        with patch.object(auth, "cmd_login", return_value=0) as mock_cmd:
            result = auth.execute(args)

        mock_cmd.assert_called_once_with(args)
        assert result == 0

    def test_dispatch_to_logout(self, args_namespace):
        """Test dispatch to logout command."""
        args = args_namespace(command="auth", auth_command="logout")

        with patch.object(auth, "cmd_logout", return_value=0) as mock_cmd:
            auth.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_dispatch_to_env(self, args_namespace):
        """Test dispatch to env command."""
        args = args_namespace(command="auth", auth_command="env")

        with patch.object(auth, "cmd_env", return_value=0) as mock_cmd:
            auth.execute(args)

        mock_cmd.assert_called_once_with(args)

    def test_unknown_command_returns_error(self, args_namespace):
        """Test that unknown command returns error."""
        args = args_namespace(command="auth", auth_command=None)

        with patch.object(auth, "print_error"):
            result = auth.execute(args)

        assert result == 1


class TestCmdLogin:
    """Tests for cmd_login function."""

    def test_login_with_api_key_direct(self, args_namespace):
        """Test login with --api-key flag and value."""
        args = args_namespace(api_key="test-api-key")

        with patch.object(
            auth,
            "_fetch_me",
            return_value=(
                200,
                {
                    "workspace": {"slug": "test-workspace", "name": "Test Workspace"},
                    "organization": {"name": "Test Org"},
                },
            ),
        ):
            with patch.object(auth, "save_workspace") as mock_save_workspace:
                with patch.object(auth, "set_active_workspace") as mock_set_active:
                    with patch.object(auth, "print_info"):
                        with patch.object(auth, "print_success"):
                            result = auth.cmd_login(args)

        mock_save_workspace.assert_called_once_with(
            "test-workspace",
            "test-api-key",
            "Test Workspace",
            "Test Org",
        )
        mock_set_active.assert_called_once_with("test-workspace")
        assert result == 0

    def test_login_with_api_key_legacy_response(self, args_namespace):
        """Test direct login when /v1/me returns no workspace metadata."""
        args = args_namespace(api_key="test-api-key")

        with patch.object(auth, "_fetch_me", return_value=(200, {})):
            with patch.object(auth, "save_api_key") as mock_save:
                with patch.object(auth, "_get_store") as mock_store:
                    with patch.object(auth, "print_info"):
                        with patch.object(auth, "print_success"):
                            result = auth.cmd_login(args)

        mock_save.assert_called_once_with("test-api-key")
        mock_store.return_value.delete.assert_called_once_with(auth.ACTIVE_WORKSPACE_KEY)
        assert result == 0

    def test_login_browser_flow(self, args_namespace):
        """Test login with browser OAuth flow."""
        args = args_namespace(api_key=None)

        with patch.object(auth, "run_async") as mock_run:
            mock_run.return_value = SimpleNamespace(
                token="browser-api-key",
                workspace_slug="test-workspace",
                workspace_name="Test Workspace",
                org_name="Test Org",
            )
            with patch.object(auth, "save_workspace") as mock_save_workspace:
                with patch.object(auth, "set_active_workspace") as mock_set_active:
                    with patch.object(auth, "print_success"):
                        result = auth.cmd_login(args)

        mock_save_workspace.assert_called_once_with(
            "test-workspace",
            "browser-api-key",
            "Test Workspace",
            "Test Org",
        )
        mock_set_active.assert_called_once_with("test-workspace")
        assert result == 0

    def test_login_browser_flow_timeout(self, args_namespace):
        """Test login when browser flow times out."""
        args = args_namespace(api_key=None)

        with patch.object(auth, "run_async") as mock_run:
            mock_run.side_effect = TimeoutError("Authentication timed out")
            with patch.object(auth, "print_error"):
                result = auth.cmd_login(args)

        assert result == 1


class TestCmdLogout:
    """Tests for cmd_logout function."""

    def test_logout_clears_legacy_credentials(self, args_namespace):
        """Test that logout clears legacy credentials when no workspace is active."""
        args = args_namespace()

        with patch.object(auth, "get_active_workspace", return_value=None):
            with patch.object(auth, "clear_legacy_credentials") as mock_clear:
                with patch.object(auth, "print_success"):
                    result = auth.cmd_logout(args)

        mock_clear.assert_called_once()
        assert result == 0


class TestCmdEnv:
    """Tests for cmd_env function."""

    def test_env_writes_to_dotenv(self, args_namespace, tmp_path, monkeypatch):
        """Test that env command writes to .env file."""
        args = args_namespace(file=str(tmp_path / ".env"))

        with patch.object(auth, "get_api_key", return_value="test-api-key"):
            with patch.object(auth, "print_success"):
                result = auth.cmd_env(args)

        assert result == 0
        env_file = tmp_path / ".env"
        assert env_file.exists()
        content = env_file.read_text()
        assert "CUA_API_KEY=test-api-key" in content

    def test_env_no_credentials_fails(self, args_namespace, tmp_path):
        """Test that env command fails when not logged in."""
        args = args_namespace(file=str(tmp_path / ".env"))

        with patch.object(auth, "get_api_key", return_value=None):
            with patch.object(auth, "print_error"):
                result = auth.cmd_env(args)

        assert result == 1

    def test_env_appends_to_existing_dotenv(self, args_namespace, tmp_path):
        """Test that env command appends to existing .env file."""
        env_file = tmp_path / ".env"
        env_file.write_text("EXISTING_VAR=value\n")
        args = args_namespace(file=str(env_file))

        with patch.object(auth, "get_api_key", return_value="test-api-key"):
            with patch.object(auth, "print_success"):
                result = auth.cmd_env(args)

        assert result == 0
        content = env_file.read_text()
        assert "EXISTING_VAR=value" in content
        assert "CUA_API_KEY=test-api-key" in content
